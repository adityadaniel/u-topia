use thiserror::Error;

use crate::ids::{BlockId, PageId};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    #[error("block {0} not found")]
    BlockNotFound(BlockId),
    #[error("page {0} not found")]
    PageNotFound(PageId),
    #[error("block {0} has no previous sibling to indent under")]
    NoPreviousSibling(BlockId),
    #[error("block {0} is already at the top level")]
    AlreadyRoot(BlockId),
    #[error("cannot move block {0} into its own subtree")]
    CycleDetected(BlockId),
    #[error("maximum depth exceeded")]
    TooDeep,
    #[error("text edit {start}..{end} is out of bounds or not on a char boundary")]
    InvalidTextEdit { start: usize, end: usize },
    #[error("daily page titles are display-only and cannot be renamed")]
    DailyTitleReadOnly,
    #[error("title must not be empty")]
    EmptyTitle,
    #[error("nothing to do")]
    NoOp,
}
