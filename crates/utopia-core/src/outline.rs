//! Outliner operations (PRD §6.3, Appendix C) planned as primitive [`Command`]s.
//!
//! Planners read a [`PageState`] and return the commands to apply plus where the caret
//! should land. They never mutate state, so a whole plan becomes one undo step.

use crate::command::{BlockKind, Command};
use crate::error::DomainError;
use crate::ids::BlockId;
use crate::markdown::parse_outline;
use crate::text::{diff, floor_char_boundary};
use crate::tree::PageState;

/// Maximum depth accepted for drag-and-drop (FR-2.9).
pub const MAX_DROP_DEPTH: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focus {
    pub block: BlockId,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub cmds: Vec<Command>,
    pub focus: Option<Focus>,
}

impl Plan {
    fn focus(mut self, block: &BlockId, offset: usize) -> Self {
        self.focus = Some(Focus {
            block: block.clone(),
            offset,
        });
        self
    }
}

fn node<'a>(st: &'a PageState, id: &BlockId) -> Result<&'a crate::tree::BlockNode, DomainError> {
    st.block(id)
        .ok_or_else(|| DomainError::BlockNotFound(id.clone()))
}

/// Enter (FR-2.2, §9.1): split at the caret. The tail moves to a new block below; it is
/// the first child when the block has expanded children, otherwise the next sibling. At
/// offset 0 of a non-empty block a new empty block is created above instead.
pub fn split_block(
    st: &PageState,
    id: &BlockId,
    caret: usize,
    collapsed: bool,
) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let caret = floor_char_boundary(&n.text, caret);
    let new_id = BlockId::new();
    if caret == 0 && !n.text.is_empty() {
        let plan = Plan {
            cmds: vec![Command::InsertBlock {
                id: new_id,
                parent: n.parent.clone(),
                after: st.prev_sibling(id, false),
                text: String::new(),
                kind: n.kind,
            }],
            focus: None,
        };
        return Ok(plan.focus(id, 0));
    }
    let tail = n.text[caret..].to_string();
    let mut cmds = Vec::new();
    if !tail.is_empty() {
        cmds.push(Command::SetBlockText {
            id: id.clone(),
            edit: crate::command::TextEdit {
                range: caret..n.text.len(),
                insert: String::new(),
            },
        });
    }
    let (parent, after) = if st.has_live_children(id) && !collapsed {
        (Some(id.clone()), None)
    } else {
        (n.parent.clone(), Some(id.clone()))
    };
    cmds.push(Command::InsertBlock {
        id: new_id.clone(),
        parent,
        after,
        text: tail,
        kind: n.kind,
    });
    Ok(Plan { cmds, focus: None }.focus(&new_id, 0))
}

/// Cmd+Enter: a new empty block directly above.
pub fn insert_above(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let new_id = BlockId::new();
    Ok(Plan {
        cmds: vec![Command::InsertBlock {
            id: new_id.clone(),
            parent: n.parent.clone(),
            after: st.prev_sibling(id, false),
            text: String::new(),
            kind: n.kind,
        }],
        focus: None,
    }
    .focus(&new_id, 0))
}

/// Append a new empty block at the end of the page (or of `parent`).
pub fn append_block(st: &PageState, parent: Option<&BlockId>) -> Plan {
    let new_id = BlockId::new();
    Plan {
        cmds: vec![Command::InsertBlock {
            id: new_id.clone(),
            parent: parent.cloned(),
            after: st.last_child(parent, false),
            text: String::new(),
            kind: BlockKind::Bullet,
        }],
        focus: None,
    }
    .focus(&new_id, 0)
}

/// Tab (FR-2.4): become the last child of the previous sibling, carrying the subtree.
pub fn indent(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    node(st, id)?;
    let prev = st
        .prev_sibling(id, true)
        .ok_or_else(|| DomainError::NoPreviousSibling(id.clone()))?;
    Ok(Plan {
        cmds: vec![Command::MoveBlock {
            id: id.clone(),
            parent: Some(prev.clone()),
            after: st.last_child(Some(&prev), false),
        }],
        focus: None,
    })
}

