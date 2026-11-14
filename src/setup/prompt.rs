//! Reading a user's prompt for the symbols it names.
//!
//! A request such as "rename the method Add of *Collection (defined in syft/pkg/collection.go)"
//! or "`parse_duration` returns the wrong sign" names code. The words that do so have a shape
//! prose does not have (an underscore, an inner capital, a dot between two names, backticks), or
//! stand in a phrase that says what they are ("the method X of Y"). This module finds them. It
//! decides nothing about the repository: whether a word is a symbol is for the index to say.

/// A word of the prompt that may name a symbol.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mention {
    /// The type, module or class the name was qualified with: `Collection` in `Collection.Add`.
    pub owner: Option<String>,
    pub name: String,
    /// Written as code: in backticks, with an owner, with `()`, or in a "method X of Y" phrase.
    pub explicit: bool,
    /// Said to be a type: `struct kiocb`, `the class Parser`.
    pub is_type: bool,
}

/// A question the index answers whole, read from the shape of a sentence of the prompt. The
/// names are as the prompt wrote them: `name` or `Owner.name`.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// "What does X call?"
    Callees(String),
    /// "How does A get to B?"
    Path(String, String),
    /// "Which functions set the f field of struct S?"
    Writers(String),
    /// "Find the files that have to be changed to fix this", over a report of a problem: the
    /// files whose text the report's words match best.
    Locate,
}

#[derive(Debug, Default, PartialEq)]
pub struct Mentions {
    pub symbols: Vec<Mention>,
    /// Words that look like file paths (`pkg/collection.go`, `collection.go`).
    pub paths: Vec<String>,
    /// The prompt asks for something to be renamed.
    pub rename: bool,
    /// What the prompt asks about the symbols it names, where that has a known shape.
    pub asks: Vec<Ask>,
    /// The other words of the sentence each symbol stands in, lower case: `x86` beside
    /// `copy_thread` says which of several definitions is meant.
    pub beside: Vec<(String, Vec<String>)>,
}

/// Prompts longer than this are read up to here: a pasted log names thousands of things.
const MAX_PROMPT: usize = 24 * 1024;
const MAX_SYMBOLS: usize = 24;

const EXTENSIONS: [&str; 34] = [
    "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "py", "pyi", "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "go", "rs", "java", "rb", "php", "cs", "kt", "kts", "swift", "scala",
    "lua", "sh", "zig", "m", "mm", "vue", "svelte",
];

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_alphabetic() || f == '_' || f == '$') && c.all(|x| x.is_alphanumeric() || x == '_' || x == '$')
}

fn is_path(w: &str) -> bool {
    let Some((stem, ext)) = w.rsplit_once('.') else { return false };
    !stem.is_empty() && EXTENSIONS.contains(&ext) && stem.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.' | '+'))
}

/// A name that prose would not contain: `snake_case`, `camelCase`, `PascalCase` with two humps.
fn code_shaped(w: &str) -> bool {
    if !is_ident(w) || w.len() < 4 {
        return false;
    }
    let b = w.as_bytes();
    let inner_underscore = w.trim_matches('_').contains('_');
    let hump = b.windows(2).any(|p| p[0].is_ascii_lowercase() && p[1].is_ascii_uppercase());
    let digit = b.iter().any(u8::is_ascii_digit) && b.iter().any(u8::is_ascii_alphabetic) && w.len() >= 5;
    inner_underscore || hump || (digit && b.iter().any(u8::is_ascii_lowercase) && w.contains('_'))
}

/// Split `a.b.C.name`, `A::name`, `A#name`, `a->name` into the owner (its last part) and name.
fn qualified(w: &str) -> Option<(Option<String>, String)> {
    let norm = w.replace("::", ".").replace("->", ".").replace('#', ".");
    let parts: Vec<&str> = norm.split('.').collect();
    if parts.len() < 2 || !parts.iter().all(|p| is_ident(p)) {
        return None;
    }
    let name = parts[parts.len() - 1].to_string();
    let owner = parts[parts.len() - 2].to_string();
    Some((Some(owner), name))
}

fn strip(w: &str) -> &str {
    w.trim_matches(|c: char| matches!(c, ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '*' | '&' | '`')).trim_end_matches('.')
}

fn push(out: &mut Mentions, m: Mention) {
    if out.symbols.len() >= MAX_SYMBOLS {
        return;
    }
    match out.symbols.iter_mut().find(|x| x.name == m.name && (x.owner == m.owner || x.owner.is_none() || m.owner.is_none())) {
        Some(x) => {
            x.explicit |= m.explicit;
            x.is_type |= m.is_type;
            if x.owner.is_none() {
                x.owner = m.owner;
            }
        }
        None => out.symbols.push(m),
    }
}

