//! `cargo run -p xtask --release -- bench`: measure the PRD performance budgets (§15.1 and
//! the timed acceptance criteria) on PRD-scale fixtures, print a table, and exit non-zero
//! if any budget is missed. Run in release mode; debug timings are meaningless.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use trellis_core::outline;
use trellis_core::{BlockId, BlockKind, Command, PageId};
use trellis_store::{EditOpts, RefQuery, Store};

struct Row {
    name: &'static str,
    budget: f64,
    unit: &'static str,
    measured: f64,
}

#[derive(Default)]
struct Report {
    rows: Vec<Row>,
}

impl Report {
    fn ms(&mut self, name: &'static str, budget_ms: f64, d: Duration) {
        self.rows.push(Row {
            name,
            budget: budget_ms,
            unit: "ms",
            measured: d.as_secs_f64() * 1000.0,
        });
    }

    fn value(&mut self, name: &'static str, budget: f64, unit: &'static str, measured: f64) {
        self.rows.push(Row {
            name,
            budget,
            unit,
            measured,
        });
    }

    fn print(&self) -> bool {
        println!(
            "\n{:<58} {:>12} {:>12}  result",
            "budget (PRD)", "limit", "measured"
        );
        let mut ok = true;
        for r in &self.rows {
            let pass = r.measured <= r.budget;
            ok &= pass;
            println!(
                "{:<58} {:>9.1} {:<2} {:>9.3} {:<2}  {}",
                r.name,
                r.budget,
                r.unit,
                r.measured,
                r.unit,
                if pass { "ok" } else { "OVER BUDGET" }
            );
        }
        ok
    }
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let v = f();
    (v, t.elapsed())
}

fn percentile(mut v: Vec<Duration>, p: f64) -> Duration {
    v.sort();
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 2).expect("valid date")
}

/// Insert `texts` as a flat list of root blocks after the page's last root.
fn insert_flat(
    s: &mut Store,
    page: &PageId,
    texts: impl Iterator<Item = String>,
) -> Result<Vec<BlockId>> {
    let mut after = s.page_state(page)?.last_child(None, false);
    let mut cmds = Vec::new();
    let mut ids = Vec::new();
    for t in texts {
        let id = BlockId::new();
        cmds.push(Command::InsertBlock {
            id: id.clone(),
            parent: None,
            after: after.clone(),
            text: t,
            kind: BlockKind::Bullet,
        });
        after = Some(id.clone());
        ids.push(id);
    }
    s.execute(
        page,
        cmds,
        EditOpts {
            no_undo: true,
            ..Default::default()
        },
    )?;
    Ok(ids)
}

struct Fixture {
    big: PageId,
    big_ids: Vec<BlockId>,
    mindfulness: PageId,
    books: PageId,
    sample_block: (PageId, BlockId),
}

/// 100 pages × 1,000 blocks (100k blocks), 2,000 of them referencing [[Mindfulness]] and
/// 37 of those also [[Books]] (AC-3.6), plus a 5,000-block page (AC-2.1).
fn build(s: &mut Store) -> Result<Fixture> {
    let mindfulness = s.create_page("Mindfulness")?;
    let books = s.create_page("Books")?;
    let mut books_left = 37;
    let mut sample_block = None;
    for p in 0..100 {
        let page = s.create_page(&format!("Research page {p}"))?;
        let texts = (0..1000).map(|b| {
            let n = p * 1000 + b;
            if n % 50 == 0 {
                let also = if books_left > 0 && n % 100 == 0 {
                    books_left -= 1;
                    " and [[Books]]"
                } else {
                    ""
                };
                format!("note {n} on [[Mindfulness]]{also} with topic {}", n % 97)
            } else {
                format!("note {n} about topic {} and some ordinary words", n % 97)
            }
        });
        let ids = insert_flat(s, &page, texts.collect::<Vec<_>>().into_iter())?;
        if p == 50 {
            sample_block = Some((page.clone(), ids[1].clone()));
        }
    }
    let big = s.create_page("Big page")?;
    let big_ids = insert_flat(
        s,
        &big,
        (0..5000).map(|i| format!("big block {i} with a little text")),
    )?;
    s.sync_index()?;
    Ok(Fixture {
        big,
        big_ids,
        mindfulness,
        books,
        sample_block: sample_block.context("fixture block")?,
    })
}

