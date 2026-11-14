//! patchwork: line diffs, unified patches, applying and reversing them.

pub mod apply;
pub mod diff;
pub mod error;
pub mod hunk;
pub mod lines;
pub mod patch;

pub use apply::{apply_patch, reverse_patch, Applied};
pub use diff::{diff_lines, Edit};
pub use error::Error;
pub use hunk::{make_hunks, Hunk, HunkLine};
pub use lines::{join_lines, split_lines};
pub use patch::{format_patch, make_patch, parse_patch, Patch};
