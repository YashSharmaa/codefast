//! `trace`: a pasted stack trace in, the frames that live in this repository out, each with its
//! location, signature and the line that was executing. Understands kernel oopses, Python
//! tracebacks, Java / Kotlin / C# stack traces, Node, Go panics, Rust backtraces, gdb and
//! sanitizer output, and bare `path:line` mentions.

use super::show::{line_starts, line_text};
use super::{Common, sig_of};
use crate::output::{Out, clip};
use crate::resolve::{Class, Resolver, class_of, common_prefix_dirs};
use crate::store::{DefId, FileId, Index};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub func: Option<String>,
    /// Owner (class, receiver type or module) when the trace names one.
    pub owner: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// Offset and size from `func+0x1a/0x60`, as a fraction of the function.
    pub frac: Option<f32>,
    pub raw: String,
}

struct Pats {
    python: Regex,
    java: Regex,
    csharp: Regex,
    js_named: Regex,
    js_anon: Regex,
    gdb: Regex,
    kernel: Regex,
    ruby: Regex,
    at_loc: Regex,
    rust_sym: Regex,
    go_func: Regex,
    go_loc: Regex,
    path_line: Regex,
}

fn pats() -> &'static Pats {
    static P: OnceLock<Pats> = OnceLock::new();
    P.get_or_init(|| Pats {
        python: Regex::new(r#"File "([^"]+)", line (\d+)(?:, in (\S+))?"#).unwrap(),
        java: Regex::new(r"\bat ([\w$.]+)\.([\w$<>]+)\(([\w$.-]+\.\w+):(\d+)\)").unwrap(),
        csharp: Regex::new(r"\bat ([\w.<>`]+)\.([\w<>`]+)\(.*?\) in (.+?):line (\d+)").unwrap(),
        js_named: Regex::new(r"\bat (?:async )?([\w$.<>\[\]# ]+?) \((?:file://)?([^()]+?):(\d+):\d+\)").unwrap(),
        js_anon: Regex::new(r"\bat (?:file://)?([^()\s]+?):(\d+):\d+\s*$").unwrap(),
        gdb: Regex::new(r"#\d+\s+(?:0x[0-9a-fA-F]+\s+in\s+)?([\w:~<>.]+)\s*(?:\(.*?\))?\s+(?:at\s+|from\s+)?([\w./+-]+\.\w+):(\d+)").unwrap(),
        kernel: Regex::new(r"(?:\?\s+)?\b([A-Za-z_][\w.]*)\+0x([0-9a-fA-F]+)/0x([0-9a-fA-F]+)").unwrap(),
        ruby: Regex::new(r"([\w./-]+\.rb):(\d+):in [`']([^']+)'").unwrap(),
        at_loc: Regex::new(r"^\s+at (?:\./)?([^\s:]+\.\w+):(\d+)(?::\d+)?\s*$").unwrap(),
        rust_sym: Regex::new(r"^\s*\d+:\s+(?:0x[0-9a-f]+ - )?([\w:<>{}\[\], $'&*-]+?)\s*$").unwrap(),
        go_func: Regex::new(r"^([\w./-]+(?:\.\(\*?[\w.]+\))?\.[\w.]+)\(.*\)\s*$").unwrap(),
        go_loc: Regex::new(r"^\s+(/?[\w./@+-]+\.go):(\d+)").unwrap(),
        path_line: Regex::new(r"((?:[\w.+-]+/)*[\w.+-]+\.(?:c|h|cc|cpp|cxx|hpp|py|js|jsx|ts|tsx|go|rs|java|kt|rb|cs|swift|php|scala|m|mm)):(\d+)").unwrap(),
    })
}

/// Split `a.b.Class.method`, `a::b::Type::method`, `pkg.(*T).Method` into (owner, name).
fn split_owner(sym: &str) -> (Option<String>, String) {
    let s = sym.trim().trim_end_matches("()");
    // Rust closures and hashes: `foo::bar::{{closure}}::h1234abcd`
    let mut parts: Vec<&str> = s.split("::").flat_map(|p| p.split('.')).filter(|p| !p.is_empty()).collect();
    while let Some(last) = parts.last() {
        if last.starts_with("{{") || last.starts_with("{closure") || (last.len() == 17 && last.starts_with('h')) || *last == "func1" || last.starts_with("<") {
            parts.pop();
        } else {
            break;
        }
    }
    let clean = |p: &str| p.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_')).to_string();
    match parts.len() {
        0 => (None, s.to_string()),
        1 => (None, clean(parts[0])),
        n => (Some(clean(parts[n - 2])).filter(|o| !o.is_empty()), clean(parts[n - 1])),
    }
}

