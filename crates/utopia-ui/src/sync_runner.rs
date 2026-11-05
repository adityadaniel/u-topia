//! Background sync for the desktop app (§12.3, §13.3): store work (prepare/apply) runs on
//! the UI thread in short steps; network exchange runs on a background thread. Polls every
//! 5 s while there is activity, backing off to 60 s when idle or failing.

use std::time::Duration;

use utopia_sync::{Incoming, Outgoing, SyncEngine, SyncError, SyncReport, SyncStatus, Transport};

pub const FAST: Duration = Duration::from_secs(5);
pub const SLOW: Duration = Duration::from_secs(60);

/// Where sync goes (§12.7 transports).
#[derive(Clone, Debug)]
pub enum SyncTarget {
    /// Hosted or self-hosted relay (`utopia-syncd`), with an optional bearer token.
    Relay { url: String, token: Option<String> },
    /// A shared folder (iCloud Drive, Dropbox, Syncthing): no server.
    Folder(std::path::PathBuf),
}

/// Where and how to sync; resolved by the app from env/keychain, never stored in the graph.
#[derive(Clone, Debug)]
pub struct SyncConfig {
    pub target: SyncTarget,
    pub graph: String,
    /// End-to-end encryption passphrase (§11.7); `None` = plaintext payloads.
    pub passphrase: Option<String>,
}

pub type BoxTransport = Box<dyn Transport + Send>;

/// Build the transport stack for a config: relay or folder, optionally encrypted.
pub fn build_transport(cfg: &SyncConfig, client_id: &str) -> Result<BoxTransport, SyncError> {
    let inner: BoxTransport = match &cfg.target {
        SyncTarget::Relay { url, token } => Box::new(utopia_sync::http::HttpTransport::new(
            url,
            &cfg.graph,
            token.clone(),
        )?),
        SyncTarget::Folder(dir) => {
            Box::new(utopia_sync::folder::FolderTransport::new(dir, client_id)?)
        }
    };
    Ok(match &cfg.passphrase {
        Some(p) => {
            let key = utopia_sync::e2e::GraphKey::derive(p, &cfg.graph)?;
            Box::new(utopia_sync::e2e::Encrypted::new(
                inner,
                key,
                cfg.graph.clone(),
            ))
        }
        None => inner,
    })
}

/// Keep a realtime connection to the relay (§12.3) on a background thread; every notice
/// raises `nudge`, which wakes the sync loop at once. Reconnects with backoff.
pub fn spawn_realtime(
    url: String,
    graph: String,
    token: Option<String>,
    nudge: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let _ = std::thread::Builder::new()
        .name("utopia-realtime".into())
        .spawn(move || {
            let mut backoff = Duration::from_secs(2);
            loop {
                let r = utopia_sync::realtime::listen(&url, &graph, token.as_deref(), |_| {
                    nudge.store(true, std::sync::atomic::Ordering::SeqCst);
                    true
                });
                if matches!(r, Err(SyncError::Unauthorized)) {
                    return;
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(SLOW);
            }
        });
}

pub struct SyncRunner {
    pub engine: SyncEngine,
    /// `None` while an exchange is in flight on the background thread.
    pub transport: Option<BoxTransport>,
    pub status: Option<SyncStatus>,
    pub interval: Duration,
    /// "Merged N changes" notice (§9.3) with the time it was raised.
    pub notice: Option<(String, i64)>,
    /// Raised by the realtime listener to sync now instead of at the next poll.
    pub nudge: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SyncRunner {
    pub fn new(engine: SyncEngine, transport: BoxTransport) -> Self {
        Self {
            engine,
            transport: Some(transport),
            status: None,
            interval: FAST,
            notice: None,
            nudge: Default::default(),
        }
    }

    /// Next poll interval after a round.
    pub fn next_interval(&self, result: &Result<SyncReport, SyncError>) -> Duration {
        match result {
            Ok(r) if !r.is_quiet() => FAST,
            _ => (self.interval * 2).min(SLOW),
        }
    }
}

/// The background half of a round.
pub fn exchange_owned(
    mut t: BoxTransport,
    out: Outgoing,
) -> (Result<Incoming, SyncError>, BoxTransport) {
    let r = utopia_sync::exchange(t.as_mut(), out);
    (r, t)
}

/// Human status line for the sidebar.
pub fn status_line(status: Option<&SyncStatus>, now_ms: i64) -> (String, bool) {
    match status {
        None => ("Sync: connecting…".into(), false),
        Some(SyncStatus::Synced { at_ms }) => {
            let secs = ((now_ms - at_ms) / 1000).max(0);
            let ago = if secs < 60 {
                "just now".to_string()
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else {
                format!("{}h ago", secs / 3600)
            };
            (format!("Synced {ago}"), false)
        }
        Some(SyncStatus::Offline { queued }) => {
            (format!("● Offline — {queued} changes queued"), true)
        }
        Some(SyncStatus::NeedsAuth) => ("● Sync: sign-in required".into(), true),
        Some(SyncStatus::Unsupported) => ("● Sync: relay protocol unsupported".into(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines() {
        assert_eq!(
            status_line(Some(&SyncStatus::Synced { at_ms: 0 }), 150_000).0,
            "Synced 2m ago"
        );
        let (s, warn) = status_line(Some(&SyncStatus::Offline { queued: 3 }), 0);
        assert_eq!(s, "● Offline — 3 changes queued");
        assert!(warn);
    }
}
