//! Index build pipeline: walk -> parallel extract -> write segment.

use crate::extract::Scratch;
use crate::facts::{Def, FileFacts, FileOut, Import, NONE, Ref, fflag};
use crate::index::writer::{SegmentStats, write_segment};
use crate::interner::Interner;
use crate::lang::{self, Lang, Parsers};
use crate::walk::{self, WalkedFile};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const INDEX_DIR: &str = ".codefast";
pub const BASE_SEGMENT: &str = "base.cfseg";
pub const OVERLAY_SEGMENT: &str = "overlay.cfseg";

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Files larger than this are not parsed (their text is still indexed).
    pub max_file_size: u64,
    /// Files larger than this are listed only.
    pub max_text_size: u64,
    pub threads: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions { max_file_size: 4 << 20, max_text_size: 16 << 20, threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) }
    }
}

#[derive(Default, Debug, Clone)]
pub struct BuildReport {
    pub stats: SegmentStats,
    pub walk: Duration,
    pub extract: Duration,
    pub write: Duration,
    pub total: Duration,
}

/// Where the index of `root` lives. `CODEFAST_INDEX_DIR` overrides the in-repo default.
pub fn index_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CODEFAST_INDEX_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => root.join(INDEX_DIR),
    }
}

/// Create the index directory, self-ignored so it never shows up in `git status`.
pub fn ensure_index_dir(root: &Path) -> Result<PathBuf> {
    let dir = index_dir(root);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let gi = dir.join(".gitignore");
    if !gi.exists() {
        let _ = std::fs::write(&gi, "*\n");
    }
    Ok(dir)
}

struct Worker {
    parsers: Parsers,
    facts: FileFacts,
    scratch: Scratch,
    tr: Vec<u32>,
    counts: Vec<u32>,
}

/// Extract one file into owned, globally-interned facts.
fn process(w: &mut Worker, root: &Path, f: &WalkedFile, interner: &Interner, opts: &BuildOptions) -> FileOut {
    let lang = Lang::from_path(&f.rel);
    let mut out = FileOut {
        path: f.rel.clone(),
        size: f.size,
        mtime_ns: f.mtime_ns,
        hash: 0,
        lang: lang as u8,
        flags: walk::classify(&f.rel),
        nlines: 0,
        parse_errors: 0,
        defs: Vec::new(),
        refs: Vec::new(),
        imports: Vec::new(),
        words: Vec::new(),
    };
    if Lang::is_header(&f.rel) {
        out.flags |= fflag::HEADER;
    }
    if f.size > opts.max_text_size || crate::text::skip_text(&f.rel) {
        out.flags |= fflag::UNPARSED;
        path_words(&f.rel, interner, &mut out.words);
        return out;
    }
    let src = match std::fs::read(root.join(&f.rel)) {
        Ok(s) => s,
        Err(_) => {
            out.flags |= fflag::UNPARSED;
            return out;
        }
    };
    if memchr::memchr(0, &src[..src.len().min(4096)]).is_some() {
        out.flags |= fflag::UNPARSED;
        return out;
    }
    out.hash = xxhash_rust::xxh3::xxh3_64(&src);
    // A stray NUL deep inside a text file (a test fixture in a string literal) stops a lexer
    // where it stands. A space of the same width keeps every position.
    let mut src = src;
    if memchr::memchr(0, &src).is_some() {
        for b in src.iter_mut().filter(|b| **b == 0) {
            *b = b' ';
        }
    }
    let lang = lang::detect(&f.rel, &src);
    out.lang = lang as u8;
    let parsed = lang.has_extractor() && f.size <= opts.max_file_size && lang::extract(lang, &src, &f.rel, &mut w.parsers, &mut w.facts, &mut w.scratch);
    if !parsed {
        w.facts.clear();
        w.facts.nlines = lang::count_lines(&src);
        out.flags |= fflag::UNPARSED;
    }
    // A JVM file that declares `@Test` functions is test code wherever it lives: Kotlin keeps
    // tests in source sets such as `src/jvmTest`, and fixtures sit next to production code.
    if parsed && matches!(lang, Lang::Java | Lang::Kotlin) && w.facts.defs.iter().any(|d| d.flags & crate::facts::dflag::TEST != 0) {
        out.flags |= fflag::TEST;
    }
    out.nlines = w.facts.nlines;
    out.parse_errors = w.facts.parse_errors;
    // The text side: every word in the file, whatever the language.
    w.counts.clear();
    crate::text::count_words(&src, &mut w.facts.names, &mut w.counts);
    // Translate file-local name ids to interner ids, interning only names that are emitted.
    w.tr.clear();
    w.tr.resize(w.facts.names.len(), NONE);
    let names = &w.facts.names;
    let tr = &mut w.tr;
    let mut g = |id: u32| -> u32 {
        if id == NONE {
            return NONE;
        }
        let slot = &mut tr[id as usize];
        if *slot == NONE {
            *slot = interner.intern(names.get(id));
        }
        *slot
    };
    out.defs = w.facts.defs.iter().map(|d| Def { name: g(d.name), qual: g(d.qual), type_name: g(d.type_name), ..*d }).collect();
    out.refs = w.facts.refs.iter().map(|r| Ref { name: g(r.name), qual: g(r.qual), ..*r }).collect();
    out.imports = w.facts.imports.iter().map(|i| Import { target: g(i.target), name: g(i.name), alias: g(i.alias), ..*i }).collect();
    out.words = w.counts.iter().enumerate().filter(|&(_, &c)| c > 0).map(|(i, &c)| (g(i as u32), c.min(255) as u8)).collect();
    path_words(&f.rel, interner, &mut out.words);
    out
}

