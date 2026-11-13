//! Reading a user's prompt for the symbols it names.
//!
//! A request such as "rename the method Add of *Collection (defined in syft/pkg/collection.go)"
//! or "`parse_duration` returns the wrong sign" names code. The words that do so have a shape
//! prose does not have (an underscore, an inner capital, a dot between two names, backticks), or
//! stand in a phrase that says what they are ("the method X of Y"). This module finds them. It
//! decides nothing about the repository: whether a word is a symbol is for the index to say.

/// A word of the prompt that may name a symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Mention {
    /// The type, module or class the name was qualified with: `Collection` in `Collection.Add`.
    pub owner: Option<String>,
    pub name: String,
    /// Written as code: in backticks, with an owner, with `()`, or in a "method X of Y" phrase.
    pub explicit: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Mentions {
    pub symbols: Vec<Mention>,
    /// Words that look like file paths (`pkg/collection.go`, `collection.go`).
    pub paths: Vec<String>,
    /// The prompt asks for something to be renamed.
    pub rename: bool,
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
            push(out, Mention { owner, name, explicit: true });
        }
        return;
    }
    if is_ident(w) && (quoted || called || code_shaped(w)) && w.len() >= 3 {
        push(out, Mention { owner: None, name: w.to_string(), explicit: quoted || called });
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
            push(out, Mention { owner: Some(owner.to_string()), name: name.to_string(), explicit: true });
        }
    }
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
        assert_eq!(m.symbols[0], Mention { owner: Some("Collection".into()), name: "Add".into(), explicit: true });
        assert_eq!(m.paths, ["syft/pkg/collection.go"]);
        assert!(m.symbols.iter().any(|s| s.name == "AddEntry"));
        assert_eq!(names("the function parse in class Parser is slow"), ["Parser.parse!"]);
    }

    #[test]
    fn test_ids_and_paths() {
        let m = mentions("Failing tests: template_tests.syntax_tests.test_include.IncludeTests.test_include_recursive\nHow to run them: `cd tests && python runtests.py template_tests.syntax_tests.test_include`");
        assert!(m.symbols.contains(&Mention { owner: Some("IncludeTests".into()), name: "test_include_recursive".into(), explicit: true }));
        assert_eq!(mentions("look at src/query/refs.rs and README.md and main.py").paths, ["src/query/refs.rs", "main.py"]);
    }

    #[test]
    fn prose_is_not_code() {
        assert_eq!(names("Please make the tests pass. Stop when done, e.g. after the first run. Add a note."), Vec::<String>::new());
        assert_eq!(names("Update the README and the API docs for version 2.0 of the HTTP client"), Vec::<String>::new());
        assert_eq!(names("Work only inside /Users/me/project/repo and do not use the web."), Vec::<String>::new());
    }
}
