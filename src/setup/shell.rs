//! Reading shell command lines without running them.
//!
//! The hooks see the command an agent handed to its shell tool. This module answers two
//! questions about such a command: does it search the tree for one pattern (`grep`, `rg`,
//! `git grep`, `ag`, `find -name`, `fd`), and does it print one whole file (`cat`, `nl`, `bat`)?
//! It is a reader, not an interpreter: nothing is ever executed or expanded, and whatever it
//! cannot follow exactly (expansions, here documents, process substitution) is left alone so
//! that the caller stays silent.

/// Longer command lines are not read at all.
const MAX_COMMAND_BYTES: usize = 32 << 10;
const MAX_WORDS: usize = 2048;
/// How deep `sh -c '...'` and `find -exec ...` are followed.
const MAX_DEPTH: u8 = 3;

/// One word of a command after quote removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub text: String,
    /// Part of the word comes from an expansion (`$VAR`, `$(...)`, backticks), so the text is
    /// not what the program receives.
    pub dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(Word),
    Pipe,
    /// `;`, `&&`, `||`, `&`, a newline or a grouping parenthesis.
    Sep,
    /// Standard output of the command goes to a file.
    RedirOut,
    RedirIn,
}

/// Where a command runs relative to the directory the shell started in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dir {
    /// The shell's own working directory.
    Start,
    /// After `cd PATH` (relative to the start, or absolute).
    At(String),
    /// After a `cd` that cannot be followed.
    Unknown,
}

/// A search for one pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    pub pattern: String,
    /// The pattern is a literal string (`grep -F`).
    pub fixed: bool,
    /// The pattern is matched against file names (`find -name`, `fd`), not file contents.
    pub by_name: bool,
    /// Files and directories the search was restricted to, as written.
    pub paths: Vec<String>,
    pub dir: Dir,
}

/// A command that prints one whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: String,
    pub dir: Dir,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Found {
    pub searches: Vec<Search>,
    pub reads: Vec<FileRead>,
}

struct Builder {
    text: String,
    dynamic: bool,
    quoted: bool,
}

