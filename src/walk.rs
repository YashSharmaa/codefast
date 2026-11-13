//! Repository file discovery. Uses the same ignore rules as ripgrep, so the index covers the
//! files a harness's own search tools would see.

use crate::facts::fflag;
use std::path::Path;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug)]
pub struct WalkedFile {
    /// Path relative to the root, with `/` separators.
    pub rel: String,
    pub size: u64,
    pub mtime_ns: i64,
}

/// Directory names never worth indexing, even when no ignore file excludes them.
const SKIP_DIRS: &[&str] = &["node_modules", "__pycache__", ".git", ".codefast", ".hg", ".svn"];

/// Per-thread batch that flushes into the shared vector when full and when the thread finishes.
struct Collector<'a> {
    local: Vec<WalkedFile>,
    out: &'a Mutex<Vec<WalkedFile>>,
}

impl Drop for Collector<'_> {
    fn drop(&mut self) {
        if !self.local.is_empty() {
            self.out.lock().unwrap().append(&mut self.local);
        }
    }
}

/// Walk `root` in parallel and return its files sorted by path.
pub fn walk(root: &Path, threads: usize) -> Vec<WalkedFile> {
    let out: Mutex<Vec<WalkedFile>> = Mutex::new(Vec::new());
    let mut b = ignore::WalkBuilder::new(root);
    b.hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .parents(true)
        .require_git(false)
        .follow_links(false)
        .add_custom_ignore_filename(".codefastignore")
        .threads(threads)
        .filter_entry(|e| match e.file_name().to_str() {
            Some(name) => !SKIP_DIRS.contains(&name),
            None => true,
        });
    b.build_parallel().run(|| {
        let mut col = Collector { local: Vec::with_capacity(512), out: &out };
        Box::new(move |entry| {
            let Ok(e) = entry else { return ignore::WalkState::Continue };
            if !e.file_type().is_some_and(|t| t.is_file()) {
                return ignore::WalkState::Continue;
            }
            let Ok(md) = e.metadata() else { return ignore::WalkState::Continue };
            let Ok(rel) = e.path().strip_prefix(root) else { return ignore::WalkState::Continue };
            let Some(rel) = rel.to_str() else { return ignore::WalkState::Continue };
            let mtime_ns = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0);
            let rel = if std::path::MAIN_SEPARATOR == '/' { rel.to_string() } else { rel.replace(std::path::MAIN_SEPARATOR, "/") };
            col.local.push(WalkedFile { rel, size: md.len(), mtime_ns });
            if col.local.len() >= 512 {
                col.out.lock().unwrap().append(&mut col.local);
            }
            ignore::WalkState::Continue
        })
    });
    let mut files = out.into_inner().unwrap();
    files.sort_unstable_by(|a, b| a.rel.cmp(&b.rel));
    files
}

/// Classify a path: test / vendored / generated / documentation.
pub fn classify(rel: &str) -> u8 {
    let mut flags = 0u8;
    let lower = rel.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    for comp in lower.split('/') {
        match comp {
            "test" | "tests" | "testing" | "__tests__" | "spec" | "specs" | "testdata" | "e2e" | "unittests" | "unittest" | "selftests" | "kunit"
            | "fixtures" | "fixture" | "__fixtures__" | "__mocks__" => flags |= fflag::TEST,
            "vendor" | "third_party" | "third-party" | "thirdparty" | "node_modules" | "godeps" => flags |= fflag::VENDOR,
            "generated" | "gen" | "__generated__" => flags |= fflag::GENERATED,
            "docs" | "doc" | "documentation" => flags |= fflag::DOC,
            // A .NET test project is a directory named after the project it tests:
            // `Serilog.Tests`, `Foo.UnitTests`, `Foo.IntegrationTests`.
            c if c.ends_with(".tests") || c.ends_with(".test") || c.ends_with(".unittests") || c.ends_with(".integrationtests") || c.ends_with(".specs") => {
                flags |= fflag::TEST
            }
            _ => {}
        }
    }
    // Gradle source sets of tests: `commonTest`, `jvmTest`, `androidUnitTest`, `integrationTest`.
    if rel.split('/').any(|c| c.len() > 4 && c.ends_with("Test") && c.as_bytes()[c.len() - 5].is_ascii_lowercase()) {
        flags |= fflag::TEST;
    }
    let stem = file.split('.').next().unwrap_or(file);
    if stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.ends_with("-test")
        || file.contains(".test.")
        || file.contains(".spec.")
        || file.contains("_spec.")
        || (rel.ends_with("Test.java")
            || rel.ends_with("Tests.java")
            || rel.ends_with("Test.kt")
            || rel.ends_with("Tests.cs")
            || rel.ends_with("Test.cs")
            || rel.ends_with("Test.php"))
    {
        flags |= fflag::TEST;
    }
    if file.ends_with(".pb.go")
        || file.ends_with(".pb.cc")
        || file.ends_with(".pb.h")
        || file.ends_with("_pb2.py")
        || file.ends_with(".min.js")
        || file.ends_with(".g.dart")
        || file.contains("_generated.")
        || file.contains(".generated.")
        || file.contains(".gen.")
        || file.starts_with("zz_generated")
    {
        flags |= fflag::GENERATED;
    }
    if matches!(file.rsplit('.').next(), Some("md" | "rst" | "txt" | "adoc")) {
        flags |= fflag::DOC;
    }
    flags
}
