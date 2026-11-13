//! Ruby extractor.
//!
//! Not written yet: the grammar is wired in, so Ruby files are parsed, but no facts are
//! recorded for them. `docs/EXTRACTOR_GUIDE.md` says what an extractor has to emit.

use super::Lang;
use crate::extract::Scratch;
use crate::facts::FileFacts;
use tree_sitter::Tree;

pub fn extract(_lang: Lang, _src: &[u8], _tree: &Tree, _path: &str, _facts: &mut FileFacts, _scratch: &mut Scratch) {}
