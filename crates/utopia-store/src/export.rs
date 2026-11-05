//! Export (G4, §2.3 principle 8): Markdown (one file per page) and JSON. Both read the
//! shards, not the projections, so they work even if the index is damaged (§15.3).

use std::collections::HashSet;
use std::path::Path;

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

    /// Write one `.md` file per live page into `dir`. Returns the number of files.
    pub fn export_markdown(&mut self, dir: &Path) -> Result<usize> {
        std::fs::create_dir_all(dir)?;
        let mut used: HashSet<String> = HashSet::new();
        let mut n = 0;
        for (title, st) in self.live_page_states()? {
            let base = match st.meta.daily_date {
                Some(d) => utopia_core::date::iso(d),
                None => sanitize(&title),
            };
            let mut name = base.clone();
            let mut i = 2;
            while !used.insert(name.to_lowercase()) {
                name = format!("{base} ({i})");
                i += 1;
            }
            // Attachments are copied beside the pages and linked relatively, so the
            // export opens in any Markdown tool (G4).
            let md = page_to_markdown(&st, &title).replace("](blob:", "](blobs/");
            std::fs::write(dir.join(format!("{name}.md")), md)?;
            n += 1;
        }
        let referenced = self.referenced_blobs()?;
        if !referenced.is_empty() {
            std::fs::create_dir_all(dir.join("blobs"))?;
            for hash in referenced {
                if let Some(bytes) = self.blob(&hash)? {
                    std::fs::write(dir.join("blobs").join(&hash), bytes)?;
                }
            }
        }
        Ok(n)
    }

    /// The whole graph as JSON: pages with nested live blocks.
    pub fn export_json_value(&mut self) -> Result<Value> {
        let pages: Vec<Value> = self
            .live_page_states()?
            .into_iter()
            .map(|(title, st)| {
                json!({
                    "id": st.page_id,
                    "title": title,
                    "kind": st.meta.kind.as_str(),
                    "daily_date": st.meta.daily_date.map(utopia_core::date::iso),
                    "aliases": st.meta.aliases,
                    "created_at": st.meta.created_at,
                    "blocks": st.live_roots().iter().map(|r| block_json(&st, r)).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({ "format": "utopia-export", "version": 1, "pages": pages }))
    }

    pub fn export_json(&mut self, path: &Path) -> Result<()> {
        let v = self.export_json_value()?;
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        std::fs::write(path, serde_json::to_vec_pretty(&v)?)?;
        Ok(())
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