fn rss_kb(pid: u32) -> Result<f64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().parse::<f64>()?)
}

/// Child process: open the graph, load three pages, report resident memory (§15.1).
fn rss_probe(db: &Path) -> Result<()> {
    let mut s = Store::open(db)?;
    let pages: Vec<PageId> = s
        .list_pages(false)?
        .into_iter()
        .take(3)
        .map(|p| p.id)
        .collect();
    for p in &pages {
        s.page_state(p)?;
    }
    s.linked_references(&pages[0], &RefQuery::default())?;
    println!("{}", rss_kb(std::process::id())?);
    Ok(())
}

fn sync_round_trip(r: &mut Report) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let db = Arc::new(trellis_syncd::Db::open_in_memory().map_err(|e| anyhow::anyhow!("{e:?}"))?);
    let app = trellis_syncd::router(db, trellis_syncd::Config::default());
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))?;
    let base = format!("http://{}", listener.local_addr()?);
    rt.spawn(async move { axum_serve(listener, app).await });
    let transport =
        |s: &Store| -> Result<trellis_sync::SyncClient<trellis_sync::http::HttpTransport>> {
            Ok(trellis_sync::SyncClient::new(
                trellis_sync::http::HttpTransport::new(&base, "bench", None)?,
                s,
            )?)
        };
    let mut a = Store::open_in_memory()?;
    let mut b = Store::open_in_memory()?;
    let mut ca = transport(&a)?;
    let mut cb = transport(&b)?;
    let page = a.create_page("Synced")?;
    let ids = insert_flat(&mut a, &page, (0..200).map(|i| format!("line {i}")))?;
    ca.sync_once(&mut a)?;
    cb.sync_once(&mut b)?;
    for id in ids.iter().take(10) {
        a.set_block_text(&page, id, "changed on A", None, None)?;
    }
    let (_, d) = time(|| -> Result<()> {
        ca.sync_once(&mut a)?;
        cb.sync_once(&mut b)?;
        Ok(())
    });
    let st = b.page_state(&page)?;
    if st.block(&ids[0]).map(|x| x.text.as_str()) != Some("changed on A") {
        bail!("sync bench: change did not arrive");
    }
    r.ms("Sync incremental, 10 changed blocks, round trip", 500.0, d);
    Ok(())
}

async fn axum_serve(listener: tokio::net::TcpListener, app: trellis_syncd::Router) {
    let _ = trellis_syncd::serve(listener, app).await;
}

