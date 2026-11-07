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

/// What Settings → Sync can say about the transport in force (§12, ST-5): where it goes and
/// whether the payloads are encrypted. Built from [`SyncConfig`], with the bearer token left
/// out on purpose — secrets never reach the UI, `settings.json` or a screenshot (§15.6).
/// The app installs it once the config is resolved; with no global installed (tests, a
/// headless run) sync reads as off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncFacts {
    /// Human-readable transport, e.g. "Relay · https://relay.example" or "Folder · /…".
    pub target: Option<String>,
    /// The graph's id, which the relay uses to find its pages.
    pub graph: Option<String>,
    pub encrypted: bool,
}

impl gpui::Global for SyncFacts {}

impl SyncFacts {
    pub fn from_config(cfg: &SyncConfig) -> Self {
        let target = match &cfg.target {
            SyncTarget::Relay { url, .. } if url == "lan" => {
                "Relay · LAN, discovered with mDNS".to_string()
            }
            SyncTarget::Relay { url, .. } => format!("Relay · {url}"),
            SyncTarget::Folder(dir) => format!("Folder · {}", dir.display()),
        };
        Self {
            target: Some(target),
            graph: Some(cfg.graph.clone()),
            encrypted: cfg.passphrase.is_some(),
        }
    }

    pub fn is_on(&self) -> bool {
        self.target.is_some()
    }
}

/// The sync facts in force; sync off when the app has installed none.
pub fn facts(cx: &gpui::App) -> SyncFacts {
    cx.try_global::<SyncFacts>().cloned().unwrap_or_default()
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

/// Which half of a sync round is running, and therefore where the transport is (§12.3,
/// §13.3). The transport's home is [`SyncPhase::Idle`]; no phase ever means "no transport
/// exists". `interval` and `last` stay fields of [`SyncRunner`] on purpose: they are the
/// cadence and the history, not a phase, and both survive every transition.
pub enum SyncPhase {
    /// Between rounds: the transport is home and the next poll waits `interval`.
    Idle { transport: BoxTransport },
    /// The UI thread holds the transport and gathers pushes (short store reads, no network).
    Preparing,
    /// The background thread holds the transport and is talking to the relay.
    Exchanging,
    /// The exchange returned; the UI thread holds the transport and imports the result.
    Applying,
}

pub struct SyncRunner {
    pub engine: SyncEngine,
    /// The round's phase; the only place that says where the transport is.
    pub phase: SyncPhase,
    /// The last round's outcome; `None` = no round has finished yet ("connecting…").
    pub last: Option<SyncStatus>,
    /// Cadence between rounds: [`FAST`] while there is activity, backing off to [`SLOW`].
    pub interval: Duration,
    /// "Merged N changes" notice (§9.3) with the time it was raised.
    pub notice: Option<(String, i64)>,
    /// Raised by the realtime listener to sync now instead of at the next poll.
    pub nudge: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// This runner's identity. A loop may only drive the runner it was started for, so a
    /// superseded loop cannot write into its replacement (nothing else has this number).
    pub generation: u64,
    /// The loop driving this runner. Kept here so that replacing the runner drops (and so
    /// cancels) the previous loop: it is replaced, never accumulated (`_tasks` would only
    /// release it with the workspace).
    pub loop_task: Option<gpui::Task<()>>,
}

impl SyncRunner {
    pub fn new(engine: SyncEngine, transport: BoxTransport) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            engine,
            phase: SyncPhase::Idle { transport },
            last: None,
            interval: FAST,
            notice: None,
            nudge: Default::default(),
            generation: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            loop_task: None,
        }
    }

    /// Whether `generation` is this runner's token; false once sync was restarted.
    pub fn is_generation(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// Start a round: take the transport out of [`SyncPhase::Idle`]. `None` when a round is
    /// already running, which the loop treats as someone else's round and leaves alone.
    pub fn begin_round(&mut self) -> Option<BoxTransport> {
        if !matches!(self.phase, SyncPhase::Idle { .. }) {
            return None;
        }
        match std::mem::replace(&mut self.phase, SyncPhase::Exchanging) {
            SyncPhase::Idle { transport } => {
                self.phase = SyncPhase::Preparing;
                Some(transport)
            }
            // Not reachable: the guard above proved `Idle`. Kept panic-free (rule 11).
            other => {
                self.phase = other;
                None
            }
        }
    }

    /// The round's store work failed, so nothing left the device: the transport comes home.
    pub fn round_failed(&mut self, transport: BoxTransport, status: SyncStatus) {
        self.last = Some(status);
        self.phase = SyncPhase::Idle { transport };
    }

    /// The prepared pushes leave for the background thread.
    pub fn exchange(&mut self) {
        self.phase = SyncPhase::Exchanging;
    }

    /// The exchange returned; the UI thread imports it next.
    pub fn exchange_returned(&mut self) {
        self.phase = SyncPhase::Applying;
    }

    /// The round is over: remember what to show and how soon to poll again.
    pub fn end_round(
        &mut self,
        transport: BoxTransport,
        report: &Result<SyncReport, SyncError>,
        status: SyncStatus,
    ) {
        self.interval = next_interval(self.interval, report);
        self.last = Some(status);
        self.phase = SyncPhase::Idle { transport };
    }
}

/// Next poll interval after a round: fast again when anything moved, else back off.
fn next_interval(current: Duration, result: &Result<SyncReport, SyncError>) -> Duration {
    match result {
        Ok(r) if !r.is_quiet() => FAST,
        _ => (current * 2).min(SLOW),
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

/// What the sidebar's single sync dot says (SB-4): grey when sync is off, green when the
/// last round finished, amber while a round is in flight or still connecting, red when the
/// relay refused, was unreachable or speaks another protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncDot {
    Off,
    Synced,
    Syncing,
    Error,
}

impl SyncDot {
    /// Spelled into the element's debug selector, so a test can tell the states apart.
    pub fn key(self) -> &'static str {
        match self {
            SyncDot::Off => "off",
            SyncDot::Synced => "synced",
            SyncDot::Syncing => "syncing",
            SyncDot::Error => "error",
        }
    }
}

