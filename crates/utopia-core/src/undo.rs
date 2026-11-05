//! Undo/redo (FR-2.12). Each group stores the forward commands and their inverses per
//! page, so undo and redo are ordinary command applications (and therefore sync like any
//! other edit). Consecutive text edits to the same block within 500 ms coalesce.
//!
//! The stack is global rather than per page: a merge (§8.6) or an edit made through a
//! transclusion (AC-4.2) touches several pages, and a single linear history guarantees
//! every inverse is applied to exactly the state it was computed against.

use crate::command::Command;
use crate::ids::{BlockId, PageId};

pub const COALESCE_MS: i64 = 500;
const LIMIT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoStep {
    pub page: PageId,
    pub forward: Vec<Command>,
    /// Applied in order to undo this step.
    pub inverse: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caret {
    pub page: PageId,
    pub block: BlockId,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoGroup {
    pub steps: Vec<UndoStep>,
    /// Text edits to one block coalesce; anything else never does.
    pub coalesce: Option<BlockId>,
    pub at_ms: i64,
    pub caret_before: Option<Caret>,
    pub caret_after: Option<Caret>,
}

impl UndoGroup {
    pub fn new(at_ms: i64) -> Self {
        Self {
            steps: Vec::new(),
            coalesce: None,
            at_ms,
            caret_before: None,
            caret_after: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.steps.iter().all(|s| s.forward.is_empty())
    }

    pub fn pages(&self) -> Vec<PageId> {
        let mut v: Vec<PageId> = self.steps.iter().map(|s| s.page.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}

#[derive(Debug, Default)]
pub struct UndoStack {
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a freshly performed group. Clears redo.
    pub fn push(&mut self, group: UndoGroup) {
        if group.is_empty() {
            return;
        }
        self.redo.clear();
        if let (Some(top), Some(key)) = (self.undo.last_mut(), group.coalesce.as_ref()) {
            if top.coalesce.as_ref() == Some(key) && group.at_ms - top.at_ms <= COALESCE_MS {
                top.steps.extend(group.steps);
                top.at_ms = group.at_ms;
                top.caret_after = group.caret_after;
                return;
            }
        }
        self.undo.push(group);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
    }

    /// Stop the current group from absorbing further edits (e.g. on blur or navigation).
    pub fn seal(&mut self) {
        if let Some(top) = self.undo.last_mut() {
            top.coalesce = None;
        }
    }

    pub fn pop_undo(&mut self) -> Option<UndoGroup> {
        self.undo.pop()
    }

    pub fn push_redo(&mut self, mut g: UndoGroup) {
        g.coalesce = None;
        self.redo.push(g);
    }

    pub fn pop_redo(&mut self) -> Option<UndoGroup> {
        self.redo.pop()
    }

    /// Re-push a redone group without clearing the redo stack.
    pub fn push_undone(&mut self, mut g: UndoGroup) {
        g.coalesce = None;
        self.undo.push(g);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn len(&self) -> usize {
        self.undo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::TextEdit;

    fn text_group(block: &str, at: i64) -> UndoGroup {
        let cmd = Command::SetBlockText {
            id: block.into(),
            edit: TextEdit {
                range: 0..0,
                insert: "x".into(),
            },
        };
        UndoGroup {
            steps: vec![UndoStep {
                page: "p".into(),
                forward: vec![cmd.clone()],
                inverse: vec![cmd],
            }],
            coalesce: Some(block.into()),
            at_ms: at,
            caret_before: None,
            caret_after: None,
        }
    }

    #[test]
    fn coalesces_within_window_only() {
        let mut s = UndoStack::new();
        s.push(text_group("a", 0));
        s.push(text_group("a", 400));
        s.push(text_group("a", 800));
        assert_eq!(
            s.len(),
            1,
            "each edit within 500 ms of the previous coalesces"
        );
        s.push(text_group("a", 1400));
        assert_eq!(s.len(), 2);
        s.push(text_group("b", 1450));
        assert_eq!(s.len(), 3, "different block never coalesces");
        s.seal();
        s.push(text_group("b", 1460));
        assert_eq!(s.len(), 4, "sealed group never coalesces");
    }

    #[test]
    fn redo_cleared_by_new_edit() {
        let mut s = UndoStack::new();
        s.push(text_group("a", 0));
        let g = s.pop_undo().unwrap();
        s.push_redo(g);
        assert!(s.can_redo());
        s.push(text_group("b", 5000));
        assert!(!s.can_redo());
    }
}