/// Skip a `$...` expansion starting at `cs[i] == '$'`. Returns the index after it and whether
/// it was an expansion at all (a lone `$` is a literal character).
fn expansion(cs: &[char], i: usize) -> Option<(usize, bool)> {
    let Some(&next) = cs.get(i + 1) else { return Some((i + 1, false)) };
    match next {
        '(' => {
            let (mut depth, mut j) = (0usize, i + 1);
            while j < cs.len() {
                match cs[j] {
                    '\\' => j += 1,
                    '\'' => {
                        j += 1;
                        while j < cs.len() && cs[j] != '\'' {
                            j += 1;
                        }
                    }
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some((j + 1, true));
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        '{' => cs[i..].iter().position(|&c| c == '}').map(|p| (i + p + 1, true)),
        '\'' => {
            let mut j = i + 2;
            while j < cs.len() {
                match cs[j] {
                    '\\' => j += 1,
                    '\'' => return Some((j + 1, true)),
                    _ => {}
                }
                j += 1;
            }
            None
        }
        c if c.is_ascii_alphabetic() || c == '_' => {
            let mut j = i + 1;
            while j < cs.len() && (cs[j].is_ascii_alphanumeric() || cs[j] == '_') {
                j += 1;
            }
            Some((j, true))
        }
        c if c.is_ascii_digit() || "@*#?!$-".contains(c) => Some((i + 2, true)),
        _ => Some((i + 1, false)),
    }
}

fn backtick(cs: &[char], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < cs.len() {
        match cs[j] {
            '\\' => j += 1,
            '`' => return Some(j + 1),
            _ => {}
        }
        j += 1;
    }
    None
}

#[derive(Default)]
struct Lexer {
    toks: Vec<Tok>,
    cur: Option<Builder>,
    /// The word after a redirection operator is its target, not an argument.
    skip_word: bool,
    words: usize,
}

impl Lexer {
    fn flush(&mut self) {
        if let Some(b) = self.cur.take() {
            if self.skip_word {
                self.skip_word = false;
            } else {
                self.words += 1;
                self.toks.push(Tok::Word(Word { text: b.text, dynamic: b.dynamic }));
            }
        }
    }

    fn word(&mut self) -> &mut Builder {
        self.cur.get_or_insert_with(|| Builder { text: String::new(), dynamic: false, quoted: false })
    }

    fn op(&mut self, t: Tok) {
        self.flush();
        self.toks.push(t);
    }

    /// At a redirection operator: an all-digit word directly before it is its descriptor.
    fn descriptor(&mut self) -> Option<u32> {
        let fd = match &self.cur {
            Some(b) if !b.quoted && !b.dynamic && !b.text.is_empty() && b.text.bytes().all(|c| c.is_ascii_digit()) => b.text.parse::<u32>().ok(),
            _ => None,
        };
        if fd.is_some() {
            self.cur = None;
        } else {
            self.flush();
        }
        fd
    }

    /// An expansion or a literal `$` at `cs[i]`; returns the index after it.
    fn dollar(&mut self, cs: &[char], i: usize) -> Option<usize> {
        let (next, expanded) = expansion(cs, i)?;
        let b = self.word();
        if expanded {
            b.dynamic = true;
            b.text.extend(&cs[i..next]);
        } else {
            b.text.push('$');
        }
        Some(next)
    }

    fn command_substitution(&mut self, cs: &[char], i: usize) -> Option<usize> {
        let next = backtick(cs, i)?;
        let b = self.word();
        b.dynamic = true;
        b.text.extend(&cs[i..next]);
        Some(next)
    }
}

/// Split a command line into words and operators. `None` when the line uses a construct this
/// reader does not follow.
fn lex(src: &str) -> Option<Vec<Tok>> {
    if src.len() > MAX_COMMAND_BYTES {
        return None;
    }
    let cs: Vec<char> = src.chars().collect();
    let mut lx = Lexer::default();
    let mut i = 0usize;
    while i < cs.len() {
        let c = cs[i];
        match c {
            ' ' | '\t' | '\r' => {
                lx.flush();
                i += 1;
            }
            '\n' => {
                lx.op(Tok::Sep);
                i += 1;
            }
            '#' if lx.cur.is_none() => {
                while i < cs.len() && cs[i] != '\n' {
                    i += 1;
                }
            }
            '\\' => {
                match cs.get(i + 1) {
                    Some('\n') | None => {}
                    Some(&n) => {
                        let b = lx.word();
                        b.text.push(n);
                        b.quoted = true;
                    }
                }
                i += 2;
            }
            '\'' => {
                let close = cs[i + 1..].iter().position(|&x| x == '\'')? + i + 1;
                let b = lx.word();
                b.quoted = true;
                b.text.extend(&cs[i + 1..close]);
                i = close + 1;
            }
            '"' => {
                lx.word().quoted = true;
                i += 1;
                loop {
                    match *cs.get(i)? {
                        '"' => {
                            i += 1;
                            break;
                        }
                        '\\' => match cs.get(i + 1) {
                            Some(&n) if matches!(n, '"' | '\\' | '$' | '`') => {
                                lx.word().text.push(n);
                                i += 2;
                            }
                            Some('\n') => i += 2,
                            _ => {
                                lx.word().text.push('\\');
                                i += 1;
                            }
                        },
                        '$' => i = lx.dollar(&cs, i)?,
                        '`' => i = lx.command_substitution(&cs, i)?,
                        other => {
                            lx.word().text.push(other);
                            i += 1;
                        }
                    }
                }
            }
            '$' => i = lx.dollar(&cs, i)?,
            '`' => i = lx.command_substitution(&cs, i)?,
            '|' => match cs.get(i + 1) {
                Some('|') => {
                    lx.op(Tok::Sep);
                    i += 2;
                }
                Some('&') => {
                    lx.op(Tok::Pipe);
                    i += 2;
                }
                _ => {
                    lx.op(Tok::Pipe);
                    i += 1;
                }
            },
            '&' => match cs.get(i + 1) {
                Some('&') => {
                    lx.op(Tok::Sep);
                    i += 2;
                }
                Some('>') => {
                    lx.op(Tok::RedirOut);
                    lx.skip_word = true;
                    i += 2;
                    if cs.get(i) == Some(&'>') {
                        i += 1;
                    }
                }
                _ => {
                    lx.op(Tok::Sep);
                    i += 1;
                }
            },
            ';' | '(' | ')' => {
                lx.op(Tok::Sep);
                i += 1;
            }
            '>' => {
                let fd = lx.descriptor().unwrap_or(1);
                i += 1;
                if matches!(cs.get(i), Some('>') | Some('|')) {
                    i += 1;
                }
                match cs.get(i) {
                    Some('(') => return None,
                    Some('&') => i += 1,
                    _ => {}
                }
                if fd == 1 {
                    lx.toks.push(Tok::RedirOut);
                }
                lx.skip_word = true;
            }
            '<' => {
                lx.descriptor();
                i += 1;
                match cs.get(i) {
                    // `<<<` is a here string; `<<` starts a here document, which is not followed.
                    Some('<') if cs.get(i + 1) == Some(&'<') => i += 2,
                    Some('<') | Some('(') => return None,
                    Some('&') => i += 1,
                    _ => {}
                }
                lx.toks.push(Tok::RedirIn);
                lx.skip_word = true;
            }
            other => {
                lx.word().text.push(other);
                i += 1;
            }
        }
        if lx.words > MAX_WORDS {
            return None;
        }
    }
    lx.flush();
    Some(lx.toks)
}

/// The words of a line that holds one simple command and nothing else (a line of a `crushrc`).
/// Empty for a blank or comment line; `None` when the line is anything more.
pub fn words(line: &str) -> Option<Vec<Word>> {
    let mut out = Vec::new();
    let mut ended = false;
    for tok in lex(line)? {
        match tok {
            Tok::Word(w) if !ended => out.push(w),
            Tok::Sep => ended = true,
            _ => return None,
        }
    }
    Some(out)
}

/// The words of the first simple command of a command line, without the variable assignments
/// in front of it: what a hook handler such as `X=1 "$ROOT/bin/tool" hook start 2>/dev/null ||
/// true` runs first. Empty when the line cannot be read or starts with something else.
pub fn leading_words(line: &str) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    for tok in lex(line).unwrap_or_default() {
        match tok {
            Tok::Word(w) if out.is_empty() && !w.dynamic && is_assignment(&w.text) => {}
            Tok::Word(w) => out.push(w),
            _ => break,
        }
    }
    out
}

/// One simple command with what surrounds it in its pipeline.
#[derive(Debug, Default, Clone)]
struct Simple {
    words: Vec<Word>,
    piped_in: bool,
    piped_out: bool,
    redirected_out: bool,
}

fn simple_commands(toks: Vec<Tok>) -> Vec<Simple> {
    let mut out: Vec<Simple> = Vec::new();
    let mut cur = Simple::default();
    for t in toks {
        match t {
            Tok::Word(w) => cur.words.push(w),
            Tok::RedirOut => cur.redirected_out = true,
            Tok::RedirIn => {}
            Tok::Pipe => {
                cur.piped_out = true;
                out.push(std::mem::take(&mut cur));
                cur.piped_in = true;
            }
            Tok::Sep => out.push(std::mem::take(&mut cur)),
        }
    }
    out.push(cur);
    out.retain(|s| !s.words.is_empty());
    out
}

fn base_name(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn is_assignment(s: &str) -> bool {
    match s.split_once('=') {
        Some((name, _)) => !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && !name.as_bytes()[0].is_ascii_digit(),
        None => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Flavor {
    Grep,
    Ugrep,
    Rg,
    GitGrep,
    Ag,
}

fn short_takes_value(f: Flavor, c: char) -> bool {
    match f {
        Flavor::Grep => "efmABCdD".contains(c),
        Flavor::Ugrep => "efmABCdDgtOMNJK".contains(c),
        Flavor::Rg => "efgtTABCmjMdEr".contains(c),
        Flavor::GitGrep => "efABCm".contains(c),
        Flavor::Ag => "ABCGgmp".contains(c),
    }
}

fn long_takes_value(f: Flavor, name: &str) -> bool {
    const COMMON: &[&str] = &["regexp", "file", "max-count", "after-context", "before-context", "context"];
    if COMMON.contains(&name) {
        return true;
    }
    let own: &[&str] = match f {
        Flavor::Grep | Flavor::Ugrep => &[
            "include",
            "exclude",
            "exclude-dir",
            "exclude-from",
            "include-dir",
            "include-from",
            "binary-files",
            "devices",
            "directories",
            "label",
            "file-type",
            "file-extension",
            "glob",
            "jobs",
        ],
        Flavor::Rg => &[
            "glob",
            "iglob",
            "type",
            "type-not",
            "type-add",
            "type-clear",
            "threads",
            "max-columns",
            "max-depth",
            "maxdepth",
            "max-filesize",
            "encoding",
            "replace",
            "sort",
            "sortr",
            "color",
            "colors",
            "context-separator",
            "field-context-separator",
            "field-match-separator",
            "path-separator",
            "pre",
            "pre-glob",
            "engine",
            "dfa-size-limit",
            "regex-size-limit",
            "ignore-file",
            "hyperlink-format",
            "hostname-bin",
            "generate",
        ],
        Flavor::GitGrep => &["max-depth", "threads"],
        Flavor::Ag => &[
            "after",
            "before",
            "depth",
            "file-search-regex",
            "ignore",
            "ignore-dir",
            "ignore-file",
            "pager",
            "path-to-ignore",
            "workers",
            "type",
            "type-set",
            "type-add",
            "match",
            "output",
        ],
    };
    own.contains(&name)
}

struct Scope<'a> {
    dir: &'a Dir,
    piped_in: bool,
    piped_out: bool,
    redirected_out: bool,
    /// The command's file arguments arrive from `xargs` or `find -exec`.
    fed_files: bool,
    depth: u8,
}

/// `grep`, `rg`, `git grep`, `ag`, `ack`: the one pattern the command searches for.
fn grep_like(f: Flavor, fixed_by_name: bool, args: &[Word], scope: &Scope) -> Option<Search> {
    let mut patterns: Vec<Word> = Vec::new();
    let mut positional: Vec<&Word> = Vec::new();
    let mut fixed = fixed_by_name;
    let mut by_name = false;
    let mut i = 0usize;
    while i < args.len() {
        let a = &args[i];
        let t = a.text.as_str();
        if t == "--" {
            positional.extend(&args[i + 1..]);
            break;
        }
        if let Some(long) = t.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (long, None),
            };
            match name {
                "fixed-strings" => fixed = true,
                // Not a search for where the pattern occurs.
                "invert-match" | "files-without-match" | "files-without-matches" | "files" | "type-list" | "help" | "version" => return None,
                "and" | "or" | "not" | "all-match" if f == Flavor::GitGrep => return None,
                "file" => return None,
                _ => {}
            }
            let mut val: Option<Word> = value.map(|v| Word { text: v.to_string(), dynamic: a.dynamic });
            if val.is_none() && long_takes_value(f, name) {
                i += 1;
                val = args.get(i).cloned();
            }
            if matches!(name, "regexp" | "match") {
                patterns.push(val?);
            }
            i += 1;
            continue;
        }
        if t.len() > 1 && t.starts_with('-') {
            let chars: Vec<char> = t[1..].chars().collect();
            let mut k = 0usize;
            while k < chars.len() {
                let c = chars[k];
                if short_takes_value(f, c) {
                    let attached: String = chars[k + 1..].iter().collect();
                    let val = if attached.is_empty() {
                        i += 1;
                        args.get(i).cloned()
                    } else {
                        Some(Word { text: attached, dynamic: a.dynamic })
                    };
                    match c {
                        'e' => patterns.push(val?),
                        'f' => return None,
                        'g' if f == Flavor::Ag => {
                            by_name = true;
                            patterns.push(val?);
                        }
                        _ => {}
                    }
                    break;
                }
                match c {
                    'F' => fixed = true,
                    'v' => return None,
                    'L' if f != Flavor::Rg => return None,
                    _ => {}
                }
                k += 1;
            }
            i += 1;
            continue;
        }
        positional.push(a);
        i += 1;
    }
    let pattern = match patterns.len() {
        0 => {
            if positional.is_empty() {
                return None;
            }
            positional.remove(0).clone()
        }
        1 => patterns.remove(0),
        _ => return None,
    };
    if pattern.dynamic {
        return None;
    }
    // `some-command | grep word` filters that command's output; it does not search the tree.
    if scope.piped_in && !scope.fed_files && positional.is_empty() {
        return None;
    }
    let paths = positional.iter().filter(|w| !w.dynamic).map(|w| w.text.clone()).collect();
    Some(Search { pattern: pattern.text, fixed, by_name, paths, dir: scope.dir.clone() })
}

/// `find PATH... -name GLOB`, also following `-exec COMMAND ;`.
fn find_like(args: &[Word], scope: &Scope, out: &mut Found) {
    let mut i = 0usize;
    let mut paths: Vec<String> = Vec::new();
    while i < args.len() && !args[i].text.starts_with('-') && !matches!(args[i].text.as_str(), "(" | "!") {
        if !args[i].dynamic {
            paths.push(args[i].text.clone());
        }
        i += 1;
    }
    let mut names: Vec<&Word> = Vec::new();
    while i < args.len() {
        match args[i].text.as_str() {
            "-name" | "-iname" => {
                if let Some(w) = args.get(i + 1) {
                    names.push(w);
                }
                i += 2;
            }
            "-exec" | "-execdir" | "-ok" | "-okdir" => {
                let start = i + 1;
                let mut end = start;
                while end < args.len() && !matches!(args[end].text.as_str(), ";" | "+") {
                    end += 1;
                }
                let before = out.searches.len();
                let inner = Scope { dir: scope.dir, piped_in: false, piped_out: true, redirected_out: false, fed_files: true, depth: scope.depth + 1 };
                classify(&args[start..end], &inner, out);
                // The inner command searches what `find` hands it: the starting points.
                for s in &mut out.searches[before..] {
                    s.paths.retain(|p| p != "{}");
                    if s.paths.is_empty() {
                        s.paths = paths.clone();
                    }
                }
                i = end + 1;
            }
            _ => i += 1,
        }
    }
    if let [name] = names[..] {
        let s = Search { pattern: name.text.clone(), fixed: false, by_name: true, paths, dir: scope.dir.clone() };
        // `-name '*.c'` only filters by extension: nothing is being looked up by name.
        if !name.dynamic && identifier(&s).is_some() {
            out.searches.push(s);
        }
    }
}

/// `fd PATTERN [PATH...]`.
fn fd_like(args: &[Word], scope: &Scope) -> Option<Search> {
    const LONG: &[&str] = &[
        "extension",
        "type",
        "max-depth",
        "min-depth",
        "exact-depth",
        "exclude",
        "color",
        "threads",
        "size",
        "changed-within",
        "changed-before",
        "newer",
        "older",
        "owner",
        "base-directory",
        "search-path",
        "max-results",
        "path-separator",
        "ignore-file",
        "format",
        "batch-size",
    ];
    let mut positional: Vec<&Word> = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        let t = args[i].text.as_str();
        if t == "--" {
            positional.extend(&args[i + 1..]);
            break;
        }
        if let Some(long) = t.strip_prefix("--") {
            if matches!(long, "exec" | "exec-batch") {
                break;
            }
            if !long.contains('=') && LONG.contains(&long) {
                i += 1;
            }
        } else if t.len() > 1 && t.starts_with('-') {
            let chars: Vec<char> = t[1..].chars().collect();
            if chars.iter().any(|c| matches!(c, 'x' | 'X')) {
                break;
            }
            if let Some(k) = chars.iter().position(|c| "etdEcjSo".contains(*c))
                && k + 1 == chars.len()
            {
                i += 1;
            }
        } else {
            positional.push(&args[i]);
        }
        i += 1;
    }
    let pattern = *positional.first()?;
    if pattern.dynamic {
        return None;
    }
    let paths = positional[1..].iter().filter(|w| !w.dynamic).map(|w| w.text.clone()).collect();
    let s = Search { pattern: pattern.text.clone(), fixed: false, by_name: true, paths, dir: scope.dir.clone() };
    identifier(&s).is_some().then_some(s)
}

/// `cat FILE`, `nl FILE`, `bat FILE`: one whole file sent to the agent.
fn read_like(prog: &str, args: &[Word], scope: &Scope) -> Option<FileRead> {
    if scope.piped_out || scope.redirected_out {
        return None;
    }
    let short_values: &str = match prog {
        "nl" => "bdfhilnsvw",
        "bat" | "batcat" => "lHrm",
        _ => "",
    };
    let long_values: &[&str] = match prog {
        "bat" | "batcat" => &[
            "language",
            "highlight-line",
            "style",
            "theme",
            "paging",
            "wrap",
            "tabs",
            "color",
            "decorations",
            "italic-text",
            "map-syntax",
            "terminal-width",
            "file-name",
            "diff-context",
        ],
        "nl" => &[
            "body-numbering",
            "header-numbering",
            "footer-numbering",
            "line-increment",
            "join-blank-lines",
            "number-format",
            "number-separator",
            "starting-line-number",
            "number-width",
            "section-delimiter",
        ],
        _ => &[],
    };
    let mut files: Vec<&Word> = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        let t = args[i].text.as_str();
        if t == "--" {
            files.extend(&args[i + 1..]);
            break;
        }
        if let Some(long) = t.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            // A line range is a partial read.
            if name == "line-range" {
                return None;
            }
            if !long.contains('=') && long_values.contains(&name) {
                i += 1;
            }
        } else if t.len() > 1 && t.starts_with('-') {
            let chars: Vec<char> = t[1..].chars().collect();
            if let Some(k) = chars.iter().position(|c| short_values.contains(*c)) {
                if matches!(prog, "bat" | "batcat") && chars[k] == 'r' {
                    return None;
                }
                if k + 1 == chars.len() {
                    i += 1;
                }
            }
        } else if t.starts_with('+') && matches!(prog, "less" | "more") {
            // `less +120 file` opens at a line.
        } else {
            files.push(&args[i]);
        }
        i += 1;
    }
    match files[..] {
        [f] if !f.dynamic && f.text != "-" => Some(FileRead { path: f.text.clone(), dir: scope.dir.clone() }),
        _ => None,
    }
}

