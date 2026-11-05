//! `utopia-syncd`: the self-hostable relay (§12.7 variant 2).
//!
//! Environment: `UTOPIA_SYNCD_ADDR` (default `127.0.0.1:8787`), `UTOPIA_SYNCD_DB`
//! (default `utopia-syncd.db`), `UTOPIA_SYNCD_TOKENS` (comma-separated bearer tokens;
//! unset = no auth, local use only), `UTOPIA_SYNCD_QUOTA_MB` (default 1024).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use utopia_syncd::{router, Config, Db};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = std::env::var("UTOPIA_SYNCD_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let db_path = PathBuf::from(
        std::env::var("UTOPIA_SYNCD_DB").unwrap_or_else(|_| "utopia-syncd.db".into()),
    );
    let tokens: Vec<String> = std::env::var("UTOPIA_SYNCD_TOKENS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let quota_mb: u64 = std::env::var("UTOPIA_SYNCD_QUOTA_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);
    if tokens.is_empty() {
        eprintln!(
            "utopia-syncd: UTOPIA_SYNCD_TOKENS is not set; accepting unauthenticated requests"
        );
    }
    let db =
        Db::open(&db_path).map_err(|e| anyhow::anyhow!("opening {}: {e:?}", db_path.display()))?;
    let app = router(
        Arc::new(db),
        Config {
            tokens,
            quota_bytes: quota_mb << 20,
        },
    );
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    eprintln!("utopia-syncd listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
