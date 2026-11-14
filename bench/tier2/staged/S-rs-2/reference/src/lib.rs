//! patchwork: line diffs, unified patches, applying and reversing them,
//! and three-way merges.

pub mod apply;
pub mod diff;
pub mod error;
pub mod hunk;
pub mod lines;
pub mod merge;
pub mod patch;

pub use apply::{apply_patch, apply_patch_fuzzy, reverse_patch, Applied};
pub use diff::{diff_lines, Edit, Whitespace};
pub use error::Error;
pub use hunk::{make_hunks, Hunk, HunkLine};
pub use lines::{join_lines, split_lines};
pub use merge::{merge3, Merged};
pub use patch::{format_patch, format_patches, make_patch, parse_patch, parse_patches, Patch};
