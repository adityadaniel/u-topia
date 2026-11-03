//! Query blocks (P2): evaluate a [`trellis_core::query::Expr`] against the index.
//!
//! Every term is a set of live block ids computed with one indexed SQL statement; `and`,
//! `or` and `not` are set algebra. Page terms are inherited like Roam's: a block matches
//! `[[X]]` when it, an ancestor, or its page references (or is) X.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection};
use trellis_core::{
    ids::{BlockId, PageId},
    query::Expr,
    title::normalize_title,
};

use crate::{project::resolve_page_key, Result, Store};

/// One matching block, topmost per subtree: a block whose parent also matched is shown
/// under that parent and not listed again.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct QueryHit {
    pub block: BlockId,
    pub page: PageId,
    pub page_title: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct QueryResult {
    pub hits: Vec<QueryHit>,
    /// Matches before `limit`.
    pub total: usize,
}

type Set = HashSet<String>;

/// A block that is itself a query never appears in results (it references its terms).
fn is_query_text(text: &str) -> bool {
    trellis_core::query::query_block(text).is_some()
}

impl Store {
    /// Run a query. Results are grouped by page (journal pages newest first, then other
    /// pages by title) and ordered by outline position within a page.
    pub fn run_query(&self, expr: &Expr, limit: usize) -> Result<QueryResult> {
        self.read_barrier()?;
        let set = eval(&self.conn, self.date_format(), expr)?;
        let ids: Vec<String> = set.into_iter().collect();
        let rows = fetch(&self.conn, &ids)?;
        let matched: HashSet<&str> = rows.keys().map(String::as_str).collect();
        let mut hits: Vec<(SortKey, QueryHit)> = rows
            .iter()
            .filter(|(_, r)| !is_query_text(&r.text))
            .filter(|(_, r)| r.parent.as_deref().is_none_or(|p| !matched.contains(p)))
            .map(|(id, r)| {
                (
                    (
                        r.daily.is_none(),
                        std::cmp::Reverse(r.daily.clone().unwrap_or_default()),
                        normalize_title(&r.page_title),
                        r.page.clone(),
                        r.path.clone(),
                    ),
                    QueryHit {
                        block: BlockId(id.clone()),
                        page: PageId(r.page.clone()),
                        page_title: r.page_title.clone(),
                        text: r.text.clone(),
                    },
                )
            })
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0));
        let total = hits.len();
        Ok(QueryResult {
            hits: hits.into_iter().take(limit).map(|(_, h)| h).collect(),
            total,
        })
    }
}

type SortKey = (bool, std::cmp::Reverse<String>, String, String, String);

struct Row {
    page: String,
    page_title: String,
    daily: Option<String>,
    parent: Option<String>,
    text: String,
    /// Positions from the root down, for outline order across depths.
    path: String,
}

/// Live blocks on live pages, with their outline path.
fn fetch(conn: &Connection, ids: &[String]) -> Result<HashMap<String, Row>> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "WITH RECURSIVE anc(id, cur, path) AS (
               SELECT b.id, b.parent_id, b.position FROM block b WHERE b.id IN ({marks})
               UNION ALL
               SELECT anc.id, p.parent_id, p.position || '/' || anc.path
               FROM anc JOIN block p ON p.id = anc.cur
             )
             SELECT b.id, b.page_id, pg.title, pg.daily_date, b.parent_id, b.text, anc.path
             FROM anc
             JOIN block b ON b.id = anc.id
             JOIN page pg ON pg.id = b.page_id
             WHERE anc.cur IS NULL AND b.deleted_at IS NULL AND pg.deleted_at IS NULL"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                Row {
                    page: r.get(1)?,
                    page_title: r.get(2)?,
                    daily: r.get(3)?,
                    parent: r.get(4)?,
                    text: r.get(5)?,
                    path: r.get(6)?,
                },
            ))
        })?;
        for row in rows {
            let (id, row) = row?;
            out.insert(id, row);
        }
    }
    Ok(out)
}

fn strings(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Set> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(p, |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Set>>()?)
}

/// Blocks referencing any of `pages` (by id or unresolved title key), on those pages,
/// and their descendants.
fn inherited(conn: &Connection, keys: &[String], pages: &[String]) -> Result<Set> {
    let mut out = Set::new();
    for key in keys {
        out.extend(strings(
            conn,
            "SELECT src_block_id FROM ref WHERE dst_kind='page' AND dst_title_key=?1",
            [key],
        )?);
    }
    for page in pages {
        out.extend(strings(
            conn,
            "SELECT src_block_id FROM ref WHERE dst_kind='page' AND dst_page_id=?1",
            [page],
        )?);
        out.extend(strings(
            conn,
            "SELECT id FROM block WHERE page_id=?1 AND deleted_at IS NULL",
            [page],
        )?);
    }
    descendants(conn, out)
}

