//! Export (G4, §2.3 principle 8): Markdown (one file per page) and JSON. Both read the
//! shards, not the projections, so they work even if the index is damaged (§15.3).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use utopia_core::markdown::page_to_markdown;
use utopia_core::{BlockId, PageState};

use crate::project::display_title;
use crate::{Result, Store};

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() {
        "untitled".into()
    } else {
        s.chars().take(120).collect()
    }
}

fn block_json(st: &PageState, id: &BlockId) -> Value {
    let b = st.block(id).expect("listed by state");
    json!({
        "id": b.id,
        "text": b.text,
        "kind": b.kind.type_str(),
        "heading_level": b.kind.heading_level(),
        "checked": b.checked,
        "created_at": b.created_at,
        "updated_at": b.updated_at,
        "children": st
            .live_children(Some(id))
            .iter()
            .map(|c| block_json(st, c))
            .collect::<Vec<_>>(),
    })
}

/// What an export needs, copied off the store so the serialization and the file writes can
/// run on a background worker (rule 10): the live pages and where their attachments live.
#[derive(Debug, Clone, Default)]
pub struct ExportSnapshot {
    /// Display title and page state for every live page.
    pub pages: Vec<(String, PageState)>,
    /// The graph's blob directory, when it has one.
    pub blob_dir: Option<PathBuf>,
    /// Attachments held in memory (in-memory graphs), by hash.
    pub mem_blobs: HashMap<String, Vec<u8>>,
    /// Hashes of every attachment the pages reference.
    pub blobs: Vec<String>,
}

/// One `.md` file per page in the snapshot, attachments copied into `blobs/` beside them.
/// Returns the number of pages written. Touches no store, so it can run anywhere.
pub fn write_markdown(snapshot: &ExportSnapshot, dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    let mut used: HashSet<String> = HashSet::new();
    let mut n = 0;
    for (title, st) in &snapshot.pages {
        let base = match st.meta.daily_date {
            Some(d) => utopia_core::date::iso(d),
            None => sanitize(title),
        };
        let mut name = base.clone();
        let mut i = 2;
        while !used.insert(name.to_lowercase()) {
            name = format!("{base} ({i})");
            i += 1;
        }
        // Attachments are copied beside the pages and linked relatively, so the export
        // opens in any Markdown tool (G4).
        let md = page_to_markdown(st, title).replace("](blob:", "](blobs/");
        std::fs::write(dir.join(format!("{name}.md")), md)?;
        n += 1;
    }
    if !snapshot.blobs.is_empty() {
        std::fs::create_dir_all(dir.join("blobs"))?;
        for hash in &snapshot.blobs {
            let target = dir.join("blobs").join(hash);
            if let Some(bytes) = snapshot.mem_blobs.get(hash) {
                std::fs::write(target, bytes)?;
            } else if let Some(from) = snapshot.blob_dir.as_ref().map(|d| d.join(hash)) {
                if from.is_file() {
                    std::fs::copy(from, target)?;
                }
            }
        }
    }
    Ok(n)
}

/// The whole snapshot as JSON: pages with nested live blocks.
pub fn json_value(snapshot: &ExportSnapshot) -> Value {
    let pages: Vec<Value> = snapshot
        .pages
        .iter()
        .map(|(title, st)| {
            json!({
                "id": st.page_id,
                "title": title,
                "kind": st.meta.kind.as_str(),
                "daily_date": st.meta.daily_date.map(utopia_core::date::iso),
                "aliases": st.meta.aliases,
                "created_at": st.meta.created_at,
                "blocks": st.live_roots().iter().map(|r| block_json(st, r)).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "format": "utopia-export", "version": 1, "pages": pages })
}

pub fn write_json(snapshot: &ExportSnapshot, path: &Path) -> Result<()> {
    let v = json_value(snapshot);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    std::fs::write(path, serde_json::to_vec_pretty(&v)?)?;
    Ok(())
}

pub fn write_opml(snapshot: &ExportSnapshot, path: &Path) -> Result<()> {
    let refs: Vec<(String, &PageState)> =
        snapshot.pages.iter().map(|(t, s)| (t.clone(), s)).collect();
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    std::fs::write(path, utopia_core::markdown::pages_to_opml(&refs))?;
    Ok(())
}

/// Write what `utopia export <graph> <dir>` writes: Markdown pages (attachments in
/// `blobs/`), `graph.json` and `graph.opml`. Returns the number of pages.
pub fn write_export(snapshot: &ExportSnapshot, dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    let n = write_markdown(snapshot, dir)?;
    write_json(snapshot, &dir.join("graph.json"))?;
    write_opml(snapshot, &dir.join("graph.opml"))?;
    Ok(n)
}

impl Store {
    fn live_page_states(&mut self) -> Result<Vec<(String, PageState)>> {
        self.sync_index()?;
        let mut out = Vec::new();
        for row in self.list_pages(true)? {
            let fmt = self.date_format.clone();
            let st = self.page_state(&row.id)?.clone();
            out.push((display_title(&st, &fmt), st));
        }
        Ok(out)
    }

    /// Copy everything an export needs out of the store (rule 10): after this, serializing
    /// and writing the three formats touches no shard and can run on a background worker.
    pub fn export_snapshot(&mut self) -> Result<ExportSnapshot> {
        let pages = self.live_page_states()?;
        let blobs: Vec<String> = self.referenced_blobs()?.into_iter().collect();
        let blob_dir = self.blob_dir();
        let mem_blobs = if blob_dir.is_none() {
            blobs
                .iter()
                .filter_map(|h| self.mem_blobs.get(h).map(|b| (h.clone(), b.clone())))
                .collect()
        } else {
            HashMap::new()
        };
        Ok(ExportSnapshot {
            pages,
            blob_dir,
            mem_blobs,
            blobs,
        })
    }

    /// Write one `.md` file per live page into `dir`. Returns the number of files.
    pub fn export_markdown(&mut self, dir: &Path) -> Result<usize> {
        let snapshot = self.export_snapshot()?;
        write_markdown(&snapshot, dir)
    }

    /// The whole graph as JSON: pages with nested live blocks.
    pub fn export_json_value(&mut self) -> Result<Value> {
        let snapshot = self.export_snapshot()?;
        Ok(json_value(&snapshot))
    }

    pub fn export_json(&mut self, path: &Path) -> Result<()> {
        let snapshot = self.export_snapshot()?;
        write_json(&snapshot, path)
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn filenames_are_safe() {
        assert_eq!(sanitize("a/b:c"), "a-b-c");
        assert_eq!(sanitize("..hidden"), "hidden");
        assert_eq!(sanitize("   "), "untitled");
    }
}
