//! Realtime invalidation (§12.3 "Realtime"): a WebSocket that only carries
//! `{"page_id", "head_seq"}` notices. A notice triggers the ordinary HTTP pull, so the
//! relay stays stateless-ish and correctness never depends on the socket.

use serde::{Deserialize, Serialize};
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;

use crate::{Result, SyncError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub page_id: String,
    pub head_seq: u64,
}

/// `http(s)://host` → `ws(s)://host/v1/graphs/{gid}/events`.
pub fn events_url(base: &str, graph: &str) -> String {
    let base = base.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws}/v1/graphs/{graph}/events")
}

/// Block on the event stream, calling `on_notice` per notice until it returns `false` or
/// the connection ends. Run it on a background thread and reconnect with backoff.
pub fn listen(
    base: &str,
    graph: &str,
    token: Option<&str>,
    mut on_notice: impl FnMut(Notice) -> bool,
) -> Result<()> {
    let mut req = events_url(base, graph)
        .into_client_request()
        .map_err(|e| SyncError::Transport(e.to_string()))?;
    if let Some(t) = token {
        let v = format!("Bearer {t}")
            .parse()
            .map_err(|_| SyncError::Transport("bad token".into()))?;
        req.headers_mut().insert("Authorization", v);
    }
    let (mut socket, _) = tungstenite::connect(req).map_err(|e| match e {
        tungstenite::Error::Http(r) if r.status() == 401 || r.status() == 403 => {
            SyncError::Unauthorized
        }
        e => SyncError::Transport(e.to_string()),
    })?;
    loop {
        let msg = socket
            .read()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        match msg {
            Message::Text(_) | Message::Binary(_) => {
                let text = msg.to_text().unwrap_or_default();
                if let Ok(n) = serde_json::from_str::<Notice>(text) {
                    if !on_notice(n) {
                        let _ = socket.close(None);
                        return Ok(());
                    }
                }
            }
            Message::Close(_) => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            events_url("https://s.example/", "g"),
            "wss://s.example/v1/graphs/g/events"
        );
        assert_eq!(
            events_url("http://127.0.0.1:1", "g"),
            "ws://127.0.0.1:1/v1/graphs/g/events"
        );
    }
}
