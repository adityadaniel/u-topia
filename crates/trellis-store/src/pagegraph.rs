//! The page graph for the graph view (P2): pages are nodes, resolved page references
//! between different pages are edges. Read straight from the `ref` index.

use std::collections::{HashMap, HashSet, VecDeque};

use trellis_core::ids::PageId;

use crate::{Result, Store};

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct GraphNode {
    pub page: PageId,
    pub title: String,
    pub daily: bool,
    /// Distinct linked pages, in either direction.
    pub degree: usize,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct PageGraph {
    pub nodes: Vec<GraphNode>,
    /// Undirected `(low, high)` node-index pairs with the number of references between
    /// them, sorted.
    pub edges: Vec<(usize, usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct GraphOptions {
    /// `Some(page)`: only pages within `depth` links of it (the local graph).
    pub center: Option<PageId>,
    pub depth: usize,
    pub include_daily: bool,
    /// Keep the best-connected pages when there are more (the center always stays).
    pub limit: usize,
}

impl Default for GraphOptions {
    fn default() -> Self {
        Self {
            center: None,
            depth: 2,
            include_daily: false,
            limit: 400,
        }
    }
}

impl Store {
    pub fn page_graph(&self, opts: &GraphOptions) -> Result<PageGraph> {
        self.read_barrier()?;
        let mut pages: HashMap<String, (String, bool)> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT id, title, kind FROM page WHERE deleted_at IS NULL")?;
            for row in stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)? == "daily",
                ))
            })? {
                let (id, title, daily) = row?;
                if daily && !opts.include_daily && opts.center.as_ref().is_none_or(|c| c.0 != id) {
                    continue;
                }
                pages.insert(id, (title, daily));
            }
        }
        // Undirected weights between kept pages.
        let mut weight: HashMap<(String, String), usize> = HashMap::new();
        {
            let mut stmt = self.conn.prepare_cached(
                "SELECT src_page_id, dst_page_id, count(*) FROM ref
                 WHERE dst_kind='page' AND dst_page_id IS NOT NULL AND dst_page_id <> src_page_id
                 GROUP BY src_page_id, dst_page_id",
            )?;
            for row in stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })? {
                let (a, b, n) = row?;
                if !pages.contains_key(&a) || !pages.contains_key(&b) {
                    continue;
                }
                let key = if a < b { (a, b) } else { (b, a) };
                *weight.entry(key).or_default() += n as usize;
            }
        }
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for (a, b) in weight.keys() {
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
        let degree = |id: &str| adj.get(id).map_or(0, Vec::len);
        let mut keep: Vec<String> = match &opts.center {
            Some(c) if pages.contains_key(&c.0) => {
                let mut seen: HashSet<&str> = HashSet::from([c.0.as_str()]);
                let mut q = VecDeque::from([(c.0.as_str(), 0usize)]);
                while let Some((id, d)) = q.pop_front() {
                    if d == opts.depth {
                        continue;
                    }
                    for &n in adj.get(id).into_iter().flatten() {
                        if seen.insert(n) {
                            q.push_back((n, d + 1));
                        }
                    }
                }
                seen.into_iter().map(str::to_string).collect()
            }
            Some(_) => Vec::new(),
            // The whole graph: pages with at least one link (orphans are noise).
            None => adj.keys().map(|s| s.to_string()).collect(),
        };
        keep.sort_by(|a, b| {
            let center = |x: &String| opts.center.as_ref().is_some_and(|c| &c.0 == x);
            center(b)
                .cmp(&center(a))
                .then(degree(b).cmp(&degree(a)))
                .then(pages[a].0.cmp(&pages[b].0))
        });
        keep.truncate(opts.limit);
        let index: HashMap<&str, usize> = keep
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let nodes = keep
            .iter()
            .map(|id| GraphNode {
                page: PageId(id.clone()),
                title: pages[id].0.clone(),
                daily: pages[id].1,
                degree: degree(id),
            })
            .collect();
        let mut edges: Vec<(usize, usize, usize)> = weight
            .iter()
            .filter_map(|((a, b), w)| {
                let (i, j) = (*index.get(a.as_str())?, *index.get(b.as_str())?);
                Some((i.min(j), i.max(j), *w))
            })
            .collect();
        edges.sort_unstable();
        Ok(PageGraph { nodes, edges })
    }
}