/// Normalise a kernel symbol: compiler clones and per-arch syscall wrappers.
fn kernel_name(s: &str) -> String {
    let mut n = s;
    for suffix in [".cold", ".isra", ".constprop", ".part", ".llvm", ".lto_priv"] {
        if let Some(i) = n.find(suffix) {
            n = &n[..i];
        }
    }
    for prefix in ["__x64_sys_", "__ia32_sys_", "__arm64_sys_", "__se_sys_", "__do_sys_", "__riscv_sys_", "__s390x_sys_"] {
        if let Some(rest) = n.strip_prefix(prefix) {
            return format!("sys_{rest}");
        }
    }
    for prefix in ["__x64_compat_sys_", "__ia32_compat_sys_", "__arm64_compat_sys_", "__se_compat_sys_", "__do_compat_sys_"] {
        if let Some(rest) = n.strip_prefix(prefix) {
            return format!("compat_sys_{rest}");
        }
    }
    n.to_string()
}

/// Extract frames from free text, in the order they appear.
pub fn parse(text: &str) -> Vec<Frame> {
    let p = pats();
    let mut frames: Vec<Frame> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let l = lines[i];
        i += 1;
        if let Some(c) = p.python.captures(l) {
            frames.push(Frame {
                file: Some(c[1].to_string()),
                line: c[2].parse().ok(),
                func: c.get(3).map(|m| m.as_str().to_string()).filter(|f| f != "<module>"),
                raw: l.trim().to_string(),
                ..Default::default()
            });
            continue;
        }
        if let Some(c) = p.csharp.captures(l) {
            let (_, owner) = split_owner(&c[1]);
            frames.push(Frame {
                func: Some(c[2].to_string()),
                owner: Some(owner),
                file: Some(c[3].replace('\\', "/")),
                line: c[4].parse().ok(),
                raw: l.trim().to_string(),
                ..Default::default()
            });
            continue;
        }
        if let Some(c) = p.java.captures(l) {
            let (_, owner) = split_owner(&c[1]);
            let owner = owner.split('$').next().unwrap_or(&owner).to_string();
            frames.push(Frame {
                func: Some(c[2].to_string()).filter(|f| f != "<init>" && f != "<clinit>"),
                owner: Some(owner),
                file: Some(c[3].to_string()),
                line: c[4].parse().ok(),
                raw: l.trim().to_string(),
                ..Default::default()
            });
            continue;
        }
        if let Some(c) = p.ruby.captures(l) {
            frames.push(Frame {
                file: Some(c[1].to_string()),
                line: c[2].parse().ok(),
                func: Some(c[3].rsplit(['#', '.']).next().unwrap_or(&c[3]).to_string()),
                raw: l.trim().to_string(),
                ..Default::default()
            });
            continue;
        }
        if let Some(c) = p.js_named.captures(l) {
            let (owner, name) = split_owner(c[1].trim_start_matches("new ").trim_start_matches("Object."));
            frames.push(Frame {
                func: Some(name).filter(|f| f != "<anonymous>"),
                owner,
                file: Some(c[2].to_string()),
                line: c[3].parse().ok(),
                raw: l.trim().to_string(),
                ..Default::default()
            });
            continue;
        }
        if let Some(c) = p.js_anon.captures(l) {
            frames.push(Frame { file: Some(c[1].to_string()), line: c[2].parse().ok(), raw: l.trim().to_string(), ..Default::default() });
            continue;
        }
        if let Some(c) = p.gdb.captures(l) {
            let (owner, name) = split_owner(&c[1]);
            frames.push(Frame {
                func: Some(name),
                owner,
                file: Some(c[2].to_string()),
                line: c[3].parse().ok(),
                raw: l.trim().to_string(),
                ..Default::default()
            });
            continue;
        }
        if let Some(c) = p.kernel.captures(l) {
            let off = u64::from_str_radix(&c[2], 16).unwrap_or(0) as f32;
            let size = u64::from_str_radix(&c[3], 16).unwrap_or(0) as f32;
            frames.push(Frame {
                func: Some(kernel_name(&c[1])),
                frac: if size > 0.0 { Some((off / size).clamp(0.0, 1.0)) } else { None },
                raw: c[0].to_string(),
                ..Default::default()
            });
            continue;
        }
        // Two-line forms: symbol line, then an indented location line.
        if let Some(next) = lines.get(i) {
            if let Some(loc) = p.go_loc.captures(next) {
                if let Some(c) = p.go_func.captures(l) {
                    let (owner, name) = split_owner(&c[1]);
                    frames.push(Frame {
                        func: Some(name),
                        owner,
                        file: Some(loc[1].to_string()),
                        line: loc[2].parse().ok(),
                        raw: l.trim().to_string(),
                        ..Default::default()
                    });
                    i += 1;
                    continue;
                }
            }
            if let Some(loc) = p.at_loc.captures(next) {
                if let Some(c) = p.rust_sym.captures(l) {
                    let (owner, name) = split_owner(&c[1]);
                    frames.push(Frame {
                        func: Some(name),
                        owner,
                        file: Some(loc[1].to_string()),
                        line: loc[2].parse().ok(),
                        raw: l.trim().to_string(),
                        ..Default::default()
                    });
                    i += 1;
                    continue;
                }
            }
        }
        if let Some(c) = p.path_line.captures(l) {
            frames.push(Frame { file: Some(c[1].to_string()), line: c[2].parse().ok(), raw: l.trim().to_string(), ..Default::default() });
        }
    }
    frames
}

