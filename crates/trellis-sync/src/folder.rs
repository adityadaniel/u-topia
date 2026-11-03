//! Folder transport (§12.7 variant 3): a shared folder (iCloud Drive, Dropbox, Syncthing,
//! WebDAV mount) holding `pages/<page>/<client>/<seq>.upd`. Files are only ever created,
//! never overwritten, so file-sync tools cannot destroy data. No server, no account.
//!
//! The folder has no global order, so a page's `seq` is its file count and a pull returns
//! every file of the page; imports are idempotent, so re-reading is safe. Snapshot
//! compaction is skipped here: other devices' files are never deleted.

use std::path::{Path, PathBuf};

use trellis_core::PageId;

use crate::wire::{Manifest, ManifestPage, PullResponse, PushRequest, PushResponse, UpdateOut};
use crate::{Result, SyncError, Transport, PROTOCOL_VERSION};

pub struct FolderTransport {
    root: PathBuf,
    client: String,
}

fn io(e: std::io::Error) -> SyncError {
    SyncError::Transport(format!("folder: {e}"))
}

fn safe(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl FolderTransport {
    /// `client` names this device's subfolder (the sync client id is a good choice).
    pub fn new(root: impl Into<PathBuf>, client: &str) -> Result<Self> {
        if !safe(client) {
            return Err(SyncError::Transport(format!(
                "invalid client id {client:?}"
            )));
        }
        let root = root.into();
        std::fs::create_dir_all(root.join("pages")).map_err(io)?;
        Ok(Self {
            root,
            client: client.to_string(),
        })
    }

    fn page_dir(&self, page: &PageId) -> Result<PathBuf> {
        if !safe(page.as_str()) {
            return Err(SyncError::Transport(format!("invalid page id {page}")));
        }
        Ok(self.root.join("pages").join(page.as_str()))
    }

    /// Every update file of a page, sorted (client, seq).
    fn files(dir: &Path) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        let Ok(clients) = std::fs::read_dir(dir) else {
            return Ok(out);
        };
        for c in clients.flatten() {
            let Ok(files) = std::fs::read_dir(c.path()) else {
                continue;
            };
            for f in files.flatten() {
                let p = f.path();
                if p.extension().is_some_and(|e| e == "upd") {
                    out.push(p);
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

impl Transport for FolderTransport {
    fn manifest(&mut self, if_none_match: Option<u64>) -> Result<Option<Manifest>> {
        let mut pages = Vec::new();
        let mut rev: u64 = 0;
        for entry in std::fs::read_dir(self.root.join("pages"))
            .map_err(io)?
            .flatten()
        {
            let id = entry.file_name().to_string_lossy().to_string();
            if !safe(&id) {
                continue;
            }
            let n = Self::files(&entry.path())?.len() as u64;
            rev = rev
                .wrapping_mul(31)
                .wrapping_add(n)
                .wrapping_add(id.len() as u64);
            pages.push(ManifestPage {
                page_id: id,
                head_seq: n,
                frontier: String::new(),
                snapshot_seq: n,
                tail_bytes: 0,
                updated_at: 0,
                deleted: false,
            });
        }
        if Some(rev) == if_none_match {
            return Ok(None);
        }
        Ok(Some(Manifest {
            rev,
            protocol_version: PROTOCOL_VERSION,
            pages,
        }))
    }

    fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse> {
        let dir = self.page_dir(page)?.join(&self.client);
        std::fs::create_dir_all(&dir).map_err(io)?;
        let (mut accepted, mut duplicate) = (0, 0);
        for u in req.updates {
            let path = dir.join(format!("{:012}.upd", u.client_seq.max(0)));
            if path.exists() {
                duplicate += 1;
                continue;
            }
            // Write then rename: readers never see a partial file.
            let tmp = dir.join(format!(".{:012}.tmp", u.client_seq.max(0)));
            std::fs::write(&tmp, &u.payload).map_err(io)?;
            std::fs::rename(&tmp, &path).map_err(io)?;
            accepted += 1;
        }
        let head = Self::files(&self.page_dir(page)?)?.len() as u64;
        Ok(PushResponse {
            accepted,
            head_seq: head,
            duplicate,
        })
    }

    fn pull(&mut self, page: &PageId, _since_seq: u64) -> Result<PullResponse> {
        let files = Self::files(&self.page_dir(page)?)?;
        let mut updates = Vec::with_capacity(files.len());
        for (i, f) in files.iter().enumerate() {
            updates.push(UpdateOut {
                seq: i as u64 + 1,
                id: f.to_string_lossy().to_string(),
                frontier: String::new(),
                payload: std::fs::read(f).map_err(io)?,
            });
        }
        Ok(PullResponse {
            snapshot: None,
            head_seq: updates.len() as u64,
            updates,
        })
    }

    fn put_snapshot(&mut self, _page: &PageId, _covers_seq: u64, _payload: Vec<u8>) -> Result<()> {
        Ok(())
    }

    /// `blobs/<sha256>`: written once, via a temporary name, never overwritten.
    fn put_blob(&mut self, hash: &str, bytes: Vec<u8>) -> Result<bool> {
        if !trellis_core::attach::is_hash(hash) {
            return Err(SyncError::Transport(format!("invalid blob hash {hash:?}")));
        }
        let dir = self.root.join("blobs");
        std::fs::create_dir_all(&dir).map_err(io)?;
        let path = dir.join(hash);
        if !path.exists() {
            let tmp = dir.join(format!(".{hash}.{}.tmp", self.client));
            std::fs::write(&tmp, &bytes).map_err(io)?;
            std::fs::rename(&tmp, &path).map_err(io)?;
        }
        Ok(true)
    }

    fn get_blob(&mut self, hash: &str) -> Result<Option<Vec<u8>>> {
        if !trellis_core::attach::is_hash(hash) {
            return Ok(None);
        }
        match std::fs::read(self.root.join("blobs").join(hash)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }
}
