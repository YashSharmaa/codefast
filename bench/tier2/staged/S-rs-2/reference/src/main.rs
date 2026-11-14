use std::path::Path;
use std::process::ExitCode;

use patchwork::{
    apply_patch_fuzzy, format_patches, make_patch, merge3, parse_patch, parse_patches, reverse_patch, Error, HunkLine,
    Patch, Whitespace,
};

const USAGE: &str = "usage: patchwork diff OLD NEW [-U N] [-Z|-w] | apply FILE PATCH [-F N] | reverse PATCH | stat PATCH | merge BASE OURS THEIRS [-Z|-w]";

enum Failure {
    Usage,
    Message(String),
    Lib(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Failure::Lib(e)
    }
}

struct Output {
    stdout: String,
    stderr: String,
    code: u8,
}

fn read(path: &str) -> Result<String, Failure> {
    std::fs::read_to_string(path).map_err(|_| Failure::Message(format!("cannot read {path}")))
}

fn read_patch(path: &str) -> Result<Patch, Failure> {
    Ok(parse_patch(&read(path)?)?)
}

fn read_patches(path: &str) -> Result<Vec<Patch>, Failure> {
    Ok(parse_patches(&read(path)?)?)
}

/// The text of a file, or the empty text when nothing exists at `path`.
fn read_or_empty(path: &str) -> Result<String, Failure> {
    if Path::new(path).exists() { read(path) } else { Ok(String::new()) }
}

/// Adds the relative paths (joined with `/`) of the regular files under `dir`.
fn files_under(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), Failure> {
    let unreadable = || Failure::Message(format!("cannot read {}", dir.display()));
    for entry in std::fs::read_dir(dir).map_err(|_| unreadable())? {
        let entry = entry.map_err(|_| unreadable())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let kind = entry.file_type().map_err(|_| unreadable())?;
        if kind.is_dir() {
            files_under(&entry.path(), &rel, out)?;
        } else if kind.is_file() {
            out.push(rel);
        }
    }
    Ok(())
}

/// Diffs two files, or two directory trees file by file. Only patches with
/// hunks are returned.
fn diff_paths(old: &str, new: &str, context: usize, ws: Whitespace) -> Result<Vec<Patch>, Failure> {
    let mut pairs = Vec::new();
    if Path::new(old).is_dir() && Path::new(new).is_dir() {
        let mut rels = Vec::new();
        files_under(Path::new(old), "", &mut rels)?;
        files_under(Path::new(new), "", &mut rels)?;
        rels.sort();
        rels.dedup();
        for rel in rels {
            let (a, b) = (format!("{old}/{rel}"), format!("{new}/{rel}"));
            let (a_text, b_text) = (read_or_empty(&a)?, read_or_empty(&b)?);
            pairs.push((a, a_text, b, b_text));
        }
    } else {
        pairs.push((old.to_string(), read(old)?, new.to_string(), read(new)?));
    }
    Ok(pairs
        .into_iter()
        .map(|(a, a_text, b, b_text)| make_patch(&a, &a_text, &b, &b_text, context, ws))
        .filter(|p| !p.hunks.is_empty())
        .collect())
}

/// A non-negative decimal number, digits only.
fn number(s: &str) -> Option<usize> {
    s.parse::<usize>().ok().filter(|_| s.bytes().all(|b| b.is_ascii_digit()))
}

/// Reads the options after the file arguments. `-U N` is allowed only when
/// `context` is given. Each option may appear once, and `-Z` and `-w` exclude each other.
fn options(rest: &[&str], mut context: Option<&mut usize>) -> Result<Whitespace, Failure> {
    let mut ws = None;
    let mut seen_context = false;
    let mut i = 0;
    while i < rest.len() {
        match (rest[i], rest.get(i + 1), context.as_deref_mut()) {
            ("-U", Some(n), Some(c)) if !seen_context => {
                *c = number(n).ok_or_else(|| Failure::Message(format!("invalid context: {n}")))?;
                seen_context = true;
                i += 1;
            }
            ("-Z", _, _) if ws.is_none() => ws = Some(Whitespace::IgnoreTrailing),
            ("-w", _, _) if ws.is_none() => ws = Some(Whitespace::IgnoreAll),
            _ => return Err(Failure::Usage),
        }
        i += 1;
    }
    Ok(ws.unwrap_or(Whitespace::Exact))
}

fn ok(stdout: String) -> Output {
    Output { stdout, stderr: String::new(), code: 0 }
}

fn run(args: &[String]) -> Result<Output, Failure> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["diff", old, new, rest @ ..] => {
            let mut context = 3;
            let ws = options(rest, Some(&mut context))?;
            let patches = diff_paths(old, new, context, ws)?;
            if patches.is_empty() {
                return Ok(ok(String::new()));
            }
            Ok(Output { stdout: format_patches(&patches), stderr: String::new(), code: 1 })
        }
        ["apply", file, patch, rest @ ..] => {
            let max_fuzz = match rest {
                [] => 0,
                ["-F", n] => number(n).ok_or_else(|| Failure::Message(format!("invalid fuzz: {n}")))?,
                _ => return Err(Failure::Usage),
            };
            let text = read(file)?;
            let patch = read_patch(patch)?;
            let applied = apply_patch_fuzzy(&text, &patch, max_fuzz)?;
            let mut stderr = String::new();
            for (i, (off, fuzz)) in applied.offsets.iter().zip(&applied.fuzz).enumerate() {
                if *fuzz != 0 {
                    stderr.push_str(&format!("hunk {} offset {} fuzz {}\n", i + 1, off, fuzz));
                } else if *off != 0 {
                    stderr.push_str(&format!("hunk {} offset {}\n", i + 1, off));
                }
            }
            Ok(Output { stdout: applied.text, stderr, code: 0 })
        }
        ["reverse", patch] => {
            let reversed: Vec<Patch> = read_patches(patch)?.iter().map(reverse_patch).collect();
            Ok(ok(format_patches(&reversed)))
        }
        ["stat", patch] => {
            let mut out = String::new();
            for p in read_patches(patch)? {
                let lines = p.hunks.iter().flat_map(|h| h.lines.iter());
                let added = lines.clone().filter(|l| matches!(l, HunkLine::Added(_))).count();
                let removed = lines.filter(|l| matches!(l, HunkLine::Removed(_))).count();
                out.push_str(&format!(
                    "{} -> {}: {} hunks, {} added, {} removed\n",
                    p.old_name,
                    p.new_name,
                    p.hunks.len(),
                    added,
                    removed
                ));
            }
            Ok(ok(out))
        }
        ["merge", base, ours, theirs, rest @ ..] => {
            let ws = options(rest, None)?;
            let (base, ours, theirs) = (read(base)?, read(ours)?, read(theirs)?);
            let merged = merge3(&base, &ours, &theirs, ws);
            if merged.conflicts == 0 {
                return Ok(ok(merged.text));
            }
            let stderr = format!("conflicts: {}\n", merged.conflicts);
            Ok(Output { stdout: merged.text, stderr, code: 1 })
        }
        _ => Err(Failure::Usage),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(out) => {
            print!("{}", out.stdout);
            eprint!("{}", out.stderr);
            ExitCode::from(out.code)
        }
        Err(Failure::Usage) => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Message(m)) => {
            eprintln!("error: {m}");
            ExitCode::from(2)
        }
        Err(Failure::Lib(e)) => {
            eprintln!("error: {e}");
            ExitCode::from(if matches!(e, Error::HunkFailed(_)) { 1 } else { 2 })
        }
    }
}