fn word(out: &mut Mentions, raw: &str, quoted: bool) {
    let called = raw.contains("()") || raw.ends_with('(');
    let w = strip(raw.trim_end_matches("()").trim_end_matches('('));
    if w.is_empty() || w.len() > 120 {
        return;
    }
    if is_path(w) {
        let p = w.trim_start_matches("./").to_string();
        if !out.paths.contains(&p) {
            out.paths.push(p);
        }
        return;
    }
    if w.contains('/') {
        return;
    }
    if let Some((owner, name)) = qualified(w) {
        // `e.g`, `i.e`, version numbers and sentence ends are not names.
        if name.len() >= 2 && (quoted || called || code_shaped(&name) || owner.as_deref().is_some_and(|o| o.chars().next().is_some_and(char::is_uppercase) || code_shaped(o))) {
            push(out, Mention { owner, name, explicit: true, is_type: false });
        }
        return;
    }
    if is_ident(w) && (quoted || called || code_shaped(w)) && w.len() >= 3 {
        push(out, Mention { owner: None, name: w.to_string(), explicit: quoted || called, is_type: false });
    }
}

/// "the method Add of *Collection", "function parse in class Parser": a name and its owner,
/// said in words. Both may be plain words, so only the phrase makes them symbols.
fn phrases(out: &mut Mentions, text: &str) {
    const KINDS: [&str; 9] = ["method", "function", "func", "member", "field", "property", "attribute", "constructor", "getter"];
    const LINKS: [&str; 4] = ["of", "on", "in", "from"];
    const FILLER: [&str; 9] = ["the", "a", "an", "type", "class", "struct", "interface", "trait", "object"];
    let words: Vec<&str> = text.split_whitespace().collect();
    for i in 0..words.len() {
        if !KINDS.contains(&strip(words[i]).to_ascii_lowercase().as_str()) {
            continue;
        }
        let Some(name) = words.get(i + 1).map(|w| strip(w.trim_end_matches("()"))) else { continue };
        if !is_ident(name) || !words.get(i + 2).is_some_and(|l| LINKS.contains(&strip(l).to_ascii_lowercase().as_str())) {
            continue;
        }
        let mut j = i + 3;
        while words.get(j).is_some_and(|w| FILLER.contains(&strip(w).to_ascii_lowercase().as_str())) {
            j += 1;
        }
        let Some(owner) = words.get(j).map(|w| strip(w)) else { continue };
        let owner = owner.rsplit(['.', ':']).next().unwrap_or(owner);
        if is_ident(owner) && owner != name {
            push(out, Mention { owner: Some(owner.to_string()), name: name.to_string(), explicit: true, is_type: false });
        }
    }
}

/// A prompt longer than this is not read for a question: a pasted report asks a hundred things
/// in passing, and none of them is the request.
const ASK_MAX_PROMPT: usize = 2000;
const TYPE_WORDS: [&str; 6] = ["struct", "union", "enum", "class", "interface", "trait"];
const MEMBER_NOUNS: [&str; 9] = ["field", "member", "counter", "flag", "pointer", "variable", "attribute", "property", "slot"];
/// Words that follow `struct` or `class` in prose and are not the name of one.
const NOT_A_NAME: [&str; 30] = [
    "of", "is", "are", "that", "which", "for", "to", "in", "and", "or", "the", "a", "an", "with", "has", "have", "does", "do", "it", "its", "this", "these", "those", "was", "were",
    "be", "been", "not", "no", "on",
];

/// Words of a question that are about asking and not about the thing asked for. A project
/// may have a directory called `function` or `driver`, and "which function" does not mean it.
const GENERIC: [&str; 97] = [
    "the", "and", "for", "with", "from", "that", "this", "these", "those", "its", "all", "any", "are", "was", "were", "not", "has", "have", "had", "does", "did", "can", "could",
    "should", "would", "will", "which", "what", "where", "who", "whom", "whose", "why", "how", "when", "then", "than", "there", "here", "into", "onto", "over", "under", "about",
    "function", "functions", "method", "methods", "code", "file", "files", "call", "calls", "called", "caller", "callers", "implement", "implements", "implemented", "handle",
    "handles", "handler", "define", "defined", "definition", "register", "registered", "driver", "drivers", "kernel", "struct", "class", "type", "types", "field", "fields", "member",
    "members", "signature", "parameters", "arguments", "return", "returns", "take", "takes", "table", "hook", "source", "project", "repository", "directory", "name", "named", "one",
    "use", "used", "uses",
];