/// Decide what one simple command is, after peeling off wrappers such as `env`, `xargs`,
/// `timeout` and `sh -c`.
fn classify(words: &[Word], scope: &Scope, out: &mut Found) {
    if scope.depth > MAX_DEPTH {
        return;
    }
    let mut w = words;
    let mut fed_files = scope.fed_files;
    loop {
        let Some(first) = w.first() else { return };
        if first.dynamic {
            return;
        }
        let name = base_name(&first.text);
        if is_assignment(&first.text)
            || matches!(
                name,
                "command" | "builtin" | "exec" | "nohup" | "time" | "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "!" | "{" | "noglob" | "rtk"
            )
        {
            w = &w[1..];
        } else if matches!(name, "env" | "sudo" | "nice" | "ionice" | "stdbuf" | "timeout" | "gtimeout") {
            w = &w[1..];
            while let Some(a) = w.first() {
                if a.text.starts_with('-') || is_assignment(&a.text) {
                    // `sudo -u USER`, `env -u NAME`, `nice -n N` carry a separate value.
                    let takes = matches!(a.text.as_str(), "-u" | "-n" | "-g" | "-s" | "-k" | "-c" | "-C" | "-S");
                    w = &w[1..];
                    if takes && !w.is_empty() {
                        w = &w[1..];
                    }
                } else {
                    break;
                }
            }
            if matches!(name, "timeout" | "gtimeout") && !w.is_empty() {
                w = &w[1..];
            }
        } else if name == "xargs" {
            w = &w[1..];
            while let Some(a) = w.first() {
                if !a.text.starts_with('-') {
                    break;
                }
                let t = a.text.as_str();
                let separate = matches!(
                    t,
                    "-n" | "-P"
                        | "-I"
                        | "-L"
                        | "-s"
                        | "-d"
                        | "-E"
                        | "-a"
                        | "-J"
                        | "-R"
                        | "-S"
                        | "--max-args"
                        | "--max-procs"
                        | "--replace"
                        | "--max-lines"
                        | "--max-chars"
                        | "--delimiter"
                        | "--arg-file"
                );
                w = &w[1..];
                if separate && !w.is_empty() {
                    w = &w[1..];
                }
            }
            fed_files = true;
        } else if matches!(name, "sh" | "bash" | "zsh" | "dash" | "ksh") {
            // `bash -lc 'script'`: the script is read as a command line of its own.
            let mut k = 1usize;
            while k < w.len() {
                let t = w[k].text.as_str();
                if t.starts_with('-') && !t.starts_with("--") && t.contains('c') {
                    if let Some(script) = w.get(k + 1)
                        && !script.dynamic
                    {
                        let inner = analyse_at(&script.text, scope.dir.clone(), scope.depth + 1);
                        out.searches.extend(inner.searches);
                        out.reads.extend(inner.reads);
                    }
                    return;
                }
                if !t.starts_with('-') {
                    return;
                }
                k += 1;
            }
            return;
        } else {
            break;
        }
    }
    let scope = Scope { fed_files, ..*scope };
    let prog = base_name(&w[0].text);
    let args = &w[1..];
    match prog {
        "grep" | "egrep" | "rgrep" | "zgrep" | "ggrep" => out.searches.extend(grep_like(Flavor::Grep, false, args, &scope)),
        "fgrep" => out.searches.extend(grep_like(Flavor::Grep, true, args, &scope)),
        "ugrep" | "ug" => out.searches.extend(grep_like(Flavor::Ugrep, false, args, &scope)),
        "rg" | "ripgrep" => out.searches.extend(grep_like(Flavor::Rg, false, args, &scope)),
        "ag" | "ack" => out.searches.extend(grep_like(Flavor::Ag, false, args, &scope)),
        "git" => {
            // Skip git's own options up to the subcommand.
            let mut k = 0usize;
            while k < args.len() {
                let t = args[k].text.as_str();
                if matches!(t, "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace") {
                    k += 2;
                } else if t.starts_with('-') {
                    k += 1;
                } else {
                    break;
                }
            }
            if args.get(k).is_some_and(|a| a.text == "grep" && !a.dynamic) {
                out.searches.extend(grep_like(Flavor::GitGrep, false, &args[k + 1..], &scope));
            }
        }
        "find" | "gfind" | "bfs" => find_like(args, &scope, out),
        "fd" | "fdfind" => out.searches.extend(fd_like(args, &scope)),
        "cat" | "nl" | "bat" | "batcat" | "less" | "more" if !scope.piped_in => out.reads.extend(read_like(prog, args, &scope)),
        _ => {}
    }
}

fn analyse_at(command: &str, start: Dir, depth: u8) -> Found {
    let mut out = Found::default();
    if depth > MAX_DEPTH {
        return out;
    }
    let Some(toks) = lex(command) else { return out };
    let mut dir = start;
    for s in simple_commands(toks) {
        let first = s.words[0].text.as_str();
        if matches!(first, "cd" | "pushd") && !s.words[0].dynamic && !s.piped_in && !s.piped_out {
            dir = match (s.words.get(1), &dir) {
                (Some(target), _) if target.dynamic || target.text == "-" || target.text.starts_with('~') => Dir::Unknown,
                (Some(target), _) if target.text.starts_with('/') => Dir::At(target.text.clone()),
                (Some(target), Dir::Start) => Dir::At(target.text.clone()),
                (Some(target), Dir::At(prev)) => Dir::At(format!("{}/{}", prev.trim_end_matches('/'), target.text)),
                _ => Dir::Unknown,
            };
            continue;
        }
        if first == "popd" {
            dir = Dir::Unknown;
            continue;
        }
        let scope = Scope { dir: &dir, piped_in: s.piped_in, piped_out: s.piped_out, redirected_out: s.redirected_out, fed_files: false, depth };
        classify(&s.words, &scope, &mut out);
    }
    out
}

/// The searches and whole-file reads in a shell command line.
/// Whether a command line may have changed source files: an in-place editor, a redirect into a
/// file, a formatter, a script given on the command line, a git command that moves the work
/// tree. Erring towards yes costs a check that finds nothing; erring towards no costs the check.
pub fn may_write(command: &str) -> bool {
    const TOOLS: [&str; 34] = [
        "sed -i", "sed -E -i", "sed -e", "perl -pi", "perl -i", "perl -0pi", "awk -i", "tee ", "mv ", "cp ", "rm ", "patch ", "git apply", "git checkout", "git restore", "git stash",
        "git mv", "git rm", "git revert", "git reset", "git merge", "git rebase", "git cherry-pick", "git pull", "gofmt -w", "go fmt", "cargo fmt", "rustfmt", "prettier", "eslint --fix",
        "ruff", "black ", "clang-format -i", "truncate ",
    ];
    const SCRIPTS: [&str; 10] = ["python - ", "python3 - ", "python -c", "python3 -c", "node -e", "ruby -e", "perl -e", "<<'", "<<\"", "<<EOF"];
    if TOOLS.iter().any(|t| command.contains(t)) || SCRIPTS.iter().any(|t| command.contains(t)) {
        return true;
    }
    // `> file` and `>> file`, but not `2>&1`, `>/dev/null` or a comparison inside a test.
    let b = command.as_bytes();
    (0..b.len()).any(|i| {
        b[i] == b'>' && b.get(i + 1) != Some(&b'&') && (i == 0 || !matches!(b[i - 1], b'2' | b'&' | b'-' | b'=')) && {
            let rest = command[i..].trim_start_matches('>').trim_start();
            !rest.starts_with("/dev/") && !rest.is_empty()
        }
    })
}

pub fn analyse(command: &str) -> Found {
    analyse_at(command, Dir::Start, 0)
}

/// A symbol name a search pattern asks for, with the kind word written in front of it, if any
/// (`struct file_operations`, `def handle`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub kind: Option<&'static str>,
}