/// Launch the release app with `TRELLIS_PROBE` and parse its measurements.
fn probe(graph: &Path, config: &Path) -> Result<(f64, f64, f64)> {
    let exe = std::env::current_exe()?.with_file_name("trellis");
    if !exe.exists() {
        bail!("build the app first: cargo build --release -p trellis-app");
    }
    let out = std::process::Command::new(&exe)
        .env("TRELLIS_PROBE", "1")
        .env("TRELLIS_GRAPH", graph)
        .env("TRELLIS_CONFIG_DIR", config)
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.starts_with("probe "))
        .with_context(|| {
            format!(
                "probe printed nothing: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        })?;
    let get = |k: &str| -> Result<f64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
            .context("probe field")?
            .parse::<f64>()
            .map_err(Into::into)
    };
    Ok((
        get("first_frame_ms")?,
        get("key_to_frame_p50_ms")?,
        get("key_to_frame_p95_ms")?,
    ))
}

fn bench(ui: bool) -> Result<()> {
    if cfg!(debug_assertions) {
        eprintln!("warning: run with --release; debug builds are 10–50× slower");
    }
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("bench.trellis");
    let mut r = Report::default();
    let fx;
    {
        let mut s = Store::open(&path)?;
        s.set_today(Some(today()));
        let (f, d) = time(|| build(&mut s));
        fx = f?;
        // The app opens today's journal page; give it a realistic 1,000 blocks.
        let (journal, _) = s.open_daily(chrono::Local::now().date_naive())?;
        insert_flat(
            &mut s,
            &journal,
            (0..1000).map(|i| format!("journal line {i} with #journal")),
        )?;
        s.sync_index()?;
        println!(
            "fixture: 100k-block graph + 5k-block page built in {:.1} s",
            d.as_secs_f64()
        );

        // AC-3.1 / §15.1 backlinks, AC-3.6 co-reference filter.
        let (refs, d) = time(|| s.linked_references(&fx.mindfulness, &RefQuery::default()));
        if refs?.shown() != 2000 {
            bail!("expected 2,000 backlinks");
        }
        r.ms("Backlink panel, 2,000 refs (AC-3.1)", 100.0, d);
        let q = RefQuery {
            also: vec![fx.books.clone()],
            ..Default::default()
        };
        let (refs, d) = time(|| s.linked_references(&fx.mindfulness, &q));
        if refs?.shown() != 37 {
            bail!("expected 37 co-referenced sources");
        }
        r.ms(
            "Co-reference filter, Mindfulness ∩ Books (AC-3.6)",
            120.0,
            d,
        );

        // Full-text search over 100k blocks.
        let mut times = Vec::new();
        for q in ["topic 42", "ordinary words", "note 77777", "mindfulness"] {
            let (_, d) = time(|| s.search_blocks(q, 20));
            times.push(d);
        }
        r.ms(
            "Full-text search, 100k blocks (p95 of 4)",
            50.0,
            percentile(times, 0.95),
        );

        // AC-3.5: editing a block in a 100k graph updates its backlinks (commit included).
        let (page, block) = fx.sample_block.clone();
        let mut times = Vec::new();
        for i in 0..20 {
            let text = if i % 2 == 0 {
                "now about [[Mindfulness]]"
            } else {
                "plain again"
            };
            let (res, d) = time(|| -> Result<()> {
                s.set_block_text(&page, &block, text, None, None)?;
                s.sync_index()?;
                Ok(())
            });
            res?;
            times.push(d);
        }
        r.ms(
            "Edit + commit + backlink update, 100k blocks (AC-3.5, p50)",
            5.0,
            percentile(times, 0.5),
        );

        // Keystroke path in debounced mode (the store's share of keystroke→paint).
        s.set_debounced(true)?;
        let target = fx.big_ids[2500].clone();
        let mut text = String::new();
        let mut times = Vec::new();
        for c in "the quick brown fox jumps over the lazy dog".chars() {
            text.push(c);
            let (res, d) = time(|| s.set_block_text(&fx.big, &target, &text, None, None));
            res?;
            times.push(d);
        }
        r.ms(
            "Keystroke apply on a 5k-block page (p50)",
            8.0,
            percentile(times.clone(), 0.5),
        );
        r.ms(
            "Keystroke apply on a 5k-block page (p95)",
            16.0,
            percentile(times, 0.95),
        );
        s.set_debounced(false)?;

        // AC-2.4: indent a block carrying 300 descendants.
        let parent = fx.big_ids[999].clone();
        let kids: Vec<BlockId> = fx.big_ids[1000..1300].to_vec();
        let cmds = kids
            .iter()
            .enumerate()
            .map(|(i, k)| Command::MoveBlock {
                id: k.clone(),
                parent: Some(parent.clone()),
                after: if i == 0 {
                    None
                } else {
                    Some(kids[i - 1].clone())
                },
            })
            .collect();
        s.execute(&fx.big, cmds, EditOpts::default())?;
        s.sync_index()?;
        let (res, d) = time(|| -> Result<()> {
            let plan = outline::indent(s.page_state(&fx.big)?, &parent)?;
            s.execute_plan(&fx.big, plan, None)?;
            s.sync_index()?;
            Ok(())
        });
        res?;
        r.ms("Indent a block with 300 descendants (AC-2.4)", 20.0, d);

        // AC-2.5: paste 500 lines of nested Markdown as one undo group.
        let paste_page = s.create_page("Paste target")?;
        let first = s.ensure_first_block(&paste_page)?;
        let md: String = (0..500)
            .map(|i| format!("{}- pasted {i}\n", "  ".repeat(i % 4)))
            .collect();
        let (res, d) = time(|| -> Result<()> {
            let plan = outline::paste(s.page_state(&paste_page)?, &first, 0..0, &md)?;
            s.execute_plan(&paste_page, plan, None)?;
            s.sync_index()?;
            Ok(())
        });
        res?;
        r.ms("Paste 500 Markdown lines (AC-2.5)", 400.0, d);

        // AC-4.7: rename onto an existing title merges 1,000 blocks.
        let a = s.create_page("Apple Watch")?;
        let b = s.create_page("applewatch")?;
        insert_flat(&mut s, &b, (0..1000).map(|i| format!("watch note {i}")))?;
        s.sync_index()?;
        // §13.3: the merge is applied to the in-memory shards (and visible, undoable) when
        // rename returns; the SQLite commit follows on the writer thread.
        let (res, applied) = time(|| s.rename_page(&b, "apple watch"));
        res?;
        let (res, durable) = time(|| s.sync_index());
        res?;
        if s.page_state(&a)?.live_count() != 1000 {
            bail!("merge moved the wrong number of blocks");
        }
        r.ms(
            "Rename-merge of 1,000 blocks, applied (AC-4.7)",
            30.0,
            applied,
        );
        // Not a PRD budget: a guard so the background commit stays well under a second.
        r.ms(
            "  … then durable on the writer thread (async guard)",
            250.0,
            durable,
        );

        // AC-1.4: backfill 30 days; a day is written by its first character (FR-1.2).
        let (res, d) = time(|| -> Result<()> {
            for i in 0..30 {
                let page = s.draft_daily(today() - chrono::Days::new(40 + i))?;
                let block = s.page_state(&page)?.live_roots()[0].clone();
                s.set_block_text(&page, &block, "x", None, None)?;
            }
            s.sync_index()?;
            Ok(())
        });
        res?;
        r.ms("Backfill 30 journal days (AC-1.4)", 300.0, d);

        // G4: full export.
        let out = dir.path().join("export");
        let (res, d) = time(|| -> Result<()> {
            s.export_markdown(&out)?;
            s.export_json(&out.join("graph.json"))?;
            Ok(())
        });
        res?;
        r.ms("Export Markdown + JSON, 100k blocks (G4)", 10_000.0, d);
        s.compact()?;
    }
    let size_mb = std::fs::metadata(&path)?.len() as f64 / (1 << 20) as f64;
    r.value("DB size, 100k blocks", 150.0, "MB", size_mb);

    // AC-2.1 / §15.1: open the 5k-block page in a fresh process state (cold shard load).
    let mut s = Store::open(&path)?;
    let (res, d) = time(|| -> Result<usize> {
        let st = s.page_state(&fx.big)?;
        Ok(st.visible_rows(None, &|_| false).len())
    });
    res?;
    r.ms("Open a 5,000-block page, cold shard load (AC-2.1)", 50.0, d);
    let (res, d) = time(|| -> Result<usize> {
        let st = s.page_state(&fx.big)?;
        Ok(st.visible_rows(None, &|_| false).len())
    });
    res?;
    r.ms("Open a 5,000-block page, warm (AC-2.1)", 50.0, d);
    drop(s);

    // §15.1 hot start → caret in today's note (data path; window paint not included).
    let (res, d) = time(|| -> Result<()> {
        let mut s = Store::open(&path)?;
        s.set_today(Some(today()));
        s.open_daily(today())?;
        Ok(())
    });
    res?;
    r.ms("Start → today's first block ready (data path)", 800.0, d);

    // §15.1 memory: a separate process with three open pages.
    let exe = std::env::current_exe()?;
    let out = std::process::Command::new(exe)
        .arg("rss-probe")
        .arg(&path)
        .output()?;
    let kb: f64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .context("rss probe")?;
    r.value(
        "Memory, 100k-block graph with 3 open pages (RSS)",
        300.0,
        "MB",
        kb / 1024.0,
    );

    sync_round_trip(&mut r)?;

    if ui {
        // A real window: first frame and keystroke → full render/layout/paint (§15.1,
        // AC-2.2). Window present and display refresh (vsync) are not included.
        let config = dir.path().join("probe-config");
        let (first_launch, _, _) = probe(&path, &config)?;
        println!(
            "first launch after a build (binary page-in, shader compile): {first_launch:.0} ms"
        );
        let (first_frame, p50, p95) = probe(&path, &config)?;
        r.ms(
            "Start → window drawn, caret in today (§15.1 cold ≤ 400)",
            400.0,
            Duration::from_secs_f64(first_frame / 1000.0),
        );
        r.ms(
            "Keystroke → frame rendered (AC-2.2, p50)",
            8.0,
            Duration::from_secs_f64(p50 / 1000.0),
        );
        r.ms(
            "Keystroke → frame rendered (AC-2.2, p95)",
            16.0,
            Duration::from_secs_f64(p95 / 1000.0),
        );
    }

    if !r.print() {
        bail!("some budgets were missed");
    }
    println!("\nall budgets met");
    Ok(())
}

/// `Info.plist` for the app bundle. `CFBundleURLTypes` claims the `trellis://` scheme so
/// links from other apps reach `on_open_urls` (P2 automation).
fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Trellis</string>
  <key>CFBundleDisplayName</key><string>Trellis</string>
  <key>CFBundleIdentifier</key><string>app.trellis.Trellis</string>
  <key>CFBundleExecutable</key><string>trellis</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>app.trellis.link</string>
      <key>CFBundleURLSchemes</key><array><string>trellis</string></array>
    </dict>
  </array>
</dict>
</plist>
"#
    )
}

