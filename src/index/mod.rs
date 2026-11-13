//! The on-disk index: format, writer and memory-mapped reader.

pub mod format;
pub mod reader;
pub mod writer;

pub use reader::Segment;