/// Words that are legitimate search patterns but never a question about a symbol.
const STOP: &[&str] = &[
    "TODO",
    "FIXME",
    "XXX",
    "HACK",
    "NOTE",
    "BUG",
    "true",
    "false",
    "null",
    "nil",
    "None",
    "True",
    "False",
    "return",
    "static",
    "const",
    "void",
    "int",
    "char",
    "long",
    "bool",
    "struct",
    "class",
    "def",
    "import",
    "from",
    "include",
    "define",
    "function",
    "var",
    "let",
    "pub",
    "use",
    "mod",
    "impl",
    "trait",
    "enum",
    "union",
    "typedef",
    "public",
    "private",
    "protected",
    "package",
    "interface",
    "extends",
    "implements",
    "new",
    "this",
    "self",
    "async",
    "await",
    "try",
    "catch",
    "throw",
    "export",
    "default",
    "switch",
    "case",
    "break",
    "continue",
    "type",
    "func",
    "for",
    "while",
    "else",
    "elif",
    "string",
    "error",
    "test",
    "tests",
];

/// Kind words as `query::split_kind_prefix` understands them.
const KINDS: &[&str] = &[
    "struct",
    "union",
    "enum",
    "class",
    "interface",
    "trait",
    "protocol",
    "fn",
    "func",
    "function",
    "def",
    "method",
    "macro",
    "define",
    "type",
    "typedef",
    "var",
    "const",
    "static",
];
/// Words that may stand before a kind word or a name without changing what is asked.
const MODIFIERS: &[&str] = &[
    "pub",
    "export",
    "default",
    "async",
    "public",
    "private",
    "protected",
    "static",
    "inline",
    "extern",
    "abstract",
    "final",
    "virtual",
    "unsafe",
    "declare",
    "let",
    "impl",
    "mod",
    "module",
    "namespace",
    "package",
    "record",
];