/// Path components are indexed as words with a `/` prefix (which no identifier can carry), so
/// "files whose path mentions ext4" is a posting-list lookup.
fn path_words(rel: &str, interner: &Interner, words: &mut Vec<(u32, u8)>) {
    let mut buf: Vec<u8> = Vec::with_capacity(32);
    let bytes = rel.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        let s = i;
        while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
            i += 1;
        }
        if i - s >= 2 && i - s <= 48 {
            buf.clear();
            buf.push(b'/');
            buf.extend(bytes[s..i].iter().map(|b| b.to_ascii_lowercase()));
            let id = interner.intern(&buf);
            if !words.iter().any(|&(w, _)| w == id) {
                words.push((id, 1));
            }
        }
    }
}

/// Extract a set of files in parallel. The result is in the same order as `files`.
pub fn extract_files(root: &Path, files: &[WalkedFile], interner: &Interner, opts: &BuildOptions) -> Vec<FileOut> {
    files
        .par_iter()
        .map_init(
            || Worker { parsers: Parsers::default(), facts: FileFacts::default(), scratch: Scratch::default(), tr: Vec::new(), counts: Vec::new() },
            |w, f| process(w, root, f, interner, opts),
        )
        .collect()
}

/// Build the base segment for `root` from scratch.
pub fn build_base(root: &Path, opts: &BuildOptions) -> Result<BuildReport> {
    let t0 = Instant::now();
    let dir = ensure_index_dir(root)?;
    let files = walk::walk(root, opts.threads);
    let walk_t = t0.elapsed();

    let t1 = Instant::now();
    let interner = Interner::new();
    let outs = extract_files(root, &files, &interner, opts);
    let extract_t = t1.elapsed();

    let t2 = Instant::now();
    let names = interner.finalize();
    let root_str = root.to_string_lossy();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let build_id = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(format!("{root_str}:{nanos}:{}", std::process::id()).as_bytes()));
    let stats = write_segment(&dir.join(BASE_SEGMENT), &root_str, &outs, &names, &[("tool", env!("CARGO_PKG_VERSION").to_string()), ("build_id", build_id)])?;
    let _ = std::fs::remove_file(dir.join(OVERLAY_SEGMENT));
    let write_t = t2.elapsed();
    Ok(BuildReport { stats, walk: walk_t, extract: extract_t, write: write_t, total: t0.elapsed() })
}

/// Configure the global thread pool: deep syntax trees need more stack than the default.
pub fn init_thread_pool(threads: usize) {
    let _ = rayon::ThreadPoolBuilder::new().num_threads(threads).stack_size(32 << 20).build_global();
}