/// Shift+Tab (FR-2.4): become the next sibling of the parent.
pub fn outdent(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let parent = n
        .parent
        .clone()
        .ok_or_else(|| DomainError::AlreadyRoot(id.clone()))?;
    Ok(Plan {
        cmds: vec![Command::MoveBlock {
            id: id.clone(),
            parent: st.parent_of(&parent),
            after: Some(parent),
        }],
        focus: None,
    })
}

/// Backspace at offset 0 (FR-2.5): merge into the previous visible block — text appended,
/// children re-parented, block tombstoned. On the first visible block it outdents.
pub fn merge_into_previous(
    st: &PageState,
    id: &BlockId,
    zoom: Option<&BlockId>,
    collapsed: &dyn Fn(&BlockId) -> bool,
    now: i64,
) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let rows = st.visible_rows(zoom, collapsed);
    let ix = rows
        .iter()
        .position(|(r, _)| r == id)
        .ok_or_else(|| DomainError::BlockNotFound(id.clone()))?;
    if ix == 0 {
        return match &n.parent {
            Some(p) if Some(p) != zoom => {
                let mut plan = outdent(st, id)?;
                plan.focus = Some(Focus {
                    block: id.clone(),
                    offset: 0,
                });
                Ok(plan)
            }
            _ => Err(DomainError::NoOp),
        };
    }
    let prev = rows[ix - 1].0.clone();
    let prev_node = node(st, &prev)?;
    let prev_len = prev_node.text.len();
    let mut cmds = Vec::new();
    if !n.text.is_empty() {
        cmds.push(Command::SetBlockText {
            id: prev.clone(),
            edit: crate::command::TextEdit {
                range: prev_len..prev_len,
                insert: n.text.clone(),
            },
        });
    }
    let mut after = if n.parent.as_ref() == Some(&prev) {
        st.prev_sibling(id, false)
    } else {
        st.last_child(Some(&prev), false)
    };
    for c in &n.children {
        if st.block(c).is_some_and(|b| b.present) {
            cmds.push(Command::MoveBlock {
                id: c.clone(),
                parent: Some(prev.clone()),
                after: after.clone(),
            });
            after = Some(c.clone());
        }
    }
    cmds.push(Command::SetDeleted {
        ids: vec![id.clone()],
        deleted_at: Some(now),
    });
    Ok(Plan { cmds, focus: None }.focus(&prev, prev_len))
}

/// Cmd+Shift+↑ (FR-2.6). At the top edge the block becomes its parent's previous sibling.
pub fn move_up(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let cmd = match st.prev_sibling(id, true) {
        Some(p) => Command::MoveBlock {
            id: id.clone(),
            parent: n.parent.clone(),
            after: st.prev_sibling(&p, false),
        },
        None => {
            let parent = n.parent.clone().ok_or(DomainError::NoOp)?;
            Command::MoveBlock {
                id: id.clone(),
                parent: st.parent_of(&parent),
                after: st.prev_sibling(&parent, false),
            }
        }
    };
    Ok(Plan {
        cmds: vec![cmd],
        focus: None,
    })
}

/// Cmd+Shift+↓ (FR-2.6). At the bottom edge the block becomes its parent's next sibling.
pub fn move_down(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let cmd = match st.next_sibling(id, true) {
        Some(next) => Command::MoveBlock {
            id: id.clone(),
            parent: n.parent.clone(),
            after: Some(next),
        },
        None => {
            let parent = n.parent.clone().ok_or(DomainError::NoOp)?;
            Command::MoveBlock {
                id: id.clone(),
                parent: st.parent_of(&parent),
                after: Some(parent),
            }
        }
    };
    Ok(Plan {
        cmds: vec![cmd],
        focus: None,
    })
}