/// The sentences of a prompt: cut where a full stop, a question mark or an exclamation mark
/// is followed by white space, and at line ends. A dot inside `a.b` or `mm/memory.c` is not
/// an end.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut start = 0;
    for i in 0..b.len() {
        let end = b[i] == b'\n' || (matches!(b[i], b'.' | b'?' | b'!') && b.get(i + 1).is_none_or(|c| c.is_ascii_whitespace()));
        if end {
            if text[start..=i].trim().len() > 1 {
                out.push(text[start..=i].trim());
            }
            start = i + 1;
        }
    }
    if start < b.len() && text[start..].trim().len() > 1 {
        out.push(text[start..].trim());
    }
    out
}

/// A word of a sentence as the symbol `word` would take it for: `name` or `Owner.name`.
fn symbol_of(raw: &str) -> Option<String> {
    let quoted = raw.contains('`');
    let called = raw.contains("()") || raw.ends_with('(');
    let w = strip(raw.trim_end_matches("()").trim_end_matches('('));
    if w.is_empty() || w.len() > 120 || is_path(w) || w.contains('/') {
        return None;
    }
    if let Some((owner, name)) = qualified(w) {
        let owner = owner.unwrap_or_default();
        let ok = name.len() >= 2 && (quoted || called || code_shaped(&name) || owner.chars().next().is_some_and(char::is_uppercase) || code_shaped(&owner));
        return ok.then(|| format!("{owner}.{name}"));
    }
    (is_ident(w) && (quoted || called || code_shaped(w)) && w.len() >= 3).then(|| w.to_string())
}

/// A word without the marks around it and without a possessive: `ext4's,` is `ext4`.
fn bare(raw: &str) -> &str {
    let w = strip(raw.trim_end_matches("()"));
    w.strip_suffix("'s").or_else(|| w.strip_suffix("\u{2019}s")).unwrap_or(w)
}

fn possessive(raw: &str) -> bool {
    let w = strip(raw);
    w.ends_with("'s") || w.ends_with("\u{2019}s")
}