/// Find the indexed file a trace path refers to. Traces carry absolute paths from other
/// machines, so progressively shorter suffixes are tried until one matches a single file.
fn find_file(ix: &Index, path: &str, near: Option<&str>) -> Option<FileId> {
    let path = path.trim_start_matches("./").replace('\\', "/");
    let comps: Vec<&str> = path.split('/').filter(|c| !c.is_empty() && *c != "." && *c != "..").collect();
    for start in 0..comps.len() {
        let suffix = comps[start..].join("/");
        if let Some(f) = ix.file_by_path(&suffix) {
            return Some(f);
        }
        let tail = format!("/{suffix}");
        let hits: Vec<FileId> = ix.files().filter(|&f| ix.path(f).ends_with(&tail)).take(64).collect();
        match hits.len() {
            0 => continue,
            1 => return Some(hits[0]),
            _ => {
                // Several files share the suffix: prefer the one nearest the previous frame.
                return match near {
                    Some(n) => hits.into_iter().max_by_key(|&f| common_prefix_dirs(n, ix.path(f))),
                    None => Some(hits[0]),
                };
            }
        }
    }
    None
}

pub struct Resolved {
    pub frame: Frame,
    pub file: Option<FileId>,
    pub def: Option<DefId>,
    pub line: Option<u32>,
    pub approx: bool,
}

