//! Opening an index that is always current.
//!
//! Every open compares the working tree with the base segment by (path, size, mtime). Files that
//! changed or appeared are extracted into a small overlay segment; base files that changed or
//! vanished are marked dead. The overlay is persisted so the next invocation reuses it as long
//! as the tree has not moved again. When too much has changed (a branch switch), the base is
//! rebuilt instead: that costs seconds even on the largest repositories.

use crate::build::{self, BASE_SEGMENT, BuildOptions, OVERLAY_SEGMENT};
use crate::index::Segment;
use crate::index::writer::encode_segment;
use crate::interner::Interner;
use crate::store::Index;
use crate::walk::{self, WalkedFile};
use anyhow::{Context, Result};
use std::path::Path;

pub struct OpenOptions {
    /// Compare the working tree with the index (default). Off: trust the index as it is.
    pub refresh: bool,
    pub quiet: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions { refresh: std::env::var_os("CODEFAST_NO_REFRESH").is_none(), quiet: false }
    }
}

struct Diff {
    /// Changed or new files, with their current metadata.
    changed: Vec<WalkedFile>,
    /// Local ids of base files that changed or no longer exist.
    dead: Vec<u32>,
}

fn diff(base: &Segment, walked: &[WalkedFile]) -> Diff {
    let files = base.files();
    let mut d = Diff { changed: Vec::new(), dead: Vec::new() };
    let (mut i, mut j) = (0usize, 0usize);
    while i < files.len() || j < walked.len() {
        let ord = match (i < files.len(), j < walked.len()) {
            (true, true) => base.path(i as u32).cmp(walked[j].rel.as_str()),
            (true, false) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Greater,
        };
        match ord {
            std::cmp::Ordering::Less => {
                d.dead.push(i as u32); // deleted
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                d.changed.push(walked[j].clone()); // new
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                let (f, w) = (&files[i], &walked[j]);
                if f.size != w.size || f.mtime_ns != w.mtime_ns {
                    d.dead.push(i as u32);
                    d.changed.push(w.clone());
                }
                i += 1;
                j += 1;
            }
        }
    }
    d
}

/// Whether a base segment was written by this version of the program.
pub fn built_by_this_version(seg: &Segment) -> bool {
    seg.meta("tool") == Some(env!("CARGO_PKG_VERSION"))
}

fn open_base(root: &Path, opts: &OpenOptions) -> Result<Segment> {
    let path = build::index_dir(root).join(BASE_SEGMENT);
    if path.exists() {
        if let Ok(seg) = Segment::open(&path) {
            // An index directory named by the environment can end up shared by two
            // repositories. An index built for another root is rebuilt, never served. So is
            // one built by another version of this program: its extractors may have read the
            // same files differently, and a rebuild costs seconds.
            if seg.meta("root").is_none_or(|r| Path::new(r) == root) && built_by_this_version(&seg) {
                return Ok(seg);
            }
        }
    }
    if !opts.quiet {
        eprintln!("codefast: indexing {} ...", root.display());
    }
    let r = build::build_base(root, &BuildOptions::default())?;
    if !opts.quiet {
        eprintln!("codefast: indexed {} files, {} symbols in {:.1?}", r.stats.files, r.stats.defs, r.total);
    }
    Segment::open(&path)
}

/// Does a persisted overlay describe exactly this diff?
fn overlay_matches(overlay: &Segment, base_id: &str, d: &Diff) -> bool {
    if overlay.meta("base") != Some(base_id) || overlay.n_files() as usize != d.changed.len() {
        return false;
    }
    let files = overlay.files();
    for (i, w) in d.changed.iter().enumerate() {
        if overlay.path(i as u32) != w.rel || files[i].size != w.size || files[i].mtime_ns != w.mtime_ns {
            return false;
        }
    }
    let dead: &[u32] = match overlay.section(b"DEAD") {
        Some(b) if b.len() % 4 == 0 => bytemuck::cast_slice(b),
        Some(_) => return false,
        None => &[],
    };
    dead == d.dead.as_slice()
}

/// Open the index of `root`, building or refreshing whatever is needed.
pub fn open(root: &Path, opts: &OpenOptions) -> Result<Index> {
    let root = root.canonicalize().with_context(|| format!("no such directory: {}", root.display()))?;
    let mut base = open_base(&root, opts)?;
    if !opts.refresh {
        return Ok(Index::assemble(root, base, None, &[]));
    }
    let bopts = BuildOptions::default();
    let walked = walk::walk(&root, bopts.threads);
    let mut d = diff(&base, &walked);
    if d.changed.is_empty() && d.dead.is_empty() {
        return Ok(Index::assemble(root, base, None, &[]));
    }
    // A large change set is cheaper to absorb by rebuilding the base.
    let total = base.n_files() as usize;
    let moved = d.changed.len() + d.dead.len();
    if moved > 4000 || (moved > 200 && moved * 4 > total.max(1)) {
        if !opts.quiet {
            eprintln!("codefast: {moved} files changed, rebuilding the index ...");
        }
        build::build_base(&root, &bopts)?;
        base = Segment::open(&build::index_dir(&root).join(BASE_SEGMENT))?;
        d = diff(&base, &walk::walk(&root, bopts.threads));
        if d.changed.is_empty() && d.dead.is_empty() {
            return Ok(Index::assemble(root, base, None, &[]));
        }
    }
    let base_id = base.meta("build_id").unwrap_or("").to_string();
    let dir = build::index_dir(&root);
    let opath = dir.join(OVERLAY_SEGMENT);
    if let Ok(o) = Segment::open(&opath) {
        if overlay_matches(&o, &base_id, &d) {
            return Ok(Index::assemble(root, base, Some(o), &d.dead));
        }
    }
    // Extract the changed files into a fresh overlay.
    let interner = Interner::new();
    let outs = build::extract_files(&root, &d.changed, &interner, &bopts);
    let names = interner.finalize();
    let dead_bytes: &[u8] = bytemuck::cast_slice(&d.dead);
    let (_, bytes) = encode_segment(&root.to_string_lossy(), &outs, &names, &[("base", base_id)], &[(b"DEAD", dead_bytes)])?;
    // Persist for the next invocation; failure to write (read-only tree) is not an error.
    let tmp = opath.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, &bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &opath);
    }
    let overlay = Segment::from_bytes(bytes)?;
    Ok(Index::assemble(root, base, Some(overlay), &d.dead))
}
