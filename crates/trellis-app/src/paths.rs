//! Where graphs and config live (§11.6, §9.3).

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// App config directory: `TRELLIS_CONFIG_DIR`, else the platform convention.
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("TRELLIS_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/Trellis")
    } else if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(x).join("trellis")
    } else {
        home().join(".config/trellis")
    }
}

/// Default folder for graphs created by onboarding: `~/Documents/Trellis`.
pub fn graphs_dir() -> PathBuf {
    home().join("Documents/Trellis")
}

/// `<name>.trellis` with path separators and leading dots removed.
pub fn graph_file_name(name: &str) -> Option<String> {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim_start_matches('.').trim().to_string();
    (!clean.is_empty()).then(|| format!("{clean}.trellis"))
}

/// Pick the graph to open: explicit argument, `TRELLIS_GRAPH`, the last graph opened,
/// or the only graph in the default folder. `None` means onboarding.
pub fn resolve_graph(explicit: Option<PathBuf>, config: &Path, graphs: &Path) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    if let Some(p) = std::env::var_os("TRELLIS_GRAPH") {
        return Some(PathBuf::from(p));
    }
    if let Ok(last) = std::fs::read_to_string(config.join("last_graph")) {
        let p = PathBuf::from(last.trim());
        if p.exists() {
            return Some(p);
        }
    }
    let found: Vec<PathBuf> = std::fs::read_dir(graphs)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "trellis"))
        .collect();
    (found.len() == 1).then(|| found[0].clone())
}

/// The sync graph id: `TRELLIS_SYNC_GRAPH`, else the graph's file name made URL-safe.
pub fn graph_id(graph: &Path) -> String {
    if let Ok(g) = std::env::var("TRELLIS_SYNC_GRAPH") {
        if !g.trim().is_empty() {
            return g;
        }
    }
    graph
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Sync settings (§12): `TRELLIS_SYNC_URL` (relay) or `TRELLIS_SYNC_FOLDER` (shared folder)
/// enables sync; the graph id defaults to the graph's file name. Secrets — the relay token
/// and the end-to-end passphrase — come from env or the macOS keychain (services
/// `trellis-sync` and `trellis-sync-passphrase`, account = graph id), never from the graph
/// file (§15.6).
pub fn sync_config(graph: &Path) -> Option<trellis_ui::sync_runner::SyncConfig> {
    use trellis_ui::sync_runner::{SyncConfig, SyncTarget};
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let graph_id = graph_id(graph);
    let target = if let Some(url) = env("TRELLIS_SYNC_URL") {
        let token =
            env("TRELLIS_SYNC_TOKEN").or_else(|| keychain_secret("trellis-sync", &graph_id));
        SyncTarget::Relay { url, token }
    } else {
        SyncTarget::Folder(PathBuf::from(env("TRELLIS_SYNC_FOLDER")?))
    };
    let passphrase = env("TRELLIS_SYNC_PASSPHRASE")
        .or_else(|| keychain_secret("trellis-sync-passphrase", &graph_id));
    Some(SyncConfig {
        target,
        graph: graph_id,
        passphrase,
    })
}

/// Passphrase for a graph encrypted at rest: `TRELLIS_DB_PASSPHRASE`, else the macOS
/// keychain item (service `trellis-db`, account = graph id). Never stored in the graph.
pub fn db_passphrase(graph: &Path) -> Option<String> {
    std::env::var("TRELLIS_DB_PASSPHRASE")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| keychain_secret("trellis-db", &graph_id(graph)))
}

fn keychain_secret(service: &str, account: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .ok()?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !token.is_empty()).then_some(token)
}

pub fn remember_graph(config: &Path, graph: &Path) {
    let _ = std::fs::create_dir_all(config);
    let _ = std::fs::write(
        config.join("last_graph"),
        graph.to_string_lossy().as_bytes(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_names_are_safe() {
        assert_eq!(
            graph_file_name("My Notes").as_deref(),
            Some("My Notes.trellis")
        );
        assert_eq!(graph_file_name("a/b").as_deref(), Some("a-b.trellis"));
        assert_eq!(graph_file_name(" .. "), None);
    }

    #[test]
    fn discovery_prefers_last_then_single_file() {
        let cfg = tempfile::tempdir().unwrap();
        let graphs = tempfile::tempdir().unwrap();
        // SAFETY of test: TRELLIS_GRAPH is not set by the test runner.
        if std::env::var_os("TRELLIS_GRAPH").is_some() {
            return;
        }
        assert_eq!(resolve_graph(None, cfg.path(), graphs.path()), None);
        let one = graphs.path().join("one.trellis");
        std::fs::write(&one, b"").unwrap();
        assert_eq!(
            resolve_graph(None, cfg.path(), graphs.path()),
            Some(one.clone())
        );
        let two = graphs.path().join("two.trellis");
        std::fs::write(&two, b"").unwrap();
        assert_eq!(
            resolve_graph(None, cfg.path(), graphs.path()),
            None,
            "ambiguous"
        );
        remember_graph(cfg.path(), &two);
        assert_eq!(resolve_graph(None, cfg.path(), graphs.path()), Some(two));
        let explicit = PathBuf::from("/x.trellis");
        assert_eq!(
            resolve_graph(Some(explicit.clone()), cfg.path(), graphs.path()),
            Some(explicit)
        );
    }
}
