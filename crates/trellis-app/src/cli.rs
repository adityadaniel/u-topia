//! Headless commands: `trellis doctor <graph>` (§10.4, §15.2) and
//! `trellis export <graph> <dir>` (G4).

use std::path::Path;

use anyhow::{bail, Context, Result};
use trellis_store::Store;

pub const USAGE: &str = "\
usage:
  trellis [--graph <file.trellis>]        open the desktop app
  trellis doctor <file.trellis> [--rebuild]  check invariants (and rebuild the index)
  trellis export <file.trellis> <dir>     write Markdown files, graph.json and graph.opml to <dir>
  trellis import-roam <file.trellis> <roam.json>   import a Roam JSON export
  trellis capture <text…>                 append a line to today's note (bind to any OS shortcut)
  trellis serve-api <file.trellis> [addr]  run the local automation API without the app
  trellis encrypt <file.trellis>          encrypt the graph at rest (or change its passphrase)
  trellis decrypt <file.trellis>          remove encryption at rest
  trellis trellis://page/<title>          open a trellis:// link (page, journal, block, search, capture)

encrypted graphs: the passphrase comes from TRELLIS_DB_PASSPHRASE, the macOS keychain
(service \"trellis-db\", account = graph name), or a prompt. It is never stored in the graph.

automation API: TRELLIS_API=1 (or TRELLIS_API=127.0.0.1:<port>) starts it with the app; the
URL and a per-launch bearer token are written to api.json in the config directory.

sync (optional): set TRELLIS_SYNC_URL=<relay origin>, and optionally TRELLIS_SYNC_GRAPH=<id>
and TRELLIS_SYNC_TOKEN=<token> (or a macOS keychain item, service \"trellis-sync\").";

/// Read a passphrase from the terminal without echoing it.
fn prompt_secret(prompt: &str) -> Result<String> {
    use std::io::Write;
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let tty = std::fs::File::open("/dev/tty").ok();
    let stty = |arg: &str| {
        if let Some(t) = &tty {
            let _ = std::process::Command::new("/bin/stty")
                .arg(arg)
                .stdin(t.try_clone().expect("tty"))
                .status();
        }
    };
    stty("-echo");
    let mut line = String::new();
    let r = std::io::stdin().read_line(&mut line);
    stty("echo");
    eprintln!();
    r?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Open a graph for a command, unlocking it if it is encrypted at rest.
pub fn open_store(graph: &Path) -> Result<Store> {
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    if !trellis_store::is_encrypted(graph) {
        return Store::open(graph).with_context(|| format!("opening {}", graph.display()));
    }
    let pass = match crate::paths::db_passphrase(graph) {
        Some(p) => p,
        None => prompt_secret("passphrase: ")?,
    };
    Store::open_with_key(graph, Some(&pass)).with_context(|| format!("opening {}", graph.display()))
}

/// `trellis encrypt`: encrypt a plain graph, or change an encrypted graph's passphrase.
pub fn encrypt(graph: &Path) -> Result<()> {
    if !trellis_store::encryption_available() {
        bail!("this build has no encryption at rest (SQLCipher)");
    }
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    let current = if trellis_store::is_encrypted(graph) {
        Some(match crate::paths::db_passphrase(graph) {
            Some(p) => p,
            None => prompt_secret("current passphrase: ")?,
        })
    } else {
        None
    };
    let new = match std::env::var("TRELLIS_DB_NEW_PASSPHRASE")
        .ok()
        .filter(|v| !v.is_empty())
    {
        Some(p) => p,
        None => {
            let a = prompt_secret("new passphrase: ")?;
            let b = prompt_secret("again: ")?;
            if a != b {
                bail!("the passphrases differ");
            }
            a
        }
    };
    trellis_store::set_encryption(graph, current.as_deref(), Some(&new))?;
    println!(
        "{} is encrypted at rest. Keep the passphrase safe: without it the graph cannot be read.",
        graph.display()
    );
    Ok(())
}

/// `trellis decrypt`: back to a plain SQLite file.
pub fn decrypt(graph: &Path) -> Result<()> {
    if !trellis_store::is_encrypted(graph) {
        bail!("{} is not encrypted", graph.display());
    }
    let pass = match crate::paths::db_passphrase(graph) {
        Some(p) => p,
        None => prompt_secret("passphrase: ")?,
    };
    trellis_store::set_encryption(graph, Some(&pass), None)?;
    println!("{} is no longer encrypted", graph.display());
    Ok(())
}

pub fn doctor(graph: &Path, rebuild: bool) -> Result<bool> {
    let mut store = open_store(graph)?;
    if rebuild {
        store.rebuild_index()?;
        println!("index rebuilt from page shards");
    }
    let report = store.doctor()?;
    println!(
        "{} live pages, {} live blocks, {} references",
        report.pages, report.blocks, report.refs
    );
    if report.is_ok() {
        println!("ok: all invariants hold");
    } else {
        for p in &report.problems {
            println!("problem: {p}");
        }
    }
    Ok(report.is_ok())
}

pub fn export(graph: &Path, dir: &Path) -> Result<()> {
    let mut store = open_store(graph)?;
    let n = store.export_markdown(dir)?;
    store.export_json(&dir.join("graph.json"))?;
    store.export_opml(&dir.join("graph.opml"))?;
    println!("exported {n} pages to {}", dir.display());
    Ok(())
}

pub fn import_roam(graph: &Path, file: &Path) -> Result<()> {
    let json =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    // Importing may create the graph.
    let mut store = if graph.exists() {
        open_store(graph)?
    } else {
        Store::open(graph).with_context(|| format!("opening {}", graph.display()))?
    };
    let r = store.import_roam_json(&json)?;
    println!("imported {} pages, {} blocks", r.pages, r.blocks);
    Ok(())
}

pub fn capture(graph: &Path, text: &str) -> Result<()> {
    if text.trim().is_empty() {
        bail!("nothing to capture");
    }
    let mut store = open_store(graph)?;
    let (page, _) = store.capture(text.trim())?;
    let title = store.page_row(&page)?.map(|r| r.title).unwrap_or_default();
    println!("captured to {title}");
    Ok(())
}

/// `trellis serve-api`: the automation API over a graph with no window, for scripts and
/// servers. Runs until killed.
pub fn serve_api(graph: &Path, addr: &str, discovery: &Path) -> Result<()> {
    let mut store = open_store(graph)?;
    let server = trellis_api::serve(addr, trellis_api::new_token())?;
    server.write_discovery(discovery)?;
    println!(
        "automation API on {} (token in {})",
        server.url(),
        discovery.display()
    );
    loop {
        if let Some(call) = server.recv_timeout(std::time::Duration::from_secs(1)) {
            call.run(&mut store);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_and_export_on_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.trellis");
        {
            let mut s = Store::open(&graph).unwrap();
            let (p, b) = s.open_daily(s.today()).unwrap();
            s.set_block_text(&p, &b, "hello [[World]]", None, None)
                .unwrap();
            s.commit_block(&p, &b).unwrap();
        }
        assert!(doctor(&graph, false).unwrap());
        assert!(doctor(&graph, true).unwrap());
        let out = dir.path().join("out");
        export(&graph, &out).unwrap();
        assert!(out.join("graph.json").exists());
        assert!(out.join("graph.opml").exists());
        capture(&graph, "from the command line").unwrap();
        let roam = dir.path().join("roam.json");
        std::fs::write(
            &roam,
            r#"[{"title":"Imported","children":[{"string":"hi","uid":"u1"}]}]"#,
        )
        .unwrap();
        import_roam(&graph, &roam).unwrap();
        let mut s = Store::open(&graph).unwrap();
        assert!(s.find_page("Imported").unwrap().is_some());
        assert!(s.doctor().unwrap().is_ok());
        assert!(out.join("World.md").exists());
        assert!(doctor(&dir.path().join("missing.trellis"), false).is_err());
    }
}