/// What one sentence says in the shapes this module knows: the types it names with a kind
/// word (`struct kiocb`), the members it names in words (`the mnt_ns field of struct mount`,
/// `mm_struct's locked_vm counter`, `ctx->cq_entries in struct io_ring_ctx`), and the
/// question it asks about them.
fn sentence(out: &mut Mentions, text: &str, ask: bool) {
    let raw: Vec<&str> = text.split_whitespace().collect();
    let low: Vec<String> = raw.iter().map(|w| bare(w).to_lowercase()).collect();
    let is = |i: usize, set: &[&str]| low.get(i).is_some_and(|w| set.contains(&w.as_str()));
    let name_at = |i: usize| raw.get(i).map(|w| bare(w)).filter(|w| is_ident(w) && !NOT_A_NAME.contains(&w.to_lowercase().as_str()));

    // Types named with their kind word.
    let mut types: Vec<String> = Vec::new();
    for i in 0..raw.len() {
        if is(i, &TYPE_WORDS) {
            if let Some(y) = name_at(i + 1) {
                if !types.iter().any(|t| t == y) {
                    types.push(y.to_string());
                }
                push(out, Mention { owner: None, name: y.to_string(), explicit: true, is_type: true });
            }
        }
    }

    // Members named in words. Each is (owner, name, the position of the name), and the words
    // that are the owners are remembered: `UploadHaves` in "the method Encode of UploadHaves"
    // is not a second symbol of the sentence.
    let mut members: Vec<(String, String, usize)> = Vec::new();
    let mut owner_words: Vec<usize> = Vec::new();
    for i in 0..raw.len() {
        let Some(x) = name_at(i) else { continue };
        // "the X field of struct Y", "X of struct Y"
        let after = if is(i + 1, &MEMBER_NOUNS) && is(i + 2, &["of", "in", "on"]) {
            Some(i + 3)
        } else if is(i + 1, &["of", "in"]) && is(i + 2, &TYPE_WORDS) {
            Some(i + 2)
        } else {
            None
        };
        if let Some(mut j) = after {
            let typed = is(j, &TYPE_WORDS) || is(j + 1, &TYPE_WORDS);
            while is(j, &["the", "a", "an", "struct", "union", "class", "type"]) {
                j += 1;
            }
            if let Some(y) = name_at(j).filter(|y| *y != x && (typed || code_shaped(y) || code_shaped(x))) {
                members.push((y.to_string(), x.to_string(), i));
                owner_words.push(j);
                continue;
            }
        }
        // "the method X of Y", "function X in class Y": the phrase `phrases` reads, here
        // with the place of the name, which the question is read from.
        if i >= 1 && is(i - 1, &["method", "function", "func", "member", "field", "property", "attribute", "constructor", "getter"]) && is(i + 1, &["of", "on", "in", "from"]) {
            let mut j = i + 2;
            while is(j, &["the", "a", "an", "type", "class", "struct", "interface", "trait", "object"]) {
                j += 1;
            }
            if let Some(y) = raw.get(j).map(|w| bare(w)).map(|y| y.rsplit(['.', ':']).next().unwrap_or(y)).filter(|y| is_ident(y) && *y != x) {
                members.push((y.to_string(), x.to_string(), i));
                owner_words.push(j);
                continue;
            }
        }
        // "struct Y's X", "Y's X counter"
        if i >= 1 && possessive(raw[i - 1]) && (is(i + 1, &MEMBER_NOUNS) || (i >= 2 && is(i - 2, &TYPE_WORDS))) {
            let y = bare(raw[i - 1]);
            if is_ident(y) && y != x {
                members.push((y.to_string(), x.to_string(), i));
                owner_words.push(i - 1);
            }
        }
    }
    // "v->X in struct Y", "thread.sp (struct thread_struct)": the variable is not the type.
    if let [y] = &types[..] {
        for (i, w) in raw.iter().enumerate() {
            let w = strip(w.trim_end_matches("()"));
            if let Some((Some(v), x)) = qualified(w) {
                let lower_var = v.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit());
                if x.len() >= 2 && lower_var && v != *y && !is_path(w) && !members.iter().any(|m| m.1 == x) {
                    members.push((y.clone(), x, i));
                }
            }
        }
    }
    for (y, x, _) in &members {
        push(out, Mention { owner: Some(y.clone()), name: x.clone(), explicit: true, is_type: false });
    }

    // The symbols of the sentence in order, and the words that stand beside each.
    let mut syms: Vec<(usize, String)> = raw.iter().enumerate().filter_map(|(i, w)| symbol_of(w).map(|s| (i, s))).collect();
    for (y, x, i) in &members {
        let key = format!("{y}.{x}");
        syms.retain(|(j, _)| j != i);
        syms.push((*i, key));
    }
    syms.retain(|(j, _)| !owner_words.contains(j));
    syms.sort_by_key(|(i, _)| *i);
    let words: Vec<String> = low.iter().filter(|w| w.len() >= 3 && w.chars().all(|c| c.is_alphanumeric() || c == '_') && !GENERIC.contains(&w.as_str())).take(60).cloned().collect();
    for (_, s) in syms.iter().chain(types.iter().map(|t| (0usize, t.clone())).collect::<Vec<_>>().iter()) {
        let name = s.rsplit('.').next().unwrap_or(s).to_string();
        if !out.beside.iter().any(|(n, _)| *n == name) && out.beside.len() < MAX_SYMBOLS {
            let own = name.to_lowercase();
            out.beside.push((name, words.iter().filter(|w| **w != own).cloned().collect()));
        }
    }
    if !ask || out.asks.len() >= 2 {
        return;
    }

    // "Which functions set the mnt_ns field of struct mount?": a verb of writing, and after
    // it a member.
    const WRITES: [&str; 22] = [
        "write", "writes", "wrote", "written", "set", "sets", "assign", "assigns", "assigned", "modify", "modifies", "modified", "change", "changes", "changed", "update", "updates",
        "updated", "mutate", "mutates", "store", "stores",
    ];
    const ASKING: [&str; 10] = ["which", "what", "who", "where", "every", "all", "list", "find", "show", "everything"];
    // "the set of flags", "a change to": a noun, not the verb.
    let noun = |i: usize| i >= 1 && is(i - 1, &["the", "a", "an", "this", "that", "each", "its", "their", "any", "no"]);
    let verb = (0..raw.len()).find(|&i| is(i, &WRITES) && !noun(i) && (0..i).any(|j| is(j, &ASKING)));
    if let Some(v) = verb {
        let member = members.iter().filter(|m| m.2 > v).map(|m| format!("{}.{}", m.0, m.1)).next().or_else(|| syms.iter().find(|(i, s)| *i > v && s.contains('.')).map(|(_, s)| s.clone()));
        if let Some(m) = member {
            out.asks.push(Ask::Writers(m));
            return;
        }
    }
    // "Where is Config.Timeout set?": the member first, then the participle.
    const WRITTEN: [&str; 7] = ["set", "written", "assigned", "modified", "changed", "updated", "stored"];
    for (i, sym) in syms.iter().filter(|(_, s)| s.contains('.')) {
        if (0..*i).any(|j| is(j, &ASKING)) && (i + 1..(i + 4).min(raw.len())).any(|k| is(k, &WRITTEN)) {
            out.asks.push(Ask::Writers(sym.clone()));
            return;
        }
    }

    // "What does X call?", "the functions X calls", "called by X".
    const AUX: [&str; 10] = ["does", "do", "did", "will", "would", "can", "could", "should", "may", "might"];
    for (i, s) in &syms {
        let i = *i;
        let aux_before = (i.saturating_sub(8)..i).any(|j| is(j, &AUX));
        let verb_after = (i + 1..(i + 9).min(raw.len())).find(|&k| is(k, &["call", "invoke"]));
        // Between the function and the verb there is only what says which function it is:
        // its type, its file, a remark in brackets. "Why does X fail when the tests call
        // it" has a clause there, and asks something else.
        let says_which = |k: usize| {
            let w = raw[k];
            is(k, &["of", "in", "on", "at", "from", "the", "a", "an", "type", "class", "struct", "defined", "declared", "directly", "itself", "actually", "really", "then", "ever"])
                || is_path(strip(w))
                || w.contains('/')
                || w.starts_with('(')
                || w.ends_with(')')
                || w.starts_with('*')
                || owner_words.contains(&k)
                || bare(w).chars().next().is_some_and(char::is_uppercase)
        };
        let clean = verb_after.is_some_and(|k| (i + 1..k).all(says_which) && !syms.iter().any(|(j, _)| *j > i && *j < k));
        let relative = is(i + 1, &["calls", "invokes"]) && (i.saturating_sub(2)..i).any(|j| is(j, &["what", "everything", "functions", "function", "that", "which", "all"]));
        let passive = i >= 2 && is(i - 1, &["by", "of"]) && is(i - 2, &["called", "invoked", "callees", "made"]);
        if (aux_before && clean) || relative || passive {
            out.asks.push(Ask::Callees(s.clone()));
            return;
        }
    }

    // "How does A get from here to B?", "how does A end up in B?", "the path from A to B".
    const REACH: [&[&str]; 13] = [
        &["reach"], &["reaches"], &["end", "up"], &["ends", "up"], &["get", "to"], &["gets", "to"], &["lead", "to"], &["leads", "to"], &["arrive", "at"], &["call", "into"],
        &["calls", "into"], &["eventually", "call"], &["eventually", "calls"],
    ];
    for pair in syms.windows(2) {
        let ((i, a), (j, b)) = (&pair[0], &pair[1]);
        let (i, j) = (*i, *j);
        if a == b || j - i > 9 {
            continue;
        }
        let from_to = (i.saturating_sub(3)..i).any(|f| is(f, &["from"])) && (i + 1..j).any(|t| is(t, &["to"]) && j - t <= 3);
        let reaches = REACH.iter().any(|ph| (i + 1..j).any(|k| ph.iter().enumerate().all(|(n, w)| k + n < j && low[k + n] == *w)));
        let between = i >= 1 && is(i - 1, &["between"]) && is(i + 1, &["and"]) && j == i + 2;
        if from_to || reaches || between {
            out.asks.push(Ask::Path(a.clone(), b.clone()));
            return;
        }
    }
}

