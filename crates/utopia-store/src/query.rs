//! Read queries over the projections: backlinks (§7), the reference inspector (FR-4.6),
//! block-ref status (FR-4.3, FR-4.7), quick jump (§9.1 Cmd+K), journal (FR-1.3) and
//! block search for `((` autocomplete (FR-4.2).

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use utopia_core::inline::plain_text;
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, BlockKind, PageId, PageKind};

use crate::{Result, Store, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageRow {
    pub id: PageId,
    pub kind: PageKind,
    pub title: String,
    pub title_key: String,
    pub daily_date: Option<NaiveDate>,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

impl PageRow {
    pub fn is_daily(&self) -> bool {
        self.kind == PageKind::Daily
    }

    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }
}

/// One reference edge into a page or block, with what breadcrumbs and ordering need.
struct HitRow {
    block: String,
    page: String,
    a0: i64,
    a1: i64,
    text: String,
    updated_at: i64,
    parent: Option<String>,
    position: String,
}

fn hit_row(r: &Row<'_>) -> rusqlite::Result<HitRow> {
    Ok(HitRow {
        block: r.get(0)?,
        page: r.get(1)?,
        a0: r.get(2)?,
        a1: r.get(3)?,
        text: r.get(4)?,
        updated_at: r.get(5)?,
        parent: r.get(6)?,
        position: r.get(7)?,
    })
}
/// `(src_block, dst_kind, dst_page, dst_block, dst_title_key)`.
type OutRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

const PAGE_COLS: &str =
    "id, kind, title, title_key, daily_date, created_at, updated_at, deleted_at";

fn page_row(r: &Row<'_>) -> rusqlite::Result<PageRow> {
    let kind: String = r.get(1)?;
    let date: Option<String> = r.get(4)?;
    Ok(PageRow {
        id: PageId(r.get(0)?),
        kind: if kind == "daily" {
            PageKind::Daily
        } else {
            PageKind::Page
        },
        title: r.get(2)?,
        title_key: r.get(3)?,
        daily_date: date.and_then(|d| utopia_core::date::parse_iso(&d)),
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        deleted_at: r.get(7)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockRow {
    pub id: BlockId,
    pub page_id: PageId,
    pub parent_id: Option<BlockId>,
    pub depth: i64,
    pub text: String,
    pub kind: BlockKind,
    pub checked: bool,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefSort {
    /// Daily pages newest first, then other pages by title (FR-3.3 default).
    #[default]
    Title,
    Recency,
}

#[derive(Debug, Clone, Default)]
pub struct RefQuery {
    pub sort: RefSort,
    /// Case-insensitive text filter within results (FR-3.3).
    pub text: Option<String>,
    /// Co-reference filters: keep only sources that also reference every listed page
    /// (FR-3.10). A source matches through the block, any ancestor, or its page.
    pub also: Vec<PageId>,
    /// Date filter (v1.1 "date filters"): keep sources dated on or after this day — the
    /// daily page's date, or the block's last edit for other pages.
    pub since: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefHit {
    pub block: BlockId,
    pub page: PageId,
    pub text: String,
    /// Ancestors of the referencing block, root first: `(id, plain text)`.
    pub breadcrumb: Vec<(BlockId, String)>,
    /// Byte ranges of the matching tokens in `text` (FR-4.9).
    pub anchors: Vec<(usize, usize)>,
    pub updated_at: i64,
    #[serde(skip)]
    order: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefGroup {
    pub page: PageRow,
    pub hits: Vec<RefHit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct LinkedRefs {
    /// Hits before filtering.
    pub total: usize,
    pub groups: Vec<RefGroup>,
}

impl LinkedRefs {
    pub fn shown(&self) -> usize {
        self.groups.iter().map(|g| g.hits.len()).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum BlockRefStatus {
    /// No block with that id exists anywhere (FR-4.7).
    Missing,
    /// The block exists as a tombstone (FR-3.7, AC-4.5).
    Deleted { page: PageId, text: String },
    Live {
        page: PageId,
        page_title: String,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RefTarget {
    Page { id: PageId, title: String },
    UnresolvedPage { key: String },
    Block { id: BlockId, status: BlockRefStatus },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutgoingRef {
    pub target: RefTarget,
    pub count: usize,
    /// One source block per target, for navigation.
    pub from_block: BlockId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TitleHit {
    pub page: PageId,
    pub title: String,
    /// Set when the match came through an alias.
    pub alias: Option<String>,
    pub daily: bool,
    pub score: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockHit {
    pub block: BlockId,
    pub page: PageId,
    pub page_title: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalEntry {
    pub page: PageRow,
    pub preview: Vec<String>,
}

/// Subsequence fuzzy score of `needle` in `hay` (both normalized); higher is better.
pub fn fuzzy_score(needle: &str, hay: &str) -> Option<i64> {
    if needle.is_empty() {
        return Some(1);
    }
    if hay == needle {
        return Some(10_000);
    }
    if hay.starts_with(needle) {
        return Some(5_000 - hay.len() as i64);
    }
    if let Some(i) = hay.find(needle) {
        let word_start = i == 0 || hay[..i].ends_with(' ');
        return Some(if word_start { 3_000 } else { 2_000 } - i as i64 - hay.len() as i64);
    }
    let mut score = 1_000i64;
    let mut it = hay.char_indices();
    let mut last: Option<usize> = None;
    for nc in needle.chars() {
        loop {
            let (i, hc) = it.next()?;
            if hc == nc {
                if let Some(l) = last {
                    score -= (i - l - 1) as i64;
                }
                last = Some(i);
                break;
            }
        }
    }
    Some(score - hay.len() as i64)
}

impl Store {
    pub fn page_row(&self, id: &PageId) -> Result<Option<PageRow>> {
        self.read_barrier()?;
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {PAGE_COLS} FROM page WHERE id=?1"),
                [id.as_str()],
                page_row,
            )
            .optional()?)
    }

    /// Resolve a title (or date, or alias) to a live page without creating anything.
    pub fn find_page(&self, title: &str) -> Result<Option<PageId>> {
        self.read_barrier()?;
        crate::resolve_page_key(&self.conn, &normalize_title(title), &self.date_format)
    }

    pub fn block_row(&self, id: &BlockId) -> Result<Option<BlockRow>> {
        self.read_barrier()?;
        Ok(self
            .conn
            .query_row(
                "SELECT id, page_id, parent_id, depth, text, block_type, heading_level, checked,
                        updated_at, deleted_at FROM block WHERE id=?1",
                [id.as_str()],
                |r| {
                    Ok(BlockRow {
                        id: BlockId(r.get(0)?),
                        page_id: PageId(r.get(1)?),
                        parent_id: r.get::<_, Option<String>>(2)?.map(BlockId),
                        depth: r.get(3)?,
                        text: r.get(4)?,
                        kind: BlockKind::from_parts(
                            &r.get::<_, String>(5)?,
                            r.get::<_, Option<i64>>(6)?.map(|l| l as u8),
                        ),
                        checked: r.get(7)?,
                        updated_at: r.get(8)?,
                        deleted_at: r.get(9)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn block_page(&self, id: &BlockId) -> Result<PageId> {
        self.read_barrier()?;
        self.block_row(id)?
            .map(|b| b.page_id)
            .ok_or_else(|| StoreError::NotFound(format!("block {id}")))
    }

    /// Live pages. Normal pages by title; daily pages newest first (FR-1.8).
    pub fn list_pages(&self, include_daily: bool) -> Result<Vec<PageRow>> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PAGE_COLS} FROM page WHERE deleted_at IS NULL
             ORDER BY kind='daily', CASE WHEN kind='daily' THEN '' ELSE title_key END,
                      daily_date DESC"
        ))?;
        let rows = stmt
            .query_map([], page_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter(|p| include_daily || !p.is_daily())
            .collect())
    }

    /// First `n` live top-level blocks of a page as plain text (previews, peek cards).
    pub fn page_preview(&self, page: &PageId, n: usize) -> Result<Vec<String>> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT text FROM block WHERE page_id=?1 AND parent_id IS NULL AND deleted_at IS NULL
             ORDER BY position LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![page.as_str(), n as i64], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.iter().map(|t| plain_text(t)).collect())
    }

    /// The last `limit` journal pages up to `today`, with a 2-line preview (FR-1.3).
    pub fn journal(&self, limit: usize) -> Result<Vec<JournalEntry>> {
        self.journal_pages(limit)?
            .into_iter()
            .map(|page| {
                let preview = self
                    .page_preview(&page.id, 2)?
                    .into_iter()
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                Ok(JournalEntry { page, preview })
            })
            .collect()
    }

    /// Daily pages up to today with something on them, newest first: the Daily Notes
    /// feed. A day counts once it has a non-empty block or a linked reference (a date
    /// pre-created by a link, FR-1.10); days that were only visited are skipped (FR-1.2).
    pub fn journal_pages(&self, limit: usize) -> Result<Vec<PageRow>> {
        self.read_barrier()?;
        let today = utopia_core::date::iso(self.today());
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {PAGE_COLS} FROM page WHERE kind='daily' AND deleted_at IS NULL
               AND daily_date <= ?1
               AND (EXISTS(SELECT 1 FROM block b WHERE b.page_id=page.id
                             AND b.deleted_at IS NULL AND trim(b.text) <> '')
                    OR EXISTS(SELECT 1 FROM ref r JOIN block b ON b.id=r.src_block_id
                               WHERE r.dst_page_id=page.id AND b.deleted_at IS NULL))
             ORDER BY daily_date DESC LIMIT ?2"
        ))?;
        let pages = stmt
            .query_map(params![today, limit as i64], page_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(pages)
    }

    /// Quick jump (Cmd+K): fuzzy match over live titles and aliases (v1: no bodies).
    pub fn search_titles(&self, query: &str, limit: usize) -> Result<Vec<TitleHit>> {
        self.read_barrier()?;
        let q = normalize_title(query);
        let mut best: HashMap<PageId, TitleHit> = HashMap::new();
        let mut consider = |hit: TitleHit| {
            let keep = best.get(&hit.page).is_none_or(|b| hit.score > b.score);
            if keep {
                best.insert(hit.page.clone(), hit);
            }
        };
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, title, title_key, kind FROM page WHERE deleted_at IS NULL")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (id, title, key, kind) = row?;
                if let Some(score) = fuzzy_score(&q, &key) {
                    consider(TitleHit {
                        page: PageId(id),
                        title,
                        alias: None,
                        daily: kind == "daily",
                        score,
                    });
                }
            }
        }
        {
            let mut stmt = self.conn.prepare(
                "SELECT a.page_id, p.title, a.alias, a.alias_key, p.kind FROM alias a
                 JOIN page p ON p.id=a.page_id WHERE p.deleted_at IS NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?;
            for row in rows {
                let (id, title, alias, key, kind) = row?;
                if let Some(score) = fuzzy_score(&q, &key) {
                    consider(TitleHit {
                        page: PageId(id),
                        title,
                        alias: Some(alias),
                        daily: kind == "daily",
                        score: score - 1,
                    });
                }
            }
        }
        let mut hits: Vec<TitleHit> = best.into_values().collect();
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(a.daily.cmp(&b.daily))
                .then(a.title.len().cmp(&b.title.len()))
                .then(a.title.cmp(&b.title))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    /// Block search for `((` autocomplete (FR-4.2): FTS5 prefix match over live bodies.
    pub fn search_blocks(&self, query: &str, limit: usize) -> Result<Vec<BlockHit>> {
        self.read_barrier()?;
        let terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| format!("\"{}\"*", t.replace('"', "")))
            .collect();
        let sql_recent =
            "SELECT b.id, b.page_id, p.title, b.text FROM block b JOIN page p ON p.id=b.page_id
             WHERE b.deleted_at IS NULL AND b.text <> '' ORDER BY b.updated_at DESC LIMIT ?1";
        let map = |r: &Row<'_>| {
            Ok(BlockHit {
                block: BlockId(r.get(0)?),
                page: PageId(r.get(1)?),
                page_title: r.get(2)?,
                text: r.get(3)?,
            })
        };
        if terms.is_empty() {
            let mut stmt = self.conn.prepare(sql_recent)?;
            let rows = stmt
                .query_map([limit as i64], map)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(rows);
        }
        // Newest matches first, straight off the FTS doclist (no scoring of every match,
        // which is what made common terms slow); then prefer blocks containing the whole
        // query as typed.
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.block_id, b.page_id, p.title, b.text FROM block_fts f
             JOIN block b ON b.rowid=f.rowid JOIN page p ON p.id=b.page_id
             WHERE block_fts MATCH ?1 ORDER BY f.rowid DESC LIMIT ?2",
        )?;
        let mut rows = stmt
            .query_map(params![terms.join(" "), (limit.max(1) * 10) as i64], map)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let needle = query.trim().to_lowercase();
        rows.sort_by_key(|h| !h.text.to_lowercase().contains(&needle));
        rows.truncate(limit);
        Ok(rows)
    }

    /// Status of a `((uid))` target, including tombstones (§10.3.1).
    pub fn block_ref_status(&self, uid: &str) -> Result<BlockRefStatus> {
        self.read_barrier()?;
        let row = self
            .conn
            .query_row(
                "SELECT b.page_id, b.text, b.deleted_at, p.title FROM block b JOIN page p ON p.id=b.page_id
                 WHERE b.id=?1",
                [uid],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(match row {
            None => BlockRefStatus::Missing,
            Some((page, text, Some(_), _)) => BlockRefStatus::Deleted {
                page: PageId(page),
                text,
            },
            Some((page, text, None, title)) => BlockRefStatus::Live {
                page: PageId(page),
                page_title: title,
                text,
            },
        })
    }

    /// Ancestors of `parent` (inclusive) as `(id, text, position)`, root first.
    fn ancestry(
        &self,
        cache: &mut HashMap<String, Vec<(String, String, String)>>,
        parent: &str,
    ) -> Result<Vec<(String, String, String)>> {
        if let Some(v) = cache.get(parent) {
            return Ok(v.clone());
        }
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE anc(id, parent_id, text, position, lvl) AS (
               SELECT id, parent_id, text, position, 0 FROM block WHERE id=?1
               UNION ALL
               SELECT b.id, b.parent_id, b.text, b.position, anc.lvl + 1
               FROM block b JOIN anc ON b.id = anc.parent_id WHERE anc.lvl < 256
             ) SELECT id, text, position FROM anc ORDER BY lvl DESC",
        )?;
        let rows = stmt
            .query_map([parent], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<(String, String, String)>>>()?;
        cache.insert(parent.to_string(), rows.clone());
        Ok(rows)
    }

    fn hits_from(&self, rows: Vec<HitRow>) -> Result<Vec<RefHit>> {
        let mut cache = HashMap::new();
        let mut by_block: Vec<RefHit> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for r in rows {
            if let Some(&i) = index.get(&r.block) {
                by_block[i].anchors.push((r.a0 as usize, r.a1 as usize));
                continue;
            }
            // Flat blocks (the common case) need no ancestor query at all.
            let anc = match &r.parent {
                Some(p) => self.ancestry(&mut cache, p)?,
                None => Vec::new(),
            };
            let order = anc
                .iter()
                .map(|(_, _, p)| p.as_str())
                .chain(std::iter::once(r.position.as_str()))
                .collect::<Vec<_>>()
                .join("/");
            index.insert(r.block.clone(), by_block.len());
            by_block.push(RefHit {
                block: BlockId(r.block),
                page: PageId(r.page),
                text: r.text,
                breadcrumb: anc
                    .into_iter()
                    .map(|(id, t, _)| (BlockId(id), plain_text(&t)))
                    .collect(),
                anchors: vec![(r.a0 as usize, r.a1 as usize)],
                updated_at: r.updated_at,
                order,
            });
        }
        Ok(by_block)
    }

    /// Linked References for a page (§7, FR-3.1–3.4, FR-3.10). Includes self-references.
    pub fn linked_references(&self, page: &PageId, q: &RefQuery) -> Result<LinkedRefs> {
        self.read_barrier()?;
        let rows: Vec<HitRow> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT r.src_block_id, r.src_page_id, r.anchor_start, r.anchor_end, b.text, b.updated_at,
                        b.parent_id, b.position
                 FROM ref r JOIN block b ON b.id=r.src_block_id
                 WHERE r.dst_page_id=?1 AND r.dst_kind='page' AND b.deleted_at IS NULL
                 ORDER BY r.src_block_id, r.anchor_start",
            )?;
            let rows = stmt
                .query_map([page.as_str()], hit_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let hits = self.hits_from(rows)?;
        let total = hits.len();
        let mut filters: Vec<(PageId, HashSet<String>)> = Vec::new();
        for f in &q.also {
            let mut stmt = self.conn.prepare_cached(
                "SELECT src_block_id FROM ref WHERE dst_page_id=?1 AND dst_kind='page'",
            )?;
            let set = stmt
                .query_map([f.as_str()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;
            filters.push((f.clone(), set));
        }
        let needle = q
            .text
            .as_ref()
            .map(|t| t.to_lowercase())
            .filter(|t| !t.is_empty());
        let mut titles: HashMap<PageId, PageRow> = HashMap::new();
        let mut groups: HashMap<PageId, Vec<RefHit>> = HashMap::new();
        for hit in hits {
            if !titles.contains_key(&hit.page) {
                if let Some(row) = self.page_row(&hit.page)? {
                    titles.insert(hit.page.clone(), row);
                }
            }
            let Some(src) = titles.get(&hit.page) else {
                continue;
            };
            let passes_filters = filters.iter().all(|(f, set)| {
                hit.page == *f
                    || set.contains(hit.block.as_str())
                    || hit.breadcrumb.iter().any(|(a, _)| set.contains(a.as_str()))
            });
            if !passes_filters {
                continue;
            }
            if let Some(since) = q.since {
                let day = src.daily_date.or_else(|| {
                    chrono::DateTime::from_timestamp_millis(hit.updated_at)
                        .map(|t| t.with_timezone(&chrono::Local).date_naive())
                });
                if day.is_none_or(|d| d < since) {
                    continue;
                }
            }
            if let Some(n) = &needle {
                let in_text = plain_text(&hit.text).to_lowercase().contains(n);
                let in_title = src.title.to_lowercase().contains(n);
                let in_crumbs = hit
                    .breadcrumb
                    .iter()
                    .any(|(_, t)| t.to_lowercase().contains(n));
                if !(in_text || in_title || in_crumbs) {
                    continue;
                }
            }
            groups.entry(hit.page.clone()).or_default().push(hit);
        }
        let mut out: Vec<RefGroup> = groups
            .into_iter()
            .map(|(p, mut hits)| {
                hits.sort_by(|a, b| a.order.cmp(&b.order));
                RefGroup {
                    page: titles[&p].clone(),
                    hits,
                }
            })
            .collect();
        match q.sort {
            RefSort::Title => out.sort_by(|a, b| {
                b.page
                    .is_daily()
                    .cmp(&a.page.is_daily())
                    .then(b.page.daily_date.cmp(&a.page.daily_date))
                    .then(a.page.title_key.cmp(&b.page.title_key))
            }),
            RefSort::Recency => out.sort_by(|a, b| {
                let ma = a.hits.iter().map(|h| h.updated_at).max();
                let mb = b.hits.iter().map(|h| h.updated_at).max();
                mb.cmp(&ma).then(a.page.title_key.cmp(&b.page.title_key))
            }),
        }
        Ok(LinkedRefs { total, groups: out })
    }

    /// Number of live blocks referencing a page.
    pub fn backlink_count(&self, page: &PageId) -> Result<usize> {
        self.read_barrier()?;
        Ok(self.conn.query_row(
            "SELECT count(DISTINCT r.src_block_id) FROM ref r JOIN block b ON b.id=r.src_block_id
             WHERE r.dst_page_id=?1 AND r.dst_kind='page' AND b.deleted_at IS NULL",
            [page.as_str()],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// "Referenced by (N)" for a block (FR-3.5).
    pub fn block_backlinks(&self, block: &BlockId) -> Result<Vec<RefHit>> {
        self.read_barrier()?;
        let rows: Vec<HitRow> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT r.src_block_id, r.src_page_id, r.anchor_start, r.anchor_end, b.text, b.updated_at,
                        b.parent_id, b.position
                 FROM ref r JOIN block b ON b.id=r.src_block_id
                 WHERE r.dst_block_id=?1 AND r.dst_kind='block' AND b.deleted_at IS NULL
                 ORDER BY r.src_page_id, r.src_block_id, r.anchor_start",
            )?;
            let rows = stmt
                .query_map([block.as_str()], hit_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        self.hits_from(rows)
    }

    /// Inspector "Outgoing" (FR-4.6): edges from a page's live blocks, or from one block.
    pub fn outgoing_refs(
        &self,
        page: &PageId,
        block: Option<&BlockId>,
    ) -> Result<Vec<OutgoingRef>> {
        self.read_barrier()?;
        let rows: Vec<OutRow> = {
            let (sql, arg) = match block {
                Some(b) => (
                    "SELECT r.src_block_id, r.dst_kind, r.dst_page_id, r.dst_block_id, r.dst_title_key
                     FROM ref r WHERE r.src_block_id=?1 ORDER BY r.anchor_start",
                    b.as_str(),
                ),
                None => (
                    "SELECT r.src_block_id, r.dst_kind, r.dst_page_id, r.dst_block_id, r.dst_title_key
                     FROM ref r JOIN block b ON b.id=r.src_block_id
                     WHERE r.src_page_id=?1 AND b.deleted_at IS NULL ORDER BY b.position, r.anchor_start",
                    page.as_str(),
                ),
            };
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt
                .query_map([arg], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let mut out: Vec<OutgoingRef> = Vec::new();
        for (src, kind, dst_page, dst_block, key) in rows {
            let target = if kind == "block" {
                let id = dst_block.unwrap_or_default();
                RefTarget::Block {
                    status: self.block_ref_status(&id)?,
                    id: BlockId(id),
                }
            } else {
                match dst_page {
                    Some(p) => {
                        let title = self
                            .page_row(&PageId(p.clone()))?
                            .map(|r| r.title)
                            .unwrap_or_default();
                        RefTarget::Page {
                            id: PageId(p),
                            title,
                        }
                    }
                    None => RefTarget::UnresolvedPage {
                        key: key.unwrap_or_default(),
                    },
                }
            };
            match out.iter_mut().find(|o| o.target == target) {
                Some(o) => o.count += 1,
                None => out.push(OutgoingRef {
                    target,
                    count: 1,
                    from_block: BlockId(src),
                }),
            }
        }
        Ok(out)
    }

    /// Inspector "Unresolved" (FR-4.6, FR-4.14): dangling page intents and missing
    /// block refs among a page's outgoing edges.
    pub fn unresolved_refs(&self, page: &PageId) -> Result<Vec<OutgoingRef>> {
        self.read_barrier()?;
        Ok(self
            .outgoing_refs(page, None)?
            .into_iter()
            .filter(|o| {
                matches!(
                    o.target,
                    RefTarget::UnresolvedPage { .. }
                        | RefTarget::Block {
                            status: BlockRefStatus::Missing,
                            ..
                        }
                )
            })
            .collect())
    }

    /// Aliases of a page with their display text (AC-3.3 tooltips).
    pub fn aliases(&self, page: &PageId) -> Result<Vec<String>> {
        self.read_barrier()?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT alias FROM alias WHERE page_id=?1 ORDER BY alias_key")?;
        let rows = stmt
            .query_map([page.as_str()], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
    }
}
