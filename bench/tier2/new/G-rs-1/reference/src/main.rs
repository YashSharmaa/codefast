use std::process::ExitCode;

use verso::{resolve, Constraint, Error, Registry, Version};

const USAGE: &str = "usage: verso match VERSION CONSTRAINT | max CONSTRAINT VERSION... | sort VERSION... | resolve FILE NAME@CONSTRAINT...";

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

fn parse_versions(args: &[String]) -> Result<Vec<Version>, Error> {
    args.iter().map(|a| Version::parse(a)).collect()
}

fn run(args: &[String]) -> Result<String, Failure> {
    let Some(cmd) = args.first() else { return Err(Failure::Usage) };
    let rest = &args[1..];
    let mut out = String::new();
    match cmd.as_str() {
        "match" if rest.len() == 2 => {
            let v = Version::parse(&rest[0])?;
            let c = Constraint::parse(&rest[1])?;
            out.push_str(if c.matches(&v) { "yes\n" } else { "no\n" });
        }
        "max" if !rest.is_empty() => {
            let c = Constraint::parse(&rest[0])?;
            let versions = parse_versions(&rest[1..])?;
            match c.max_satisfying(&versions) {
                Some(v) => out.push_str(&format!("{v}\n")),
                None => out.push_str("none\n"),
            }
        }
        "sort" => {
            let mut versions = parse_versions(rest)?;
            versions.sort();
            for v in versions {
                out.push_str(&format!("{v}\n"));
            }
        }
        "resolve" if !rest.is_empty() => {
            let text = std::fs::read_to_string(&rest[0])
                .map_err(|_| Failure::Message(format!("cannot read {}", rest[0])))?;
            let registry = Registry::parse(&text)?;
            let mut root = Vec::new();
            for arg in &rest[1..] {
                let (name, cons) = arg
                    .split_once('@')
                    .ok_or_else(|| Failure::Message(format!("invalid requirement: {arg}")))?;
                root.push((name.to_string(), Constraint::parse(cons)?));
            }
            for (name, v) in resolve(&registry, &root)? {
                out.push_str(&format!("{name} {v}\n"));
            }
        }
        _ => return Err(Failure::Usage),
    }
    Ok(out)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
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
            ExitCode::from(if e == Error::NoSolution { 1 } else { 2 })
        }
    }
}