pub fn is_identifier(s: &str) -> bool {
    let b = s.as_bytes();
    (3..=64).contains(&b.len())
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        && !b.iter().all(|c| *c == b'_')
}

fn strip_any<'a>(s: &'a str, parts: &[&str], from_end: bool) -> Option<&'a str> {
    parts.iter().find_map(|p| if from_end { s.strip_suffix(p) } else { s.strip_prefix(p) })
}

/// The single identifier a pattern looks for, when the pattern is nothing but that identifier
/// with the usual decorations: anchors, word boundaries, a kind word, an opening parenthesis.
pub fn identifier(search: &Search) -> Option<Ident> {
    let p = search.pattern.trim();
    if p.len() < 3 || p.len() > 160 || p.contains('\n') {
        return None;
    }
    if search.by_name {
        // A file-name pattern: `*vfs_read*`, `vfs_read.c`, `**/vfs_read.*`.
        let last = p.rsplit('/').next().unwrap_or(p);
        let last = last.trim_start_matches(['*', '^', '?']).trim_end_matches(['*', '$', '?']);
        let last = last.strip_prefix(".*").unwrap_or(last);
        let stem = last.split('.').next().unwrap_or(last);
        return (is_identifier(stem) && !STOP.contains(&stem)).then(|| Ident { name: stem.to_string(), kind: None });
    }
    let mut s = p;
    if search.fixed {
        s = s.trim_end_matches(['(', '{', ':', ';', ' ', '=', ',']).trim_start();
    } else {
        const LEAD: &[&str] = &["^", "\\b", "\\<", "\\s*", "\\s+", "[[:space:]]*", "[[:space:]]+", "(?i)", "(?m)", " "];
        const TRAIL: &[&str] = &["$", "\\b", "\\>", "\\s*", "\\s+", "\\s", "[[:space:]]*", "\\(", "(", "\\{", "{", "\\[", ":", ";", ",", "=", " "];
        while let Some(rest) = strip_any(s, LEAD, false) {
            s = rest;
        }
        while let Some(rest) = strip_any(s, TRAIL, true) {
            s = rest;
        }
    }
    // Whitespace between words may be written as a regex.
    let mut plain = s.to_string();
    if !search.fixed {
        for sep in ["[[:space:]]+", "[[:space:]]*", "[[:blank:]]+", "\\s+", "\\s*", "\\s", "\\ ", " +", " *", "\\t"] {
            plain = plain.replace(sep, " ");
        }
    }
    let parts: Vec<&str> = plain.split_whitespace().collect();
    let (name, lead) = parts.split_last()?;
    if !is_identifier(name) || STOP.contains(name) || lead.len() > 3 {
        return None;
    }
    let mut kind: Option<&'static str> = None;
    for (i, word) in lead.iter().enumerate() {
        let word = word.trim_start_matches(['#', '\\']);
        let last = i + 1 == lead.len();
        if let (true, Some(k)) = (last, KINDS.iter().find(|k| **k == word)) {
            kind = Some(*k);
        } else if !MODIFIERS.contains(&word) && !KINDS.contains(&word) {
            return None;
        }
    }
    Some(Ident { name: name.to_string(), kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(command: &str) -> Vec<Search> {
        analyse(command).searches
    }

    fn one(command: &str) -> Search {
        let mut s = search(command);
        assert_eq!(s.len(), 1, "{command}: {s:?}");
        s.remove(0)
    }

    fn name(command: &str) -> Option<String> {
        let s = search(command);
        match &s[..] {
            [s] => identifier(s).map(|i| i.name),
            _ => None,
        }
    }

    #[test]
    fn plain_grep_and_rg() {
        let s = one("grep -rn \"vfs_read\" fs/ --include='*.c'");
        assert_eq!((s.pattern.as_str(), s.fixed, s.by_name), ("vfs_read", false, false));
        assert_eq!(s.paths, ["fs/"]);
        assert_eq!(one("rg -n --type c -g '*.h' vfs_read include kernel").paths, ["include", "kernel"]);
        assert_eq!(one("rg -t c -e vfs_read").pattern, "vfs_read");
        assert_eq!(one("grep -rnw -e vfs_read .").pattern, "vfs_read");
        assert_eq!(one("grep -rnA3 vfs_read .").pattern, "vfs_read");
        assert_eq!(one("grep -rn -A 3 -m 5 vfs_read .").pattern, "vfs_read");
        assert_eq!(one("/usr/bin/grep --color=never -rn vfs_read .").pattern, "vfs_read");
        // GNU grep's --color takes its value attached only; ripgrep's takes the next word.
        assert_eq!(one("grep --color -rn vfs_read .").pattern, "vfs_read");
        assert_eq!(one("rg --color never vfs_read").pattern, "vfs_read");
        assert!(one("grep -rnF 'vfs_read(' .").fixed);
        assert!(one("fgrep -rn vfs_read .").fixed);
        assert_eq!(one("git --no-pager grep -n vfs_read -- fs").paths, ["fs"]);
        assert_eq!(one("git -C /repo grep -nw vfs_read").pattern, "vfs_read");
        assert_eq!(one("ag --depth 3 vfs_read fs").pattern, "vfs_read");
    }

    #[test]
    fn wrappers_pipelines_and_directories() {
        assert_eq!(one("cd fs && grep -rn vfs_read . | head -20").dir, Dir::At("fs".into()));
        assert_eq!(one("cd /abs/x; cd y; rg vfs_read").dir, Dir::At("/abs/x/y".into()));
        assert_eq!(one("cd \"$HOME\" && rg vfs_read").dir, Dir::Unknown);
        assert_eq!(one("LC_ALL=C timeout 10 grep -rn vfs_read . 2>/dev/null").pattern, "vfs_read");
        assert_eq!(one("bash -lc 'rg -n \"vfs_read\" fs'").pattern, "vfs_read");
        assert_eq!(one("find . -name '*.c' | xargs grep -n vfs_read").pattern, "vfs_read");
        assert_eq!(one("find . -name '*.c' -print0 | xargs -0 -n 50 grep -ln vfs_read").pattern, "vfs_read");
        let s = one("find fs -type f -exec grep -l vfs_read {} +");
        assert_eq!((s.pattern.as_str(), s.paths.clone()), ("vfs_read", vec!["fs".to_string()]));
        assert_eq!(one("grep -rn vfs_read . > /tmp/out.txt; wc -l /tmp/out.txt").pattern, "vfs_read");
        assert_eq!(search("grep -rn foo_bar . ; rg baz_qux").len(), 2);
    }

    #[test]
    fn things_that_are_not_tree_searches() {
        for command in [
            "ps aux | grep node_server",
            "grep -v vfs_read file.c",
            "grep -L vfs_read *.c",
            "grep -f patterns.txt -r .",
            "grep -e foo_one -e bar_two -r .",
            "rg --files | head",
            "grep \"$PATTERN\" -r .",
            "grep `cat name` -r .",
            "grep $(cat name) -r .",
            "echo grep vfs_read",
            "git log --grep vfs_read",
            "cat <<EOF\ngrep vfs_read\nEOF",
            "diff <(grep vfs_read a) b",
            "grep 'unterminated",
            "",
        ] {
            assert_eq!(search(command), [], "{command}");
        }
    }

    #[test]
    fn identifiers_from_patterns() {
        for (command, want) in [
            ("grep -rn vfs_read .", Some("vfs_read")),
            ("grep -rn 'vfs_read(' .", Some("vfs_read")),
            ("grep -rnE 'vfs_read\\(' .", Some("vfs_read")),
            ("rg '\\bvfs_read\\b'", Some("vfs_read")),
            ("rg \"\\bvfs_read\\b\"", Some("vfs_read")),
            ("rg '^vfs_read\\s*\\('", Some("vfs_read")),
            ("grep -rn '\\<vfs_read\\>' .", Some("vfs_read")),
            ("grep -rn 'struct file_operations {' include", Some("file_operations")),
            ("rg 'fn parse_spec'", Some("parse_spec")),
            ("rg 'pub fn parse_spec\\('", Some("parse_spec")),
            ("rg 'def\\s+handle_request'", Some("handle_request")),
            ("grep -rn '#define container_of' include", Some("container_of")),
            ("rg 'class\\s+HttpServer\\b'", Some("HttpServer")),
            ("grep -rn 'vfs_read.*' .", None),
            ("grep -rnE 'vfs_(read|write)' .", None),
            ("grep -rn 'vfs_read|vfs_write' .", None),
            ("grep -rn 'foo->bar' .", None),
            ("grep -rn 'int vfs_read' .", None),
            ("grep -rn 'return' .", None),
            ("grep -rn 'TODO' .", None),
            ("grep -rn 'ab' .", None),
            ("grep -rn '[a-z]+_read' .", None),
            ("grep -rn 'vfs_read$(reboot)' .", None),
            ("grep -rn 'vfs_read;rm -rf /' .", None),
        ] {
            assert_eq!(name(command).as_deref(), want, "{command}");
        }
        let s = one("grep -rn 'struct file_operations {' include");
        assert_eq!(identifier(&s).unwrap().kind, Some("struct"));
    }

    #[test]
    fn file_name_searches() {
        for (command, want) in [
            ("find . -name '*vfs_read*'", Some("vfs_read")),
            ("find fs -iname \"read_write.c\"", Some("read_write")),
            ("find . -name '*.c'", None),
            ("find . -type f", None),
            ("find . -name 'a*' -o -name 'b*'", None),
            ("fd -e c vfs_read fs", Some("vfs_read")),
            ("fd --type f 'vfs_read.*'", Some("vfs_read")),
            ("find . -name \"$NAME\"", None),
        ] {
            assert_eq!(name(command).as_deref(), want, "{command}");
        }
        assert_eq!(one("find fs kernel -name 'vfs_read*'").paths, ["fs", "kernel"]);
    }

    #[test]
    fn whole_file_reads() {
        let reads = |c: &str| analyse(c).reads.into_iter().map(|r| r.path).collect::<Vec<_>>();
        assert_eq!(reads("cat src/big.c"), ["src/big.c"]);
        assert_eq!(reads("cat -n 'dir with space/big.c'"), ["dir with space/big.c"]);
        assert_eq!(reads("nl -ba src/big.c"), ["src/big.c"]);
        assert_eq!(reads("nl -b a -w 4 src/big.c"), ["src/big.c"]);
        assert_eq!(reads("bat --paging=never --style plain src/big.c"), ["src/big.c"]);
        assert_eq!(reads("cd src && cat big.c"), ["big.c"]);
        assert_eq!(analyse("cd src && cat big.c").reads[0].dir, Dir::At("src".into()));
        assert_eq!(reads("bash -lc 'cat src/big.c'"), ["src/big.c"]);
        for partial in [
            "cat src/big.c | head -50",
            "cat src/big.c > copy.c",
            "cat a.c b.c",
            "cat",
            "cat \"$FILE\"",
            "bat -r 10:40 src/big.c",
            "bat --line-range 10:40 src/big.c",
            "head -n 500 src/big.c",
            "sed -n 1,200p src/big.c",
            "git show HEAD:src/big.c | cat",
        ] {
            assert_eq!(reads(partial), [] as [&str; 0], "{partial}");
        }
        // Reading the error stream elsewhere does not make it a partial read.
        assert_eq!(reads("cat src/big.c 2>/dev/null"), ["src/big.c"]);
        assert_eq!(reads("cat src/big.c 2>&1"), ["src/big.c"]);
    }

    #[test]
    fn quoting_is_followed_exactly() {
        let w = |s: &str| lex(s).unwrap().into_iter().filter_map(|t| if let Tok::Word(w) = t { Some(w.text) } else { None }).collect::<Vec<_>>();
        assert_eq!(w("a 'b c' \"d e\" f\\ g"), ["a", "b c", "d e", "f g"]);
        assert_eq!(w("grep \"\\bfoo\\b\" 'a\"b' \"a'b\""), ["grep", "\\bfoo\\b", "a\"b", "a'b"]);
        assert_eq!(w("grep foo\\( x"), ["grep", "foo(", "x"]);
        assert_eq!(w("grep \"foo$\" x # comment"), ["grep", "foo$", "x"]);
        assert_eq!(w("find . -exec grep foo {} \\;"), ["find", ".", "-exec", "grep", "foo", "{}", ";"]);
        assert_eq!(w("a 2>&1 >/dev/null b"), ["a", "b"]);
        assert_eq!(w("a ''"), ["a", ""]);
        assert!(lex(&"x ".repeat(MAX_COMMAND_BYTES)).is_none());
        assert!(lex("echo \"unterminated").is_none());
        assert!(lex("echo $(unterminated").is_none());
    }
}