pub fn resolve(ix: &Index, rs: &Resolver, frames: &[Frame]) -> Vec<Resolved> {
    let mut out: Vec<Resolved> = Vec::new();
    let mut near: Option<String> = None;
    for fr in frames {
        let mut r = Resolved { frame: fr.clone(), file: None, def: None, line: fr.line, approx: false };
        if let Some(p) = &fr.file {
            r.file = find_file(ix, p, near.as_deref());
        }
        if let (Some(f), Some(l)) = (r.file, fr.line) {
            r.def = ix.def_at(f, l).map(|d| d.id);
        }
        if r.def.is_none() {
            if let Some(name) = &fr.func {
                let mut cands: Vec<DefId> =
                    ix.defs_named(name.as_bytes()).into_iter().filter(|&d| matches!(class_of(&ix.def(d)), Class::Callable) && !ix.def(d).is_decl()).collect();
                if let Some(f) = r.file {
                    let same: Vec<DefId> = cands.iter().copied().filter(|&d| ix.def(d).file() == f).collect();
                    if !same.is_empty() {
                        cands = same;
                    }
                }
                if let Some(o) = &fr.owner {
                    let owned: Vec<DefId> = cands
                        .iter()
                        .copied()
                        .filter(|&d| ix.def(d).parent_name().is_some_and(|p| p == o.as_bytes()) || ix.def(d).path().contains(o.as_str()))
                        .collect();
                    if !owned.is_empty() {
                        cands = owned;
                    }
                }
                // Among same-named functions, the one nearest the neighbouring frames.
                let best = cands.into_iter().max_by_key(|&d| {
                    let v = ix.def(d);
                    let prox = near.as_deref().map(|n| common_prefix_dirs(n, v.path()) as i64 * 100).unwrap_or(0);
                    prox + crate::resolve::def_rank(ix, &v)
                });
                if let Some(d) = best {
                    let v = ix.def(d);
                    r.def = Some(d);
                    r.file = Some(v.file());
                    if r.line.is_none() {
                        if let Some(frac) = fr.frac {
                            // The offset within the machine code approximates the position
                            // within the source of the function.
                            let span = (v.rec.end_line - v.rec.line) as f32;
                            r.line = Some(v.rec.line + (span * frac) as u32);
                            r.approx = true;
                        }
                    }
                }
            }
        }
        if let Some(f) = r.file {
            near = Some(ix.path(f).to_string());
        }
        out.push(r);
    }
    let _ = rs;
    out
}

pub fn trace(ix: &Index, rs: &Resolver, text: &str, _common: &Common, out: &mut Out) {
    let frames = parse(text);
    if frames.is_empty() {
        out.push("no stack frames recognised (expected a kernel oops, a Python / Java / C# / Node / Go / Rust trace, gdb or sanitizer output, or path:line mentions). For free text use `codefast locate`.");
        return;
    }
    let resolved = resolve(ix, rs, &frames);
    let hit = resolved.iter().filter(|r| r.def.is_some() || r.file.is_some()).count();
    out.push(&format!("{} frames, {} in this repository", resolved.len(), hit));
    let mut missing: Vec<String> = Vec::new();
    let mut last: Option<(Option<DefId>, Option<u32>)> = None;
    let mut detailed = 0usize;
    for (i, r) in resolved.iter().enumerate() {
        let Some(file) = r.file else {
            missing.push(r.frame.func.clone().or(r.frame.file.clone()).unwrap_or_else(|| clip(&r.frame.raw, 40)));
            continue;
        };
        // Recursion and duplicate frames print once.
        if last == Some((r.def, r.line)) {
            continue;
        }
        last = Some((r.def, r.line));
        let path = ix.path(file);
        let head = match (r.def, r.line) {
            (Some(d), Some(l)) => {
                let v = ix.def(d);
                format!("#{i} {} {}:{}{} (function {}-{})", v.name_str(), path, l, if r.approx { " (approximate)" } else { "" }, v.rec.line, v.rec.end_line)
            }
            (Some(d), None) => {
                let v = ix.def(d);
                format!("#{i} {} {}:{}-{}", v.name_str(), path, v.rec.line, v.rec.end_line)
            }
            (None, Some(l)) => format!("#{i} {path}:{l}"),
            (None, None) => format!("#{i} {path}"),
        };
        if !out.line(&head) {
            break;
        }
        // The first frames get their signature and the executing line; the rest stay one line.
        if detailed < 8 {
            detailed += 1;
            if let Some(d) = r.def {
                out.line(&format!("   {}", clip(&sig_of(&ix.def(d)), 160)));
            }
            if let (Some(l), false) = (r.line, r.approx) {
                if let Some(src) = ix.source(file) {
                    let starts = line_starts(&src);
                    for n in l.saturating_sub(1).max(1)..=l + 1 {
                        let t = String::from_utf8_lossy(line_text(&src, &starts, n));
                        if !t.trim().is_empty() || n == l {
                            out.line(&format!("   {}{}\t{}", if n == l { ">" } else { " " }, n, clip(&t, 160)));
                        }
                    }
                }
            }
        }
    }
    if !missing.is_empty() {
        missing.dedup();
        let total = missing.len();
        missing.truncate(12);
        let more = if total > missing.len() { format!(" +{}", total - missing.len()) } else { String::new() };
        out.push(&format!("not in this repository: {}{}", missing.join(", "), more));
    }
}
