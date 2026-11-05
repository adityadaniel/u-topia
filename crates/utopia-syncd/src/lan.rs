//! LAN direct (§12.7 variant 4): a desktop hosts the relay on the local network and
//! advertises it over mDNS (`_utopia._tcp`), so a phone or laptop at home syncs with no
//! internet and no account. The transport is the same HTTP relay protocol (not QUIC), so
//! every client already speaks it.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::{router, Config, Db};

pub const SERVICE: &str = "_utopia._tcp.local.";

/// A relay running in-process on a background runtime. Dropping it stops serving.
pub struct LanRelay {
    pub addr: SocketAddr,
    _rt: tokio::runtime::Runtime,
    mdns: Option<ServiceDaemon>,
}

impl Drop for LanRelay {
    fn drop(&mut self) {
        if let Some(m) = self.mdns.take() {
            let _ = m.shutdown();
        }
    }
}

/// Serve the relay on `bind` (e.g. `0.0.0.0:8787`) backed by `db`, optionally advertising
/// it for `graph` over mDNS.
pub fn serve_lan(
    db: &Path,
    bind: &str,
    graph: &str,
    cfg: Config,
    advertise: bool,
) -> anyhow::Result<LanRelay> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("utopia-lan-relay")
        .build()?;
    let db = Db::open(db).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let listener = rt.block_on(tokio::net::TcpListener::bind(bind))?;
    let addr = listener.local_addr()?;
    let app = router(Arc::new(db), cfg);
    rt.spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let mdns = if advertise {
        let daemon = ServiceDaemon::new()?;
        let info = ServiceInfo::new(
            SERVICE,
            &format!("utopia-{graph}-{}", addr.port()),
            &format!("utopia-{}.local.", addr.port()),
            "",
            addr.port(),
            [("graph", graph)].as_slice(),
        )?
        .enable_addr_auto();
        daemon.register(info)?;
        Some(daemon)
    } else {
        None
    };
    Ok(LanRelay {
        addr,
        _rt: rt,
        mdns,
    })
}

/// Find a relay advertising `graph` on the LAN; returns its `http://ip:port` origin.
pub fn discover(graph: &str, timeout: Duration) -> Option<String> {
    let daemon = ServiceDaemon::new().ok()?;
    let rx = daemon.browse(SERVICE).ok()?;
    let deadline = Instant::now() + timeout;
    let mut found = None;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(r)) => {
                if r.get_property_val_str("graph") != Some(graph) {
                    continue;
                }
                if let Some(ip) = r.get_addresses_v4().into_iter().next() {
                    found = Some(format!("http://{ip}:{}", r.get_port()));
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    found
}