pub fn sync_dot(runner: Option<&SyncRunner>) -> SyncDot {
    let Some(runner) = runner else {
        return SyncDot::Off;
    };
    if !matches!(runner.phase, SyncPhase::Idle { .. }) {
        return SyncDot::Syncing;
    }
    match runner.last {
        None => SyncDot::Syncing,
        Some(SyncStatus::Synced { .. }) => SyncDot::Synced,
        Some(SyncStatus::Offline { .. })
        | Some(SyncStatus::LocalError { .. })
        | Some(SyncStatus::NeedsAuth)
        | Some(SyncStatus::Unsupported) => SyncDot::Error,
    }
}

/// Human status line for the sidebar, from the one state the dot reads.
pub fn status_line(runner: &SyncRunner, now_ms: i64) -> (String, bool) {
    status_text(runner.last.as_ref(), now_ms)
}

/// The line for one outcome.
pub fn status_text(status: Option<&SyncStatus>, now_ms: i64) -> (String, bool) {
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
        Some(SyncStatus::LocalError { message }) => {
            (format!("● Sync: local error — {message}"), true)
        }
        Some(SyncStatus::NeedsAuth) => ("● Sync: sign-in required".into(), true),
        Some(SyncStatus::Unsupported) => ("● Sync: relay protocol unsupported".into(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ST-5: Settings → Sync shows where the graph goes, and never the token or the
    /// passphrase that got it there (§15.6).
    #[test]
    fn sync_facts_never_carry_secrets() {
        let cfg = SyncConfig {
            target: SyncTarget::Relay {
                url: "https://relay.example".into(),
                token: Some("t0ken".into()),
            },
            graph: "graph-1".into(),
            passphrase: Some("p4ssphrase".into()),
        };
        let facts = SyncFacts::from_config(&cfg);
        assert_eq!(
            facts.target.as_deref(),
            Some("Relay · https://relay.example")
        );
        assert_eq!(facts.graph.as_deref(), Some("graph-1"));
        assert!(facts.encrypted);
        let shown = format!("{facts:?}");
        assert!(!shown.contains("t0ken"), "{shown}");
        assert!(!shown.contains("p4ssphrase"), "{shown}");

        let folder = SyncConfig {
            target: SyncTarget::Folder("/tmp/shared notes".into()),
            graph: "graph-2".into(),
            passphrase: None,
        };
        let facts = SyncFacts::from_config(&folder);
        assert_eq!(facts.target.as_deref(), Some("Folder · /tmp/shared notes"));
        assert!(!facts.encrypted);

        assert!(!SyncFacts::default().is_on(), "no config means sync is off");
    }

    #[test]
    fn the_dot_tells_off_synced_syncing_and_error_apart() {
        assert_eq!(sync_dot(None), SyncDot::Off);
        // A runner with a transport and no round yet is connecting.
        let engine = utopia_sync::SyncEngine::new(&utopia_store::Store::open_in_memory().unwrap())
            .expect("engine");
        let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
            utopia_sync::relay::Relay::new(),
            "g",
        ));
        let mut runner = SyncRunner::new(engine, transport);
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Syncing);
        runner.last = Some(SyncStatus::Synced { at_ms: 0 });
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Synced);
        runner.last = Some(SyncStatus::Offline { queued: 2 });
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Error);
        runner.last = Some(SyncStatus::LocalError {
            message: "database: disk I/O error".into(),
        });
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Error);
        // A round in flight wins over the last round's status.
        runner.last = Some(SyncStatus::Synced { at_ms: 0 });
        runner.begin_round().expect("the transport is home");
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Syncing);
    }

    #[test]
    fn status_lines() {
        assert_eq!(
            status_text(Some(&SyncStatus::Synced { at_ms: 0 }), 150_000).0,
            "Synced 2m ago"
        );
        let (s, warn) = status_text(Some(&SyncStatus::Offline { queued: 3 }), 0);
        assert_eq!(s, "● Offline — 3 changes queued");
        assert!(warn);
        // §12.6: a round that dies in the store is not the relay being offline.
        let local = SyncStatus::LocalError {
            message: "database: no such table: sync_state".into(),
        };
        let (s, warn) = status_text(Some(&local), 0);
        assert_eq!(s, "● Sync: local error — database: no such table: sync_state");
        assert!(warn);
    }

    /// Only one round at a time, and the transport always comes home: `begin_round` is the
    /// only way to take it, and it refuses while a round runs. This is what made the old
    /// "no transport at prepare" path unreachable (`sync-runner-round.md`, issue 7).
    #[test]
    fn one_round_at_a_time() {
        let engine = utopia_sync::SyncEngine::new(&utopia_store::Store::open_in_memory().unwrap())
            .expect("engine");
        let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
            utopia_sync::relay::Relay::new(),
            "g",
        ));
        let mut runner = SyncRunner::new(engine, transport);
        let transport = runner.begin_round().expect("idle round starts");
        assert!(
            runner.begin_round().is_none(),
            "a second round cannot take the transport"
        );
        runner.exchange();
        assert!(runner.begin_round().is_none(), "the exchange owns it");
        runner.exchange_returned();
        runner.end_round(
            transport,
            &Ok(SyncReport::default()),
            SyncStatus::Synced { at_ms: 0 },
        );
        assert!(runner.begin_round().is_some(), "the transport came home");
    }
}