fn descendants(conn: &Connection, seeds: Set) -> Result<Set> {
    let mut all = seeds.clone();
    let mut frontier: Vec<String> = seeds.into_iter().collect();
    let mut stmt =
        conn.prepare_cached("SELECT id FROM block WHERE parent_id=?1 AND deleted_at IS NULL")?;
    while let Some(id) = frontier.pop() {
        for child in stmt.query_map([&id], |r| r.get::<_, String>(0))? {
            let child = child?;
            if all.insert(child.clone()) {
                frontier.push(child);
            }
        }
    }
    Ok(all)
}

fn eval(conn: &Connection, fmt: &trellis_core::date::DateFormat, e: &Expr) -> Result<Set> {
    Ok(match e {
        Expr::Ref(t) => {
            let key = normalize_title(t);
            let page = resolve_page_key(conn, &key, fmt)?;
            inherited(
                conn,
                &[key],
                &page.into_iter().map(|p| p.0).collect::<Vec<_>>(),
            )?
        }
        Expr::Todo | Expr::Done => strings(
            conn,
            "SELECT id FROM block WHERE block_type='todo' AND checked=?1 AND deleted_at IS NULL",
            [i64::from(matches!(e, Expr::Done))],
        )?,
        Expr::Text(t) => {
            let phrase = format!("\"{}\"", t.replace('"', "\"\""));
            strings(
                conn,
                "SELECT block_id FROM block_fts WHERE block_fts MATCH ?1",
                [phrase],
            )?
        }
        Expr::Attr { key, value } => {
            let k = normalize_title(key);
            match value {
                None => strings(conn, "SELECT block_id FROM attr WHERE key_norm=?1", [k])?,
                Some(v) => {
                    let vk = normalize_title(v);
                    strings(
                        conn,
                        "SELECT block_id FROM attr a WHERE key_norm=?1 AND (value_key=?2
                           OR EXISTS (SELECT 1 FROM ref r WHERE r.src_block_id = a.block_id
                                      AND r.dst_title_key = ?2))",
                        params![k, vk],
                    )?
                }
            }
        }
        Expr::Between(a, b) => {
            let date = |s: &str| {
                fmt.parse(s)
                    .or_else(|| trellis_core::date::parse_unambiguous_date(s, fmt))
                    .or_else(|| trellis_core::date::parse_iso(s))
                    .ok_or_else(|| crate::StoreError::Invalid(format!("not a date: {s}")))
            };
            let (mut lo, mut hi) = (date(a)?, date(b)?);
            if lo > hi {
                std::mem::swap(&mut lo, &mut hi);
            }
            let pages: Vec<String> = strings(
                conn,
                "SELECT id FROM page WHERE kind='daily' AND deleted_at IS NULL
                   AND daily_date BETWEEN ?1 AND ?2",
                [lo.to_string(), hi.to_string()],
            )?
            .into_iter()
            .collect();
            inherited(conn, &[], &pages)?
        }
        Expr::And(xs) => {
            let (neg, pos): (Vec<&Expr>, Vec<&Expr>) =
                xs.iter().partition(|x| matches!(x, Expr::Not(_)));
            let mut acc: Option<Set> = None;
            for x in pos {
                let s = eval(conn, fmt, x)?;
                acc = Some(match acc {
                    None => s,
                    Some(a) => a.intersection(&s).cloned().collect(),
                });
                if acc.as_ref().is_some_and(Set::is_empty) {
                    return Ok(Set::new());
                }
            }
            let mut acc = match acc {
                Some(a) => a,
                None => universe(conn)?,
            };
            for x in neg {
                if let Expr::Not(inner) = x {
                    let s = eval(conn, fmt, inner)?;
                    acc.retain(|id| !s.contains(id));
                }
            }
            acc
        }
        Expr::Or(xs) => {
            let mut acc = Set::new();
            for x in xs {
                acc.extend(eval(conn, fmt, x)?);
            }
            acc
        }
        Expr::Not(x) => {
            let s = eval(conn, fmt, x)?;
            let mut all = universe(conn)?;
            all.retain(|id| !s.contains(id));
            all
        }
    })
}

fn universe(conn: &Connection) -> Result<Set> {
    strings(conn, "SELECT id FROM block WHERE deleted_at IS NULL", [])
}
