//! Page lifecycle: creation (§10.3), daily pages (§5), commit-time normalization (§6.5),
//! rename and merge (§8.6), trash (AC-1.5), reference rewrites (FR-3.6, FR-4.14), and the
//! import-time convergence rules (dedupe, re-merge).

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension};
use utopia_core::command::PageMetaPatch;
use utopia_core::date::{iso, parse_unambiguous_date};
use utopia_core::inline::{extract_refs, heal_tags, rewrite_page_refs, unlink_page_refs};
use utopia_core::text::{diff, nfc};
use utopia_core::title::{clean_title, normalize_title};
use utopia_core::{BlockId, BlockKind, Command, DomainError, PageId, PageKind, PageState};
use utopia_doc::PageDoc;

use crate::project::{display_title, resolve_page_key};
use crate::{EditOpts, Result, Store, StoreError};

impl Store {
    fn live_daily(&self, date: NaiveDate) -> Result<Option<PageId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM page WHERE kind='daily' AND daily_date=?1 AND deleted_at IS NULL",
                [iso(date)],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(PageId))
    }

    /// Whether `id` was ever used by a page on this device (live or tombstoned).
    fn id_taken(&self, id: &PageId) -> Result<bool> {
        Ok(self.page_exists(id)?
            || self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM page WHERE id=?1)",
                [id.as_str()],
                |r| r.get(0),
            )?)
    }

    /// Resolve `title` to a page, creating it if nothing matches (`CreatePage`, App. C).
    /// Never duplicates: dates go to their daily page, existing titles and aliases win, and a
    /// tombstoned deterministic id is never resurrected (§10.3).
    pub fn create_page(&mut self, title: &str) -> Result<PageId> {
        self.sync_index()?;
        let title = clean_title(title);
        if title.is_empty() {
            return Err(DomainError::EmptyTitle.into());
        }
        let key = normalize_title(&title);
        if let Some(d) = parse_unambiguous_date(&key, &self.date_format) {
            return self.ensure_daily_page(d);
        }
        if let Some(p) = resolve_page_key(&self.conn, &key, &self.date_format)? {
            return Ok(p);
        }
        let mut id = PageId::for_title_key(&key);
        if self.id_taken(&id)? {
            id = PageId::fresh();
        }
        let now = self.now();
        let entry = self.new_doc(&id);
        entry.doc.apply(
            &Command::SetPageMeta(PageMetaPatch {
                title: Some(title),
                kind: Some(PageKind::Page),
                created_at: Some(now),
                ..Default::default()
            }),
            now,
        )?;
        self.queue(&[(id.clone(), None)], true)?;
        self.flush()?;
        Ok(id)
    }

    /// The live daily page for `date`, created empty if absent (FR-1.5, FR-1.10). A draft
    /// of that date is written out instead, so the date never gets two pages.
    pub fn ensure_daily_page(&mut self, date: NaiveDate) -> Result<PageId> {
        self.sync_index()?;
        if let Some(p) = self.live_daily(date)? {
            return Ok(p);
        }
        if let Some(p) = self.drafts.get(&date).cloned() {
            if self.docs.contains_key(&p) {
                self.queue(&[(p.clone(), None)], true)?;
                return Ok(p);
            }
            self.drafts.remove(&date);
        }
        let mut id = PageId::for_date(date);
        if self.id_taken(&id)? {
            id = PageId::fresh();
        }
        let now = self.now();
        let entry = self.new_doc(&id);
        entry.doc.apply(
            &Command::SetPageMeta(PageMetaPatch {
                kind: Some(PageKind::Daily),
                daily_date: Some(Some(date)),
                created_at: Some(now),
                ..Default::default()
            }),
            now,
        )?;
        self.queue(&[(id.clone(), None)], true)?;
        self.flush()?;
        Ok(id)
    }

    /// Visiting a date (FR-1.2): its live page, or else a draft held only in memory — one
    /// empty block, or the journal template — that is written to the graph (and synced)
    /// by its first edit. A day that is only looked at never becomes a page.
    pub fn draft_daily(&mut self, date: NaiveDate) -> Result<PageId> {
        self.sync_index()?;
        if let Some(p) = self.live_daily(date)? {
            return Ok(p);
        }
        if let Some(p) = self.drafts.get(&date) {
            if self.docs.contains_key(p) {
                return Ok(p.clone());
            }
        }
        let mut id = PageId::for_date(date);
        if self.id_taken(&id)? {
            id = PageId::fresh();
        }
        let template = self.find_page(&format!("{}journal", crate::TEMPLATE_PREFIX))?;
        let mut cmds = match template {
            Some(t) => self.template_cmds(&t, None, None)?,
            None => Vec::new(),
        };
        if cmds.is_empty() {
            cmds.push(Command::InsertBlock {
                id: BlockId::new(),
                parent: None,
                after: None,
                text: String::new(),
                kind: BlockKind::Bullet,
            });
        }
        let now = self.now();
        let entry = self.new_doc(&id);
        entry.doc.apply(
            &Command::SetPageMeta(PageMetaPatch {
                kind: Some(PageKind::Daily),
                daily_date: Some(Some(date)),
                created_at: Some(now),
                ..Default::default()
            }),
            now,
        )?;
        for cmd in &cmds {
            if let Err(e) = entry.doc.apply(cmd, now) {
                self.docs.remove(&id);
                return Err(e.into());
            }
        }
        self.drafts.insert(date, id.clone());
        Ok(id)
    }

    /// Append an empty root block to a draft day, in memory only (FR-1.2): the click that
    /// starts writing must not write the day, even when a journal template already gave it
    /// blocks. Nothing is queued, projected or flushed; the first keystroke promotes the
    /// whole draft, this block included, in one write.
    pub fn draft_append_block(&mut self, page: &PageId) -> Result<BlockId> {
        if !self.is_draft(page) {
            return Err(StoreError::Invalid("not a draft day".into()));
        }
        let after = self.page_state(page)?.last_child(None, false);
        let id = BlockId::new();
        let now = self.now();
        let entry = self
            .docs
            .get_mut(page)
            .ok_or_else(|| StoreError::NotFound(format!("draft {page}")))?;
        entry.doc.apply(
            &Command::InsertBlock {
                id: id.clone(),
                parent: None,
                after,
                text: String::new(),
                kind: BlockKind::Bullet,
            },
            now,
        )?;
        Ok(id)
    }

    /// Daily pages dated `from..=to`, drafts included: what the Daily Notes feed shows for
    /// those days (the rest are blank days, written only if you type on them).
    pub fn days_between(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<std::collections::BTreeMap<NaiveDate, PageId>> {
        self.read_barrier()?;
        let mut out = std::collections::BTreeMap::new();
        let mut stmt = self.conn.prepare_cached(
            "SELECT daily_date, id FROM page WHERE kind='daily' AND deleted_at IS NULL
               AND daily_date BETWEEN ?1 AND ?2",
        )?;
        let rows = stmt
            .query_map(params![iso(from), iso(to)], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (d, id) in rows {
            if let Ok(d) = NaiveDate::parse_from_str(&d, "%Y-%m-%d") {
                out.insert(d, PageId(id));
            }
        }
        for (d, p) in &self.drafts {
            if (from..=to).contains(d) && self.docs.contains_key(p) {
                out.entry(*d).or_insert_with(|| p.clone());
            }
        }
        Ok(out)
    }

    /// Whether `page` is a draft day (shown, never written).
    pub fn is_draft(&self, page: &PageId) -> bool {
        self.drafts.values().any(|p| p == page)
    }

    /// Stop tracking `page` as a draft; true if it was one.
    pub(crate) fn take_draft(&mut self, page: &PageId) -> bool {
        let before = self.drafts.len();
        self.drafts.retain(|_, p| p != page);
        self.drafts.len() != before
    }

    /// Visiting a page: make sure there is a block to type into (§9.3) and return the
    /// first visible one. Not an undoable edit.
    pub fn ensure_first_block(&mut self, page: &PageId) -> Result<BlockId> {
        let st = self.page_state(page)?;
        if let Some(first) = st.live_roots().first() {
            return Ok(first.clone());
        }
        let id = BlockId::new();
        let after = st.last_child(None, false);
        self.execute(
            page,
            vec![Command::InsertBlock {
                id: id.clone(),
                parent: None,
                after,
                text: String::new(),
                kind: BlockKind::Bullet,
            }],
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )?;
        Ok(id)
    }

    /// Open a date: create its page lazily and put a block in it (FR-1.2, AC-1.1).
    pub fn open_daily(&mut self, date: NaiveDate) -> Result<(PageId, BlockId)> {
        let page = self.ensure_daily_page(date)?;
        if let Some(first) = self.fill_new_daily(&page)? {
            return Ok((page, first));
        }
        let block = self.ensure_first_block(&page)?;
        Ok((page, block))
    }

    fn is_live_title(&self, phrase: &str) -> bool {
        let key = normalize_title(phrase);
        parse_unambiguous_date(&key, &self.date_format).is_none()
            && resolve_page_key(&self.conn, &key, &self.date_format)
                .ok()
                .flatten()
                .is_some()
    }

    /// Commit a block (blur, Enter, or idle; AC-3.2): NFC-normalize, apply the hashtag
    /// longest-match rule, and create pages for new `[[links]]`, tags and dates so they
    /// exist immediately (W3, FR-1.10). Returns whether the text was rewritten.
    pub fn commit_block(&mut self, page: &PageId, block: &BlockId) -> Result<bool> {
        self.sync_index()?;
        let Some(text) = self
            .page_state(page)?
            .block(block)
            .filter(|b| b.live)
            .map(|b| b.text.clone())
        else {
            return Ok(false);
        };
        let mut new = nfc(&text);
        if let Some(healed) = heal_tags(&new, |p| self.is_live_title(p)) {
            new = healed;
        }
        let rewritten = new != text && self.set_block_text(page, block, &new, None, None)?;
        let kind = self.page_state(page)?.block(block).map(|b| b.kind);
        if kind != Some(BlockKind::Code) {
            for r in extract_refs(&new) {
                if !r.is_page_like() {
                    continue;
                }
                let key = normalize_title(&r.target);
                if resolve_page_key(&self.conn, &key, &self.date_format)?.is_none() {
                    self.create_page(&r.target)?;
                }
            }
        }
        Ok(rewritten)
    }

    /// Rename (FR-3.6, FR-4.12, FR-4.13, §5.5). Returns the surviving page: `page` itself,
    /// or the page it merged into when the new title collides or is a date.
    pub fn rename_page(&mut self, page: &PageId, new_title: &str) -> Result<PageId> {
        self.sync_index()?;
        let meta = self.page_state(page)?.meta.clone();
        if meta.kind == PageKind::Daily {
            return Err(DomainError::DailyTitleReadOnly.into());
        }
        let new = clean_title(new_title);
        if new.is_empty() {
            return Err(DomainError::EmptyTitle.into());
        }
        let key = normalize_title(&new);
        if let Some(d) = parse_unambiguous_date(&key, &self.date_format) {
            let daily = self.ensure_daily_page(d)?;
            self.merge_pages(page, &daily)?;
            return Ok(daily);
        }
        let old_key = normalize_title(&meta.title);
        if key != old_key {
            if let Some(other) = resolve_page_key(&self.conn, &key, &self.date_format)? {
                if other != *page {
                    self.merge_pages(page, &other)?;
                    return Ok(other);
                }
            }
        }
        if new == meta.title {
            return Ok(page.clone());
        }
        let mut aliases = meta.aliases.clone();
        if key != old_key && !meta.title.is_empty() {
            aliases.push(meta.title.clone());
        }
        self.seal_undo();
        self.execute(
            page,
            vec![Command::SetPageMeta(PageMetaPatch {
                title: Some(new),
                aliases: Some(aliases),
                ..Default::default()
            })],
            EditOpts::default(),
        )?;
        self.seal_undo();
        Ok(page.clone())
    }

    /// Commands that move every block of `from` not yet moved into `into`, preserving ids
    /// and subtrees (§8.6 step 1). Blocks whose parent already lives in `into` attach to
    /// it; others become roots appended after `into`'s last root.
    fn plan_block_moves(from: &PageState, into: &PageState) -> (Vec<Command>, Vec<BlockId>) {
        let mut cmds = Vec::new();
        let mut moved: Vec<BlockId> = Vec::new();
        let mut in_batch: HashSet<BlockId> = HashSet::new();
        let mut last_under: HashMap<Option<BlockId>, Option<BlockId>> = HashMap::new();
        let mut order = Vec::new();
        for r in &from.roots {
            order.extend(from.subtree(r, false));
        }
        for x in order {
            let Some(node) = from.block(&x) else { continue };
            if node.moved_to.is_some() {
                continue;
            }
            moved.push(x.clone());
            if into.contains(&x) {
                // Already there (a concurrent re-merge won); only hide it here.
                in_batch.insert(x.clone());
                continue;
            }
            let parent = node
                .parent
                .clone()
                .filter(|p| in_batch.contains(p) || into.contains(p));
            let after = last_under
                .entry(parent.clone())
                .or_insert_with(|| match &parent {
                    Some(p) if in_batch.contains(p) && !into.contains(p) => None,
                    p => into.last_child(p.as_ref(), false),
                })
                .clone();
            cmds.push(Command::InsertBlock {
                id: x.clone(),
                parent: parent.clone(),
                after,
                text: node.text.clone(),
                kind: node.kind,
            });
            if node.checked {
                cmds.push(Command::SetBlockKind {
                    id: x.clone(),
                    kind: node.kind,
                    checked: true,
                });
            }
            if let Some(d) = node.deleted_at {
                cmds.push(Command::SetDeleted {
                    ids: vec![x.clone()],
                    deleted_at: Some(d),
                });
            }
            last_under.insert(parent, Some(x.clone()));
            in_batch.insert(x);
        }
        (cmds, moved)
    }

    /// The full merge plan (§8.6): blocks move, aliases transfer, `from` is tombstoned
    /// with a pointer to the survivor. Emits only ordinary commands, so sync needs no
    /// special merge message (step 6).
    fn plan_merge(&mut self, from: &PageId, into: &PageId) -> Result<Vec<(PageId, Vec<Command>)>> {
        if from == into {
            return Err(DomainError::NoOp.into());
        }
        let b = self.page_state(from)?.clone();
        let a = self.page_state(into)?.clone();
        let now = self.now();
        let (mut cmds_a, moved) = Self::plan_block_moves(&b, &a);
        let mut aliases = a.meta.aliases.clone();
        aliases.extend(b.meta.aliases.iter().cloned());
        if b.meta.kind == PageKind::Page && !b.meta.title.is_empty() {
            aliases.push(b.meta.title.clone());
        }
        cmds_a.push(Command::SetPageMeta(PageMetaPatch {
            aliases: Some(aliases),
            ..Default::default()
        }));
        let mut cmds_b = Vec::new();
        if !moved.is_empty() {
            cmds_b.push(Command::SetMoved {
                ids: moved,
                to: Some(into.clone()),
            });
        }
        cmds_b.push(Command::SetPageMeta(PageMetaPatch {
            deleted_at: Some(Some(b.meta.deleted_at.unwrap_or(now))),
            merged_into: Some(Some(into.clone())),
            ..Default::default()
        }));
        Ok(vec![(from.clone(), cmds_b), (into.clone(), cmds_a)])
    }

    /// `MergePages` (§8.6): one transaction, one undo step.
    pub fn merge_pages(&mut self, from: &PageId, into: &PageId) -> Result<()> {
        self.sync_index()?;
        let steps = self.plan_merge(from, into)?;
        self.seal_undo();
        self.execute_multi(steps, EditOpts::default())?;
        self.seal_undo();
        Ok(())
    }

    /// Delete a page to the trash (AC-1.5). Undoable; restorable for 30 days.
    pub fn delete_page(&mut self, page: &PageId) -> Result<()> {
        let st = self.page_state(page)?.clone();
        if st.meta.deleted_at.is_some() {
            return Err(DomainError::NoOp.into());
        }
        let title = display_title(&st, &self.date_format);
        let now = self.now();
        self.seal_undo();
        self.execute(
            page,
            vec![Command::SetPageMeta(PageMetaPatch {
                deleted_at: Some(Some(now)),
                ..Default::default()
            })],
            EditOpts::default(),
        )?;
        self.seal_undo();
        self.conn.execute(
            "INSERT OR REPLACE INTO trash(id, kind, payload, deleted_at) VALUES (?1, 'page', ?2, ?3)",
            params![page.as_str(), serde_json::json!({ "title": title }).to_string().into_bytes(), now],
        )?;
        Ok(())
    }

    /// Restore a trashed page. If its title or date is taken by a live page meanwhile, it
    /// merges into that page instead of creating a duplicate. Returns the surviving page.
    pub fn restore_page(&mut self, page: &PageId) -> Result<PageId> {
        self.sync_index()?;
        let meta = self.page_state(page)?.meta.clone();
        if meta.deleted_at.is_none() {
            return Ok(page.clone());
        }
        let existing = match (meta.kind, meta.daily_date) {
            (PageKind::Daily, Some(d)) => self.live_daily(d)?,
            _ => resolve_page_key(&self.conn, &normalize_title(&meta.title), &self.date_format)?,
        };
        let survivor = match existing {
            Some(other) if other != *page => {
                self.merge_pages(page, &other)?;
                other
            }
            _ => {
                self.execute(
                    page,
                    vec![Command::SetPageMeta(PageMetaPatch {
                        deleted_at: Some(None),
                        merged_into: Some(None),
                        ..Default::default()
                    })],
                    EditOpts::default(),
                )?;
                page.clone()
            }
        };
        self.conn
            .execute("DELETE FROM trash WHERE id=?1", [page.as_str()])?;
        Ok(survivor)
    }

    /// Trashed pages that are still tombstoned: `(id, title, deleted_at)`.
    pub fn list_trash(&self) -> Result<Vec<(PageId, String, i64)>> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare(
            "SELECT t.id, p.title, t.deleted_at FROM trash t JOIN page p ON p.id=t.id
             WHERE p.deleted_at IS NOT NULL ORDER BY t.deleted_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((PageId(r.get(0)?), r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Hard-delete trashed pages deleted before `cutoff_ms` (all when `None`). The only
    /// operation that permanently removes data (§6.5).
    pub fn empty_trash(&mut self, cutoff_ms: Option<i64>) -> Result<usize> {
        self.sync_index()?;
        let ids: Vec<PageId> = self
            .list_trash()?
            .into_iter()
            .filter(|(_, _, at)| cutoff_ms.is_none_or(|c| *at < c))
            .map(|(id, _, _)| id)
            .collect();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for id in &ids {
            tx.execute("DELETE FROM ref WHERE src_page_id=?1", [id.as_str()])?;
            tx.execute("DELETE FROM block WHERE page_id=?1", [id.as_str()])?;
            tx.execute("DELETE FROM page_update WHERE page_id=?1", [id.as_str()])?;
            tx.execute("DELETE FROM page WHERE id=?1", [id.as_str()])?;
            tx.execute("DELETE FROM trash WHERE id=?1", [id.as_str()])?;
        }
        tx.commit()?;
        for id in &ids {
            self.docs.remove(id);
        }
        Ok(ids.len())
    }

    fn rewrite_blocks(
        &mut self,
        blocks: Vec<(PageId, BlockId)>,
        rewrite: impl Fn(&str) -> Option<String>,
    ) -> Result<usize> {
        let mut steps: Vec<(PageId, Vec<Command>)> = Vec::new();
        let mut n = 0;
        for (page, block) in blocks {
            let Some(text) = self
                .page_state(&page)?
                .block(&block)
                .map(|b| b.text.clone())
            else {
                continue;
            };
            let Some(new) = rewrite(&text) else { continue };
            let Some(edit) = diff(&text, &new) else {
                continue;
            };
            n += 1;
            let cmd = Command::SetBlockText { id: block, edit };
            match steps.iter_mut().find(|(p, _)| *p == page) {
                Some((_, v)) => v.push(cmd),
                None => steps.push((page, vec![cmd])),
            }
        }
        if n > 0 {
            self.seal_undo();
            self.execute_multi(steps, EditOpts::default())?;
            self.seal_undo();
        }
        Ok(n)
    }

    /// "Rewrite references to new title" (FR-3.6): every link that reaches this page
    /// through an alias is rewritten to the current title, as one undo step.
    pub fn rewrite_references(&mut self, page: &PageId) -> Result<usize> {
        self.sync_index()?;
        let meta = self.page_state(page)?.meta.clone();
        if meta.kind == PageKind::Daily {
            return Ok(0);
        }
        let alias_keys: HashSet<String> = meta.aliases.iter().map(|a| normalize_title(a)).collect();
        let blocks: Vec<(PageId, BlockId)> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT b.page_id, b.id FROM ref r JOIN block b ON b.id = r.src_block_id
                 WHERE r.dst_page_id=?1 AND r.dst_kind='page' AND b.deleted_at IS NULL",
            )?;
            let rows = stmt
                .query_map([page.as_str()], |r| {
                    Ok((PageId(r.get(0)?), BlockId(r.get(1)?)))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let title = meta.title.clone();
        self.rewrite_blocks(blocks, |text| {
            rewrite_page_refs(text, |t| alias_keys.contains(&normalize_title(t)), &title)
        })
    }

    /// Unresolved intent → "Redirect to an existing page" (FR-4.14), within one block.
    pub fn redirect_intent(&mut self, block: &BlockId, key: &str, to: &PageId) -> Result<()> {
        self.sync_index()?;
        let page = self.block_page(block)?;
        let to_title = self
            .page_row(to)?
            .ok_or_else(|| StoreError::NotFound(format!("page {to}")))?
            .title;
        let key = normalize_title(key);
        self.rewrite_blocks(vec![(page, block.clone())], |text| {
            rewrite_page_refs(text, |t| normalize_title(t) == key, &to_title)
        })?;
        Ok(())
    }

    /// Unresolved intent → "Convert to plain text" (FR-4.14), within one block.
    pub fn unlink_intent(&mut self, block: &BlockId, key: &str) -> Result<()> {
        self.sync_index()?;
        let page = self.block_page(block)?;
        let key = normalize_title(key);
        self.rewrite_blocks(vec![(page, block.clone())], |text| {
            unlink_page_refs(text, |t| normalize_title(t) == key)
        })?;
        Ok(())
    }

    /// Import-time deduplication (§8.6): if `page` is live and its title (or date) matches
    /// another live page, merge the larger id into the smaller one so every device picks
    /// the same survivor. Applies commands in memory; returns the other pages touched.
    pub(crate) fn dedupe_collisions(&mut self, page: &PageId) -> Result<Vec<PageId>> {
        let st = self.page_state(page)?.clone();
        if st.meta.deleted_at.is_some() {
            return Ok(Vec::new());
        }
        let other: Option<PageId> = match (st.meta.kind, st.meta.daily_date) {
            (PageKind::Daily, Some(d)) => self
                .conn
                .query_row(
                    "SELECT id FROM page WHERE kind='daily' AND daily_date=?1 AND deleted_at IS NULL AND id<>?2",
                    params![iso(d), page.as_str()],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .map(PageId),
            _ => self
                .conn
                .query_row(
                    "SELECT id FROM page WHERE kind='page' AND title_key=?1 AND deleted_at IS NULL AND id<>?2",
                    params![normalize_title(&st.meta.title), page.as_str()],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .map(PageId),
        };
        let Some(other) = other else {
            return Ok(Vec::new());
        };
        let (from, into) = if *page < other {
            (other.clone(), page.clone())
        } else {
            (page.clone(), other.clone())
        };
        let now = self.now();
        for (p, cmds) in self.plan_merge(&from, &into)? {
            self.apply_cmds(&p, &cmds, now, false)?;
        }
        Ok(vec![other])
    }

    /// Convergent merge rule (§8.6): blocks that arrive in a merged-away page after the
    /// merge are moved into the survivor (following chains of merges).
    pub(crate) fn reconcile_merged(&mut self, page: &PageId) -> Result<bool> {
        let mut current = page.clone();
        let mut changed = false;
        for _ in 0..8 {
            let st = self.page_state(&current)?.clone();
            let (Some(_), Some(into)) = (st.meta.deleted_at, st.meta.merged_into.clone()) else {
                break;
            };
            let has_orphans = st.blocks.values().any(|b| b.moved_to.is_none());
            if has_orphans && self.page_exists(&into)? {
                let a = self.page_state(&into)?.clone();
                let (cmds_a, moved) = Self::plan_block_moves(&st, &a);
                let steps = vec![
                    (
                        current.clone(),
                        vec![Command::SetMoved {
                            ids: moved,
                            to: Some(into.clone()),
                        }],
                    ),
                    (into.clone(), cmds_a),
                ];
                self.execute_multi(
                    steps,
                    EditOpts {
                        no_undo: true,
                        ..Default::default()
                    },
                )?;
                changed = true;
            }
            current = into;
        }
        Ok(changed)
    }

    /// Import a batch of remote updates (§12.3 step 3): import every shard, then dedupe
    /// and project page by page, then re-merge orphans. Idempotent.
    pub fn import_batch(&mut self, batch: &[(PageId, Vec<u8>)]) -> Result<Vec<PageId>> {
        self.sync_index()?;
        let mut changed: Vec<PageId> = Vec::new();
        for (page, bytes) in batch {
            // Another device wrote this day first: drop the local draft so its unsent
            // empty block never merges into theirs.
            if self.take_draft(page) {
                self.docs.remove(page);
            }
            let did = if self.page_exists(page)? {
                self.load_doc(page)?.doc.import(bytes)
            } else {
                self.new_doc(page).doc.import(bytes)
            };
            let did = match did {
                Ok(did) => did,
                Err(e) => {
                    // The shards imported so far were never committed (the loop below did
                    // not run), so they exist only in memory. Drop every page of the batch
                    // so a retry re-imports its payload instead of seeing an unchanged
                    // version vector and skipping it (§12.3 steps 3–4).
                    for (p, _) in batch {
                        self.docs.remove(p);
                    }
                    return Err(e.into());
                }
            };
            if did && !changed.contains(page) {
                changed.push(page.clone());
            }
        }
        for page in &changed {
            let others = self.dedupe_collisions(page)?;
            let local_ops = !others.is_empty();
            let mut pages = vec![(page.clone(), None)];
            pages.extend(others.into_iter().map(|p| (p, None)));
            // Merge commands generated here are new local ops and must reach the relay.
            // Each page is committed before the next one's dedupe check reads the index.
            self.queue(&pages, local_ops)?;
            self.sync_index()?;
        }
        for page in &changed {
            self.reconcile_merged(page)?;
        }
        Ok(changed)
    }
}