/// Build the app and lay out `target/Trellis.app`.
fn bundle(debug: bool) -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut build = std::process::Command::new(env!("CARGO"));
    build
        .current_dir(&root)
        .args(["build", "-p", "trellis-app"]);
    if !debug {
        build.arg("--release");
    }
    anyhow::ensure!(build.status()?.success(), "cargo build failed");
    let profile = if debug { "debug" } else { "release" };
    let app = root.join("target/Trellis.app");
    let contents = app.join("Contents");
    if app.exists() {
        std::fs::remove_dir_all(&app)?;
    }
    std::fs::create_dir_all(contents.join("MacOS"))?;
    std::fs::create_dir_all(contents.join("Resources"))?;
    std::fs::copy(
        root.join("target").join(profile).join("trellis"),
        contents.join("MacOS/trellis"),
    )?;
    std::fs::write(
        contents.join("Info.plist"),
        info_plist(env!("CARGO_PKG_VERSION")),
    )?;
    std::fs::write(contents.join("PkgInfo"), "APPL????")?;
    println!("{}", app.canonicalize()?.display());
    Ok(())
}

fn run(cmd: &mut std::process::Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("running {cmd:?}"))?;
    anyhow::ensure!(status.success(), "{cmd:?} failed: {status}");
    Ok(())
}

