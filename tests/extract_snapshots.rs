//! Snapshot tests for language extractors.
//!
//! Every source file under `tests/fixtures/<lang>/` has a sibling `<file>.facts` holding the
//! expected dump of its extracted facts. Run with `CODEFAST_BLESS=1` to (re)write snapshots
//! after reviewing a change by hand.

use std::path::{Path, PathBuf};

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(&root) else { return out };
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else { continue };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().is_some_and(|e| e == "facts") || !p.is_file() {
                continue;
            }
            out.push(p);
        }
    }
    out.sort();
    out
}

#[test]
fn extractor_snapshots() {
    let bless = std::env::var_os("CODEFAST_BLESS").is_some();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut failures = Vec::new();
    let mut checked = 0;
    for path in fixtures() {
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if !codefast::lang::Lang::from_path(&rel).has_extractor() {
            continue;
        }
        let src = std::fs::read(&path).unwrap();
        let got = codefast::debug::dump_facts(&rel, &src);
        let snap = PathBuf::from(format!("{}.facts", path.display()));
        if bless {
            std::fs::write(&snap, &got).unwrap();
            continue;
        }
        checked += 1;
        match std::fs::read_to_string(&snap) {
            Ok(want) if want == got => {}
            Ok(want) => {
                let diff = want.lines().zip(got.lines()).enumerate().find(|(_, (a, b))| a != b);
                let detail = match diff {
                    Some((i, (a, b))) => format!("first difference at line {}:\n  want: {a}\n  got:  {b}", i + 1),
                    None => format!("length differs: want {} lines, got {}", want.lines().count(), got.lines().count()),
                };
                failures.push(format!("{rel}: {detail}"));
            }
            Err(_) => failures.push(format!("{rel}: missing snapshot (run with CODEFAST_BLESS=1)")),
        }
    }
    assert!(failures.is_empty(), "{} of {} snapshots differ:\n{}", failures.len(), checked, failures.join("\n"));
}
