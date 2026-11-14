use std::process::ExitCode;

use patchwork::{apply_patch, format_patch, make_patch, parse_patch, reverse_patch, Error, Patch};

const USAGE: &str = "usage: patchwork diff OLD NEW [-U N] | apply FILE PATCH | reverse PATCH | stat PATCH";

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

fn ok(stdout: String) -> Output {
    Output { stdout, stderr: String::new(), code: 0 }
}

fn run(args: &[String]) -> Result<Output, Failure> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["diff", old, new, rest @ ..] => {
            let context = match rest {
                [] => 3,
                ["-U", n] => n
                    .parse::<usize>()
                    .ok()
                    .filter(|_| n.bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(|| Failure::Message(format!("invalid context: {n}")))?,
                _ => return Err(Failure::Usage),
            };
            let (a, b) = (read(old)?, read(new)?);
            let patch = make_patch(old, &a, new, &b, context);
            if patch.hunks.is_empty() {
                return Ok(ok(String::new()));
            }
            Ok(Output { stdout: format_patch(&patch), stderr: String::new(), code: 1 })
        }
        ["apply", file, patch] => {
            let text = read(file)?;
            let patch = read_patch(patch)?;
            let applied = apply_patch(&text, &patch)?;
            let mut stderr = String::new();
            for (i, off) in applied.offsets.iter().enumerate() {
                if *off != 0 {
                    stderr.push_str(&format!("hunk {} offset {}\n", i + 1, off));
                }
            }
            Ok(Output { stdout: applied.text, stderr, code: 0 })
        }
        ["reverse", patch] => Ok(ok(format_patch(&reverse_patch(&read_patch(patch)?)))),
        ["stat", patch] => {
            let p = read_patch(patch)?;
            let lines = p.hunks.iter().flat_map(|h| h.lines.iter());
            let added = lines.clone().filter(|l| matches!(l, patchwork::HunkLine::Added(_))).count();
            let removed = lines.filter(|l| matches!(l, patchwork::HunkLine::Removed(_))).count();
            Ok(ok(format!(
                "{} -> {}: {} hunks, {} added, {} removed\n",
                p.old_name,
                p.new_name,
                p.hunks.len(),
                added,
                removed
            )))
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