/// Cmd+Shift+Backspace: tombstone the block and its live subtree (§6.5).
pub fn delete_subtree(st: &PageState, id: &BlockId, now: i64) -> Result<Plan, DomainError> {
    node(st, id)?;
    let ids = st.subtree(id, true);
    if ids.is_empty() {
        return Err(DomainError::NoOp);
    }
    let rows = st.live_preorder();
    let ix = rows.iter().position(|(r, _)| r == id);
    let focus = ix.and_then(|i| {
        rows[..i]
            .iter()
            .rev()
            .chain(rows[i + 1..].iter())
            .find(|(r, _)| !ids.contains(r))
            .map(|(r, _)| r.clone())
    });
    let plan = Plan {
        cmds: vec![Command::SetDeleted {
            ids,
            deleted_at: Some(now),
        }],
        focus: None,
    };
    Ok(match focus {
        Some(f) => {
            let len = st.block(&f).map_or(0, |b| b.text.len());
            plan.focus(&f, len)
        }
        None => plan,
    })
}

/// Replace a block's text with `new_text` as one minimal edit.
pub fn set_text(st: &PageState, id: &BlockId, new_text: &str) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let edit = diff(&n.text, new_text).ok_or(DomainError::NoOp)?;
    Ok(Plan {
        cmds: vec![Command::SetBlockText {
            id: id.clone(),
            edit,
        }],
        focus: None,
    })
}

/// Drag-and-drop (FR-2.9): rejects drops into the block's own subtree and drops that
/// would push any descendant deeper than [`MAX_DROP_DEPTH`].
pub fn move_to(
    st: &PageState,
    id: &BlockId,
    parent: Option<&BlockId>,
    after: Option<&BlockId>,
) -> Result<Plan, DomainError> {
    node(st, id)?;
    if let Some(p) = parent {
        node(st, p)?;
        if p == id || st.is_ancestor(id, p) {
            return Err(DomainError::CycleDetected(id.clone()));
        }
    }
    let base = parent.map_or(0, |p| st.depth(p) + 1);
    let own = st.depth(id);
    let max_rel = st
        .subtree(id, true)
        .iter()
        .map(|b| st.depth(b) - own)
        .max()
        .unwrap_or(0);
    if base + max_rel >= MAX_DROP_DEPTH {
        return Err(DomainError::TooDeep);
    }
    if after == Some(id) {
        return Err(DomainError::NoOp);
    }
    Ok(Plan {
        cmds: vec![Command::MoveBlock {
            id: id.clone(),
            parent: parent.cloned(),
            after: after.cloned(),
        }],
        focus: None,
    })
}

