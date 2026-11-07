//! End-to-end encryption (§11.7): a 256-bit graph key derived from a passphrase with
//! Argon2id; every update and snapshot payload is sealed with XChaCha20-Poly1305 under a
//! fresh random nonce, with AAD = graph id ‖ page id. The relay stores ciphertext only;
//! compaction stays client-driven. Wraps any [`Transport`].

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use utopia_core::PageId;

use crate::wire::{Manifest, PullResponse, PushRequest, PushResponse};
use crate::{CursorSpace, Result, SyncError, Transport};

/// Format tag so a future scheme can coexist (§12.6: never corrupt the log).
const VERSION: u8 = 1;
const NONCE: usize = 24;

#[derive(Clone)]
pub struct GraphKey([u8; 32]);

impl GraphKey {
    /// Argon2id over the passphrase; the salt binds the key to the graph id, so every
    /// device with the passphrase derives the same key.
    pub fn derive(passphrase: &str, graph: &str) -> Result<Self> {
        let salt = format!("utopia-e2e-v1:{graph}");
        let mut key = [0u8; 32];
        Argon2::default()
            .hash_password_into(passphrase.as_bytes(), salt.as_bytes(), &mut key)
            .map_err(|e| SyncError::Transport(format!("key derivation: {e}")))?;
        Ok(Self(key))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.0).into())
    }

    fn aad(graph: &str, page: &PageId) -> Vec<u8> {
        let mut v = graph.as_bytes().to_vec();
        v.push(0);
        v.extend_from_slice(page.as_str().as_bytes());
        v
    }

    pub fn seal(&self, graph: &str, page: &PageId, plain: &[u8]) -> Result<Vec<u8>> {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let aad = Self::aad(graph, page);
        let ct = self
            .cipher()
            .encrypt(
                &nonce,
                Payload {
                    msg: plain,
                    aad: &aad,
                },
            )
            .map_err(|_| SyncError::Transport("encryption failed".into()))?;
        let mut out = Vec::with_capacity(1 + NONCE + ct.len());
        out.push(VERSION);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn open(&self, graph: &str, page: &PageId, sealed: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < 1 + NONCE || sealed[0] != VERSION {
            return Err(SyncError::Transport(
                "not an encrypted Utopia payload".into(),
            ));
        }
        let nonce = XNonce::from_slice(&sealed[1..1 + NONCE]);
        let aad = Self::aad(graph, page);
        self.cipher()
            .decrypt(
                nonce,
                Payload {
                    msg: &sealed[1 + NONCE..],
                    aad: &aad,
                },
            )
            .map_err(|_| SyncError::Transport("decryption failed (wrong passphrase?)".into()))
    }
}

/// A transport that encrypts on the way out and decrypts on the way in.
pub struct Encrypted<T> {
    inner: T,
    key: GraphKey,
    graph: String,
}

impl<T: Transport> Encrypted<T> {
    pub fn new(inner: T, key: GraphKey, graph: impl Into<String>) -> Self {
        Self {
            inner,
            key,
            graph: graph.into(),
        }
    }

    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: Transport> Transport for Encrypted<T> {
    fn cursor_space(&self) -> CursorSpace {
        // Sealing changes payloads, not the relay's sequence.
        self.inner.cursor_space()
    }

    fn manifest(&mut self, if_none_match: Option<u64>) -> Result<Option<Manifest>> {
        self.inner.manifest(if_none_match)
    }

    fn push(&mut self, page: &PageId, mut req: PushRequest) -> Result<PushResponse> {
        for u in &mut req.updates {
            u.payload = self.key.seal(&self.graph, page, &u.payload)?;
        }
        self.inner.push(page, req)
    }

    fn pull(&mut self, page: &PageId, since_seq: u64) -> Result<PullResponse> {
        let mut resp = self.inner.pull(page, since_seq)?;
        if let Some(s) = &resp.snapshot {
            resp.snapshot = Some(self.key.open(&self.graph, page, s)?);
        }
        for u in &mut resp.updates {
            u.payload = self.key.open(&self.graph, page, &u.payload)?;
        }
        Ok(resp)
    }

    fn put_snapshot(&mut self, page: &PageId, covers_seq: u64, payload: Vec<u8>) -> Result<()> {
        let sealed = self.key.seal(&self.graph, page, &payload)?;
        self.inner.put_snapshot(page, covers_seq, sealed)
    }

    // Attachments are sealed like pages, with `blob:<hash>` in the AAD so a ciphertext
    // cannot be served under another hash. The relay still sees the plaintext hash (it
    // names the object), which reveals only that two uploads are the same file.
    fn put_blob(&mut self, hash: &str, bytes: Vec<u8>) -> Result<bool> {
        let sealed = self.key.seal(&self.graph, &blob_id(hash), &bytes)?;
        self.inner.put_blob(hash, sealed)
    }

    fn get_blob(&mut self, hash: &str) -> Result<Option<Vec<u8>>> {
        match self.inner.get_blob(hash)? {
            Some(sealed) => Ok(Some(self.key.open(&self.graph, &blob_id(hash), &sealed)?)),
            None => Ok(None),
        }
    }
}

fn blob_id(hash: &str) -> PageId {
    PageId(format!("blob:{hash}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_and_tamper() {
        let k = GraphKey::derive("correct horse", "g1").unwrap();
        let page = PageId::from("p1");
        let sealed = k.seal("g1", &page, b"hello").unwrap();
        assert_ne!(&sealed[1 + NONCE..], b"hello");
        assert_eq!(k.open("g1", &page, &sealed).unwrap(), b"hello");
        // Bound to graph and page (AAD), and to the key.
        assert!(k.open("g1", &PageId::from("p2"), &sealed).is_err());
        assert!(k.open("g2", &page, &sealed).is_err());
        let wrong = GraphKey::derive("wrong", "g1").unwrap();
        assert!(wrong.open("g1", &page, &sealed).is_err());
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(k.open("g1", &page, &bad).is_err());
        // Fresh nonce each time.
        assert_ne!(k.seal("g1", &page, b"hello").unwrap(), sealed);
    }
}