/// The mobile pipeline (PRD §14): build the UniFFI core, generate Swift bindings, run the
/// Swift check against the real core, build the SwiftUI shell for macOS, and type-check
/// it against the iOS Simulator SDK. `--ios` also links the app for the simulator
/// (needs `rustup target add aarch64-apple-ios-sim`).
fn mobile(ios: bool) -> Result<()> {
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()?;
    let cargo = env!("CARGO");
    let out = root.join("target/mobile");
    let gen = out.join("gen");
    std::fs::create_dir_all(&gen)?;
    run(Command::new(cargo)
        .current_dir(&root)
        .args(["build", "-p", "trellis-ffi"]))?;
    let lib = root.join("target/debug/libtrellis_ffi.a");
    run(Command::new(cargo).current_dir(&root).args([
        "run",
        "-q",
        "-p",
        "trellis-ffi",
        "--bin",
        "uniffi-bindgen",
        "--",
        "generate",
        "--library",
        root.join("target/debug/libtrellis_ffi.dylib")
            .to_str()
            .unwrap(),
        "--language",
        "swift",
        "--out-dir",
        gen.to_str().unwrap(),
    ]))?;
    // Kotlin too: the same surface for an Android shell (compiling it needs kotlinc).
    run(Command::new(cargo).current_dir(&root).args([
        "run",
        "-q",
        "-p",
        "trellis-ffi",
        "--bin",
        "uniffi-bindgen",
        "--",
        "generate",
        "--no-format",
        "--library",
        root.join("target/debug/libtrellis_ffi.dylib")
            .to_str()
            .unwrap(),
        "--language",
        "kotlin",
        "--out-dir",
        out.join("kotlin").to_str().unwrap(),
    ]))?;
    let bindings = gen.join("trellis_ffi.swift");
    let modulemap = format!(
        "-fmodule-map-file={}",
        gen.join("trellis_ffiFFI.modulemap").display()
    );
    let app_sources: Vec<PathBuf> = std::fs::read_dir(root.join("mobile/ios/TrellisMobile"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "swift"))
        .collect();
    let swiftc = |sdk: &str| {
        let mut c = Command::new("xcrun");
        c.args(["--sdk", sdk, "swiftc", "-swift-version", "5"])
            .arg(&bindings)
            .args(["-Xcc", &modulemap, "-I"])
            .arg(&gen);
        c
    };
    let link = |c: &mut Command, lib: &Path| {
        c.arg(lib).args([
            "-framework",
            "Security",
            "-framework",
            "CoreFoundation",
            "-framework",
            "SystemConfiguration",
        ]);
    };
    // 1. The bindings against the real core.
    let check = out.join("check");
    let mut c = swiftc("macosx");
    c.arg(root.join("mobile/ios/Check/main.swift"))
        .arg("-o")
        .arg(&check);
    link(&mut c, &lib);
    run(&mut c)?;
    run(&mut Command::new(&check))?;
    // 2. The shell on macOS (a preview of the same SwiftUI code).
    let mac = out.join("TrellisMobile-macos");
    let mut c = swiftc("macosx");
    c.arg("-parse-as-library")
        .args(&app_sources)
        .arg("-o")
        .arg(&mac);
    link(&mut c, &lib);
    run(&mut c)?;
    // 3. The shell against the iOS SDK.
    let mut c = swiftc("iphonesimulator");
    c.args([
        "-parse-as-library",
        "-typecheck",
        "-target",
        "arm64-apple-ios17.0-simulator",
    ])
    .args(&app_sources);
    run(&mut c)?;
    println!(
        "mobile: bindings checked, macOS preview at {}, iOS type-check ok, Kotlin bindings in {}",
        mac.display(),
        out.join("kotlin").display()
    );
    if !ios {
        return Ok(());
    }
    // 4. The iOS Simulator app.
    let target = "aarch64-apple-ios-sim";
    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(target))
        .unwrap_or(false);
    anyhow::ensure!(
        installed,
        "the Rust target {target} is not installed; run `rustup target add {target}` first"
    );
    run(Command::new(cargo).current_dir(&root).args([
        "build",
        "-p",
        "trellis-ffi",
        "--target",
        target,
    ]))?;
    let app = out.join("TrellisMobile.app");
    if app.exists() {
        std::fs::remove_dir_all(&app)?;
    }
    std::fs::create_dir_all(&app)?;
    let mut c = swiftc("iphonesimulator");
    c.args([
        "-parse-as-library",
        "-target",
        "arm64-apple-ios17.0-simulator",
    ])
    .args(&app_sources)
    .arg("-o")
    .arg(app.join("TrellisMobile"));
    link(
        &mut c,
        &root.join(format!("target/{target}/debug/libtrellis_ffi.a")),
    );
    run(&mut c)?;
    std::fs::copy(
        root.join("mobile/ios/TrellisMobile/Info.plist"),
        app.join("Info.plist"),
    )?;
    run(Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app))?;
    println!(
        "mobile: {} built; install with `xcrun simctl install booted {}`",
        app.display(),
        app.display()
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("bench") => bench(args.iter().any(|a| a == "--ui")),
        Some("profile-merge") => {
            let dir = tempfile::tempdir()?;
            let mut s = Store::open(dir.path().join("p.trellis"))?;
            let filler = s.create_page("Filler")?;
            insert_flat(&mut s, &filler, (0..5_000).map(|i| format!("filler {i}")))?;
            for round in 0..150 {
                let a = s.create_page(&format!("Target {round}"))?;
                let b = s.create_page(&format!("Dup{round}"))?;
                insert_flat(&mut s, &b, (0..1000).map(|i| format!("watch note {i}")))?;
                s.sync_index()?;
                let t = Instant::now();
                s.merge_pages(&b, &a)?;
                s.sync_index()?;
                println!(
                    "merge {round}: {:.1} ms",
                    t.elapsed().as_secs_f64() * 1000.0
                );
            }
            Ok(())
        }
        Some("rss-probe") => rss_probe(Path::new(args.get(1).context("db path")?)),
        Some("bundle") => bundle(args.iter().any(|a| a == "--debug")),
        Some("mobile") => mobile(args.iter().any(|a| a == "--ios")),
        _ => {
            println!(
                "usage: cargo run -p xtask --release -- bench [--ui]\n       cargo run -p xtask -- bundle [--debug]   (target/Trellis.app, registers trellis://)\n       cargo run -p xtask -- mobile [--ios]     (Swift bindings, checks, iOS shell)\n       cargo run -p xtask --release -- profile-merge   (loop for sampling with `sample`)"
            );
            Ok(())
        }
    }
}