/// Whether a sentence asks where in the code a problem is: which files or functions would
/// have to change, where the bug is. The answer to that is a ranking of files, and it is
/// only given when it is asked for: a report that comes with a request to fix it asks for
/// the fix, and a ranking beside it was measured to cost more than it gave.
fn asks_where(text: &str) -> bool {
    // Plain words only, and the word that opens the sentence. A name of the code (`*Change`,
    // `BUG:` in a quoted log line) is not the word it resembles.
    let low: Vec<String> = text
        .split_whitespace()
        .enumerate()
        .map(|(i, w)| {
            let b = bare(w);
            let plain = b.chars().all(|c| c.is_ascii_lowercase()) || (i == 0 && b.chars().skip(1).all(|c| c.is_ascii_lowercase()) && !w.contains('*'));
            if plain { b.to_lowercase() } else { String::new() }
        })
        .collect();
    let is = |i: usize, set: &[&str]| low.get(i).is_some_and(|w| set.contains(&w.as_str()));
    let any = |set: &[&str]| low.iter().any(|w| set.contains(&w.as_str()));
    const PLACE: [&str; 9] = ["file", "files", "function", "functions", "place", "places", "location", "locations", "code"];
    // Not `fault`, `change`, `patch` or `root`: a page fault, a change and a root are things
    // code is about, and "which functions make the call on a page fault" asks for callers.
    const FAULT: [&str; 18] = [
        "fix", "fixed", "fixing", "changed", "modify", "modified", "patched", "bug", "faulty", "defect", "responsible", "cause", "causes", "caused", "causing", "culprit",
        "regression", "broken",
    ];
    const PROBLEM: [&str; 9] = ["bug", "fault", "problem", "issue", "defect", "regression", "error", "crash", "failure"];
    for i in 0..low.len() {
        // "find the source files that would have to be changed to fix it"
        if is(i, &["find", "identify", "locate", "determine", "name", "list"]) && (i + 1..(i + 7).min(low.len())).any(|j| is(j, &PLACE)) && any(&FAULT) {
            return true;
        }
        // "which files does this bug come from", "which functions are responsible"
        if is(i, &["which", "what"]) && (i + 1..(i + 4).min(low.len())).any(|j| is(j, &["file", "files", "function", "functions"])) && any(&FAULT) {
            return true;
        }
        // "where is the bug", "where does the problem come from"
        if is(i, &["where"]) && (i + 1..(i + 5).min(low.len())).any(|j| is(j, &PROBLEM)) {
            return true;
        }
        // "localise the fault", "locate the root cause"
        if is(i, &["localize", "localise", "locate"]) && (i + 1..(i + 4).min(low.len())).any(|j| is(j, &PROBLEM) || is(j, &["root"])) {
            return true;
        }
    }
    false
}