/// Paste (FR-2.10). A single line is inserted at the caret. Multi-line text becomes one
/// block per line with relative indentation preserved, placed after the current block (or
/// filling it, when it is empty and childless).
pub fn paste(
    st: &PageState,
    id: &BlockId,
    selection: std::ops::Range<usize>,
    pasted: &str,
) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let start = floor_char_boundary(&n.text, selection.start);
    let end = floor_char_boundary(&n.text, selection.end.max(start));
    let pasted = pasted.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = pasted.trim_end_matches('\n');
    if !trimmed.contains('\n') {
        let line = trimmed.to_string();
        if line.is_empty() && start == end {
            return Err(DomainError::NoOp);
        }
        let len = line.len();
        return Ok(Plan {
            cmds: vec![Command::SetBlockText {
                id: id.clone(),
                edit: crate::command::TextEdit {
                    range: start..end,
                    insert: line,
                },
            }],
            focus: None,
        }
        .focus(id, start + len));
    }
    let items = parse_outline(trimmed);
    if items.is_empty() {
        return Err(DomainError::NoOp);
    }
    let mut cmds = Vec::new();
    // last[d]: the most recent block at depth d; parents[d]: its parent.
    let mut last: Vec<BlockId> = Vec::new();
    let mut parents: Vec<Option<BlockId>> = Vec::new();
    let mut rest = &items[..];
    let reuse_current = n.text.is_empty() && n.children.is_empty();
    if reuse_current {
        cmds.push(Command::SetBlockText {
            id: id.clone(),
            edit: crate::command::TextEdit {
                range: 0..0,
                insert: items[0].1.clone(),
            },
        });
        rest = &items[1..];
    }
    last.push(id.clone());
    parents.push(n.parent.clone());
    // Children of a freshly inserted block always start empty, so `after` for the first
    // child is None; for the pre-existing current block it is its last child.
    let mut first_child_after: Option<Option<BlockId>> = Some(st.last_child(Some(id), false));
    let mut focus = (
        id.clone(),
        if reuse_current {
            items[0].1.len()
        } else {
            n.text.len()
        },
    );
    for (depth, text) in rest {
        let d = (*depth).min(last.len());
        let new_id = BlockId::new();
        let (parent, after) = if d < last.len() {
            // Sibling of last[d].
            (parents[d].clone(), Some(last[d].clone()))
        } else {
            // First child of last[d-1].
            let after = if last[d - 1] == *id {
                first_child_after.take().flatten()
            } else {
                None
            };
            (Some(last[d - 1].clone()), after)
        };
        cmds.push(Command::InsertBlock {
            id: new_id.clone(),
            parent: parent.clone(),
            after,
            text: text.clone(),
            kind: BlockKind::Bullet,
        });
        last.truncate(d);
        parents.truncate(d);
        last.push(new_id.clone());
        parents.push(parent);
        focus = (new_id, text.len());
    }
    Ok(Plan { cmds, focus: None }.focus(&focus.0, focus.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::PageMeta;

    fn apply(st: &mut PageState, plan: &Plan) {
        for c in &plan.cmds {
            st.apply(c, 10).unwrap();
        }
    }

    fn rows(st: &PageState) -> Vec<(String, usize)> {
        st.live_preorder()
            .into_iter()
            .map(|(id, d)| (st.block(&id).unwrap().text.clone(), d))
            .collect()
    }

    fn page_with(lines: &[(&str, usize)]) -> (PageState, Vec<BlockId>) {
        let mut st = PageState::new("p".into(), PageMeta::default());
        let items: Vec<(usize, String)> = lines.iter().map(|(t, d)| (*d, t.to_string())).collect();
        let root = BlockId::new();
        st.apply(
            &Command::InsertBlock {
                id: root.clone(),
                parent: None,
                after: None,
                text: String::new(),
                kind: BlockKind::Bullet,
            },
            0,
        )
        .unwrap();
        let text: String = items
            .iter()
            .map(|(d, t)| format!("{}{}\n", "  ".repeat(*d), t))
            .collect();
        let plan = paste(&st, &root, 0..0, &text).unwrap();
        apply(&mut st, &plan);
        let ids = st.live_preorder().into_iter().map(|(id, _)| id).collect();
        (st, ids)
    }

    #[test]
    fn enter_splits_and_places() {
        let (mut st, ids) = page_with(&[("hello world", 0), ("child", 1)]);
        // Caret mid-block with expanded children: tail becomes first child.
        let plan = split_block(&st, &ids[0], 5, false).unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![
                ("hello".into(), 0),
                (" world".into(), 1),
                ("child".into(), 1)
            ]
        );
        // Caret at 0 of non-empty block: empty block above, focus stays.
        let plan = split_block(&st, &ids[0], 0, false).unwrap();
        assert_eq!(plan.focus.as_ref().unwrap().block, ids[0]);
        apply(&mut st, &plan);
        assert_eq!(rows(&st)[0], ("".into(), 0));
    }

    #[test]
    fn enter_on_collapsed_parent_creates_sibling() {
        let (mut st, ids) = page_with(&[("parent", 0), ("child", 1)]);
        let plan = split_block(&st, &ids[0], 6, true).unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![("parent".into(), 0), ("child".into(), 1), ("".into(), 0)]
        );
    }

    #[test]
    fn tab_and_shift_tab() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("b1", 1)]);
        assert!(matches!(
            indent(&st, &ids[0]),
            Err(DomainError::NoPreviousSibling(_))
        ));
        {
            let p = indent(&st, &ids[1]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("a".into(), 0), ("b".into(), 1), ("b1".into(), 2)]
        );
        {
            let p = outdent(&st, &ids[1]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("a".into(), 0), ("b".into(), 0), ("b1".into(), 1)]
        );
        assert!(matches!(
            outdent(&st, &ids[0]),
            Err(DomainError::AlreadyRoot(_))
        ));
    }

    #[test]
    fn backspace_merges_text_and_children() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("b1", 1), ("c", 0)]);
        let plan = merge_into_previous(&st, &ids[1], None, &|_| false, 99).unwrap();
        assert_eq!(
            plan.focus,
            Some(Focus {
                block: ids[0].clone(),
                offset: 1
            })
        );
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![("ab".into(), 0), ("b1".into(), 1), ("c".into(), 0)]
        );
        assert_eq!(st.block(&ids[1]).unwrap().deleted_at, Some(99));
        // First child merging into its parent keeps its children in place.
        let (mut st, ids) = page_with(&[("p", 0), ("x", 1), ("x1", 2), ("y", 1)]);
        {
            let p = merge_into_previous(&st, &ids[1], None, &|_| false, 1).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("px".into(), 0), ("x1".into(), 1), ("y".into(), 1)]
        );
        // First block at root: nothing to do.
        assert_eq!(
            merge_into_previous(&st, &ids[0], None, &|_| false, 1),
            Err(DomainError::NoOp)
        );
    }

    #[test]
    fn move_up_down_edges() {
        let (mut st, ids) = page_with(&[("p", 0), ("x", 1), ("y", 1)]);
        {
            let p = move_up(&st, &ids[2]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("p".into(), 0), ("y".into(), 1), ("x".into(), 1)]
        );
        {
            let p = move_up(&st, &ids[2]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("y".into(), 0), ("p".into(), 0), ("x".into(), 1)]
        );
        {
            let p = move_down(&st, &ids[1]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("y".into(), 0), ("p".into(), 0), ("x".into(), 0)]
        );
        assert_eq!(move_down(&st, &ids[1]), Err(DomainError::NoOp));
    }

    #[test]
    fn drop_validation() {
        let (st, ids) = page_with(&[("a", 0), ("b", 1)]);
        assert!(matches!(
            move_to(&st, &ids[0], Some(&ids[1]), None),
            Err(DomainError::CycleDetected(_))
        ));
        // A 12-level chain (depths 0..=11) plus a separate leaf.
        let mut lines: Vec<(String, usize)> = (0..12).map(|d| (format!("d{d}"), d)).collect();
        lines.push(("leaf".into(), 0));
        let lines_ref: Vec<(&str, usize)> = lines.iter().map(|(t, d)| (t.as_str(), *d)).collect();
        let (st, ids) = page_with(&lines_ref);
        let leaf = &ids[12];
        assert_eq!(
            move_to(&st, leaf, Some(&ids[11]), None),
            Err(DomainError::TooDeep)
        );
        assert!(move_to(&st, leaf, Some(&ids[10]), None).is_ok());
        assert_eq!(move_to(&st, leaf, None, Some(leaf)), Err(DomainError::NoOp));
    }

    #[test]
    fn paste_preserves_nesting() {
        let (st, _) = page_with(&[("one", 0), ("two", 1), ("three", 2), ("four", 0)]);
        assert_eq!(
            rows(&st),
            vec![
                ("one".into(), 0),
                ("two".into(), 1),
                ("three".into(), 2),
                ("four".into(), 0)
            ]
        );
        // Pasting after a non-empty block inserts siblings after it.
        let (mut st, ids) = page_with(&[("keep", 0)]);
        let plan = paste(&st, &ids[0], 4..4, "x\n  y\nz").unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![
                ("keep".into(), 0),
                ("x".into(), 0),
                ("y".into(), 1),
                ("z".into(), 0)
            ]
        );
        // Single line goes inline at the caret.
        let plan = paste(&st, &ids[0], 2..2, "--").unwrap();
        apply(&mut st, &plan);
        assert_eq!(st.block(&ids[0]).unwrap().text, "ke--ep");
    }

    #[test]
    fn delete_tombstones_live_subtree() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("b1", 1)]);
        let plan = delete_subtree(&st, &ids[1], 7).unwrap();
        assert_eq!(plan.focus.as_ref().unwrap().block, ids[0]);
        apply(&mut st, &plan);
        assert_eq!(rows(&st), vec![("a".into(), 0)]);
        assert!(st.block(&ids[2]).unwrap().deleted_at.is_some());
    }
}