/// The symbols and paths a prompt names, in the order they appear.
pub fn mentions(prompt: &str) -> Mentions {
    let mut end = prompt.len().min(MAX_PROMPT);
    while !prompt.is_char_boundary(end) {
        end -= 1;
    }
    let text = &prompt[..end];
    let mut out = Mentions::default();
    out.rename = text.split(|c: char| !c.is_alphanumeric()).any(|w| ["rename", "renames", "renamed", "renaming"].iter().any(|r| w.eq_ignore_ascii_case(r)));
    phrases(&mut out, text);
    // Backtick spans first: what the author marked as code.
    let mut rest = text;
    let mut plain = String::with_capacity(text.len());
    while let Some(a) = rest.find('`') {
        plain.push_str(&rest[..a]);
        plain.push(' ');
        let after = &rest[a + 1..];
        match after.find('`') {
            Some(b) if b <= 160 && !after[..b].contains('\n') => {
                for w in after[..b].split_whitespace() {
                    word(&mut out, w, true);
                }
                rest = &after[b + 1..];
            }
            _ => rest = after,
        }
    }
    plain.push_str(rest);
    for w in plain.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '=' | '"' | '\'' | '[' | ']' | '{' | '}' | '|')) {
        word(&mut out, w, false);
    }
    let ask = text.len() <= ASK_MAX_PROMPT && !out.rename;
    let mut where_asked = false;
    for s in sentences(text) {
        sentence(&mut out, s, ask);
        where_asked |= asks_where(s);
    }
    // Asked of a prompt of any length: the report it is asked about is the prompt.
    if where_asked && !out.rename && out.asks.is_empty() {
        out.asks.push(Ask::Locate);
    }
    // A type that is named only as the owner of a member the prompt is about (`the function
    // parse in class Parser`, `the mnt_ns field of struct mount`) is not a second subject.
    let owners: Vec<String> = out.symbols.iter().filter_map(|m| m.owner.clone()).collect();
    out.symbols.retain(|m| !(m.is_type && m.owner.is_none() && owners.contains(&m.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(p: &str) -> Vec<String> {
        mentions(p).symbols.into_iter().map(|m| format!("{}{}{}", m.owner.map(|o| o + ".").unwrap_or_default(), m.name, if m.explicit { "!" } else { "" })).collect()
    }

    #[test]
    fn names_written_as_code() {
        assert_eq!(names("`parse_duration` returns the wrong sign for negative input."), ["parse_duration!"]);
        assert_eq!(names("Why does Collection.Add copy the slice? See also sync_all() and NewCollection."), ["Collection.Add!", "sync_all!", "NewCollection"]);
        assert_eq!(names("The crash is in vfs_read when the buffer is empty"), ["vfs_read"]);
        assert_eq!(names("fix `Foo::bar` and a->b"), ["Foo.bar!"]);
    }

    #[test]
    fn names_said_in_words() {
        let m = mentions("Rename the method Add of *Collection (defined in syft/pkg/collection.go) to AddEntry: the definition and every place that calls it.");
        assert_eq!(m.symbols[0], Mention { owner: Some("Collection".into()), name: "Add".into(), explicit: true, is_type: false });
        assert_eq!(m.paths, ["syft/pkg/collection.go"]);
        assert!(m.symbols.iter().any(|s| s.name == "AddEntry"));
        assert_eq!(names("the function parse in class Parser is slow"), ["Parser.parse!"]);
    }

    #[test]
    fn test_ids_and_paths() {
        let m = mentions("Failing tests: template_tests.syntax_tests.test_include.IncludeTests.test_include_recursive\nHow to run them: `cd tests && python runtests.py template_tests.syntax_tests.test_include`");
        assert!(m.symbols.contains(&Mention { owner: Some("IncludeTests".into()), name: "test_include_recursive".into(), explicit: true, is_type: false }));
        assert_eq!(mentions("look at src/query/refs.rs and README.md and main.py").paths, ["src/query/refs.rs", "main.py"]);
    }

    fn asks(p: &str) -> Vec<Ask> {
        mentions(p).asks
    }

    #[test]
    fn a_question_about_what_a_function_calls() {
        let callees = |n: &str| vec![Ask::Callees(n.to_string())];
        assert_eq!(asks("What does vfs_get_tree() call directly?"), callees("vfs_get_tree"));
        assert_eq!(asks("What functions does __do_fault() in mm/memory.c call?"), callees("__do_fault"));
        assert_eq!(asks("What does the scheduler tick handler sched_tick() call directly?"), callees("sched_tick"));
        assert_eq!(asks("List the functions that tcp_send_fin calls."), callees("tcp_send_fin"));
        assert_eq!(asks("Which functions are called by io_issue_sqe()?"), callees("io_issue_sqe"));
        assert_eq!(asks("Does Collection.Add call growSlice?"), callees("Collection.Add"));
        assert_eq!(asks("Which functions does the method Add of *Collection (defined in syft/pkg/collection.go) call directly?"), callees("Collection.Add"));
        assert_eq!(asks("Which functions of this repository does the function resolveRefs (defined in plumbing/revlist.go) call?"), callees("resolveRefs"));
        assert_eq!(asks("Which functions does the method Encode of UploadHaves (defined in plumbing/protocol/packp/uphav.go) call directly?"), callees("UploadHaves.Encode"));
        assert_eq!(asks("Which functions does the method Next of idxfileEntryIter (defined in plumbing/format/idxfile/idxfile.go) call?"), callees("idxfileEntryIter.Next"));
        // A clause between the function and the verb is another question.
        assert_eq!(asks("Why does parse_config fail when the tests call it twice?"), []);
        assert_eq!(asks("Which functions call the method Add of *Collection (defined in syft/pkg/collection.go)?"), []);
        // Who calls a function is another question, and the card for a symbol answers it.
        assert_eq!(asks("Which functions call wake_up_new_task()?"), []);
        assert_eq!(asks("Who calls folio_add_new_anon_rmap()? I need every caller before changing its arguments."), []);
        assert_eq!(asks("Where is inode_doinit_with_dentry() called from in SELinux?"), []);
        assert_eq!(asks("ext4_alloc_inode() is installed as a callback. Which function in the VFS calls it?"), []);
        assert_eq!(asks("Which functions send a TCP reset through tcp_send_active_reset()?"), []);
    }

    #[test]
    fn a_question_about_how_one_function_reaches_another() {
        let path = |a: &str, b: &str| vec![Ask::Path(a.to_string(), b.to_string())];
        assert_eq!(asks("How does an IORING_OP_READ request get from io_submit_sqes() to io_read()? Show the call chain."), path("io_submit_sqes", "io_read"));
        assert_eq!(asks("On a read fault in a tmpfs/shmem mapping, how does handle_pte_fault() end up in shmem_fault()?"), path("handle_pte_fault", "shmem_fault"));
        assert_eq!(asks("What is the call path from submit_bio() down to blk_mq_submit_bio() for a blk-mq device?"), path("submit_bio", "blk_mq_submit_bio"));
        assert_eq!(asks("When a file is opened, how does do_dentry_open() get to SELinux's avc_has_perm() check?"), path("do_dentry_open", "avc_has_perm"));
        assert_eq!(asks("Is there a call chain between `parse_config` and `open_file`?"), path("parse_config", "open_file"));
        // Two functions in one sentence are not a path between them.
        assert_eq!(asks("`dev_write` and `copy_all` and `dev_read` all need a timeout parameter."), []);
        assert_eq!(asks("Why is parse_config slower than open_file?"), []);
    }

    #[test]
    fn a_question_about_who_writes_a_member() {
        let writers = |m: &str| vec![Ask::Writers(m.to_string())];
        assert_eq!(asks("Which functions write the on_rq field of struct task_struct (p->on_rq)?"), writers("task_struct.on_rq"));
        assert_eq!(asks("Which functions assign struct request_queue's nr_requests?"), writers("request_queue.nr_requests"));
        assert_eq!(asks("Which functions set the mnt_ns field of struct mount?"), writers("mount.mnt_ns"));
        assert_eq!(asks("Which functions modify mm_struct's locked_vm counter?"), writers("mm_struct.locked_vm"));
        assert_eq!(asks("Which functions change the queue_count of struct nvme_ctrl?"), writers("nvme_ctrl.queue_count"));
        assert_eq!(asks("Which code writes the saved kernel stack pointer thread.sp (struct thread_struct, x86)?"), writers("thread_struct.sp"));
        assert_eq!(asks("Where is Config.Timeout set?"), writers("Config.Timeout"));
        // A member that is read, or a set that is a noun, is not that question.
        assert_eq!(asks("Which functions read the mnt_ns field of struct mount?"), []);
        assert_eq!(asks("What is the set of flags in struct mount?"), []);
    }

    #[test]
    fn a_question_about_where_a_problem_is() {
        let report = "The backlight keys do nothing on this laptop since the last update. ".repeat(40);
        for yes in [
            "Find the source files that would have to be changed to fix it. Do not fix it.",
            "Which files are most likely responsible for this bug?",
            "Where is the bug?",
            "Localise the fault and list the files.",
            "Can you identify the functions that cause this regression?",
        ] {
            assert_eq!(asks(&format!("{report}\n{yes}")), [Ask::Locate], "{yes}");
        }
        // A request to fix the thing is not a request to be told where it is, and neither is
        // a request to find something else.
        for no in [
            "Fix it in the source code. The tests that the project added for this change are not in the tree.",
            "Change the source code so that they pass.",
            "Find the tests for the parser and run them.",
            "Which functions call the method Action of *Change (defined in utils/merkletrie/change.go)?",
            "shmem_fault() is only installed in vm_ops tables. Which functions make the indirect call that reaches it on a page fault?",
            "An oops starts with \"BUG: unable to handle page fault\". Which function prints that line?",
            "Which version of the kernel is this?",
            "Write the body again, so that it does what the rest of the project expects of it.",
        ] {
            assert_eq!(asks(&format!("{report}\n{no}")), [], "{no}");
        }
    }

    #[test]
    fn a_long_report_and_a_request_to_rename_ask_nothing() {
        assert_eq!(asks("Rename the method Add of *Collection to AddEntry: the definition and every place that calls it. What does Add call?"), []);
        let report = format!("{}\nWhat does vfs_get_tree() call directly?", "The upload fails after a while and nothing is logged. ".repeat(60));
        assert_eq!(asks(&report), []);
    }

    #[test]
    fn a_type_said_with_its_kind_and_a_member_said_in_words() {
        let m = mentions("What members does struct kiocb have?");
        assert!(m.symbols.contains(&Mention { owner: None, name: "kiocb".into(), explicit: true, is_type: true }), "{:?}", m.symbols);
        let m = mentions("Which functions set the mnt_ns field of struct mount?");
        assert!(m.symbols.contains(&Mention { owner: Some("mount".into()), name: "mnt_ns".into(), explicit: true, is_type: false }), "{:?}", m.symbols);
        // A possessive that is not a type does not become an owner: the name stands alone.
        let m = mentions("Where is SELinux's avc_has_perm function defined?");
        assert!(m.symbols.contains(&Mention { owner: None, name: "avc_has_perm".into(), explicit: false, is_type: false }), "{:?}", m.symbols);
        // Prose after a kind word is not a name.
        assert_eq!(names("Which class of bugs is this? The struct is too large."), Vec::<String>::new());
    }

    #[test]
    fn the_words_beside_a_symbol() {
        let m = mentions("What is the signature of copy_thread on x86?");
        let (name, words) = &m.beside[0];
        assert_eq!(name, "copy_thread");
        assert!(words.contains(&"x86".to_string()) && !words.contains(&"copy_thread".to_string()), "{words:?}");
        let m = mentions("ext4 registers a slab constructor called init_once for its inode cache. Where is ext4's init_once defined?");
        assert!(m.beside.iter().any(|(n, w)| n == "init_once" && w.contains(&"ext4".to_string())), "{:?}", m.beside);
    }

    #[test]
    fn prose_is_not_code() {
        assert_eq!(names("Please make the tests pass. Stop when done, e.g. after the first run. Add a note."), Vec::<String>::new());
        assert_eq!(names("Update the README and the API docs for version 2.0 of the HTTP client"), Vec::<String>::new());
        assert_eq!(names("Work only inside /Users/me/project/repo and do not use the web."), Vec::<String>::new());
    }
}
