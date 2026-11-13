//! Editing JSON configuration files in place.
//!
//! A harness's settings file belongs to its user. It has its own key order and formatting and,
//! in many harnesses, comments and trailing commas (JSONC). An edit made here touches only the
//! bytes of the member it sets and leaves every other byte as it was. Each edit is then checked
//! before it is handed back: the new text must parse, must hold the intended value at the
//! intended place, and must equal the old document everywhere else.

use anyhow::{Result, bail};
use serde_json::{Map, Value};

const MAX_DEPTH: usize = 128;

fn syntax(what: &str, at: usize) -> anyhow::Error {
    anyhow::anyhow!("not valid JSON: {what} at byte {at}")
}

/// Skip whitespace and comments.
fn skip_trivia(b: &[u8], mut i: usize) -> Result<usize> {
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b.get(i) != Some(&b'/') {
            return Ok(i);
        }
        match b.get(i + 1) {
            Some(b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            Some(b'*') => match memchr::memmem::find(&b[i + 2..], b"*/") {
                Some(p) => i += 2 + p + 2,
                None => return Err(syntax("unterminated comment", i)),
            },
            _ => return Err(syntax("unexpected `/`", i)),
        }
    }
}

/// End of the string that starts at `b[i] == b'"'`.
fn string_end(b: &[u8], i: usize) -> Result<usize> {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'"' => return Ok(j + 1),
            b'\\' => j += 2,
            b'\n' => break,
            _ => j += 1,
        }
    }
    Err(syntax("unterminated string", i))
}

#[derive(Debug)]
struct Member {
    key: String,
    key_start: usize,
    value: Node,
}

#[derive(Debug)]
enum Kind {
    Object(Vec<Member>),
    Array(Vec<Node>),
    Scalar,
}

/// A value and the bytes it occupies.
#[derive(Debug)]
struct Node {
    start: usize,
    end: usize,
    kind: Kind,
}

fn parse_node(b: &[u8], i: usize, depth: usize) -> Result<Node> {
    if depth > MAX_DEPTH {
        return Err(syntax("nesting too deep", i));
    }
    match b.get(i) {
        Some(b'{') => {
            let mut members = Vec::new();
            let mut j = skip_trivia(b, i + 1)?;
            loop {
                match b.get(j) {
                    Some(b'}') => return Ok(Node { start: i, end: j + 1, kind: Kind::Object(members) }),
                    Some(b'"') => {}
                    _ => return Err(syntax("expected a key or `}`", j)),
                }
                let key_end = string_end(b, j)?;
                let key: String = std::str::from_utf8(&b[j..key_end]).ok().and_then(|s| serde_json::from_str(s).ok()).ok_or_else(|| syntax("bad key", j))?;
                let colon = skip_trivia(b, key_end)?;
                if b.get(colon) != Some(&b':') {
                    return Err(syntax("expected `:`", colon));
                }
                let value = parse_node(b, skip_trivia(b, colon + 1)?, depth + 1)?;
                let after = skip_trivia(b, value.end)?;
                members.push(Member { key, key_start: j, value });
                j = match b.get(after) {
                    Some(b',') => skip_trivia(b, after + 1)?,
                    Some(b'}') => after,
                    _ => return Err(syntax("expected `,` or `}`", after)),
                };
            }
        }
        Some(b'[') => {
            let mut items = Vec::new();
            let mut j = skip_trivia(b, i + 1)?;
            loop {
                if b.get(j) == Some(&b']') {
                    return Ok(Node { start: i, end: j + 1, kind: Kind::Array(items) });
                }
                let item = parse_node(b, j, depth + 1)?;
                let after = skip_trivia(b, item.end)?;
                items.push(item);
                j = match b.get(after) {
                    Some(b',') => skip_trivia(b, after + 1)?,
                    Some(b']') => after,
                    _ => return Err(syntax("expected `,` or `]`", after)),
                };
            }
        }
        Some(b'"') => Ok(Node { start: i, end: string_end(b, i)?, kind: Kind::Scalar }),
        Some(_) => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']' | b'/' | b':') && !b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j == i {
                return Err(syntax("expected a value", i));
            }
            Ok(Node { start: i, end: j, kind: Kind::Scalar })
        }
        None => Err(syntax("unexpected end", i)),
    }
}

fn body(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn parse_document(text: &str) -> Result<Node> {
    let b = text.as_bytes();
    let start = skip_trivia(b, text.len() - body(text).len())?;
    let root = parse_node(b, start, 0)?;
    let end = skip_trivia(b, root.end)?;
    if end != b.len() {
        return Err(syntax("text after the document", end));
    }
    Ok(root)
}

/// Rewrite JSON-with-comments as plain JSON: comments become spaces, trailing commas go.
fn strict(text: &str) -> Result<String> {
    let b = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let end = string_end(b, i)?;
                out.extend_from_slice(&b[i..end]);
                i = end;
            }
            b'/' => {
                let next = skip_trivia(b, i)?;
                out.push(b' ');
                i = next;
            }
            b',' => {
                let next = skip_trivia(b, i + 1)?;
                if !matches!(b.get(next), Some(b'}') | Some(b']')) {
                    out.push(b',');
                }
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| anyhow::anyhow!("not valid UTF-8"))
}

/// Parse a JSON or JSONC document. A file that is empty counts as an empty object.
pub fn parse(text: &str) -> Result<Value> {
    let text = body(text);
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(&strict(text)?).map_err(|e| anyhow::anyhow!("not valid JSON: {e}"))
}

/// The value at `path`, if the document has one.
pub fn get(text: &str, path: &[&str]) -> Result<Option<Value>> {
    let doc = parse(text)?;
    let mut v = &doc;
    for key in path {
        match v.get(key) {
            Some(next) => v = next,
            None => return Ok(None),
        }
    }
    Ok(Some(v.clone()))
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// The leading whitespace of the line that holds byte `pos`.
fn indent_at(text: &str, pos: usize) -> &str {
    let line = text[..pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let rest = &text[line..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// Whether `pos` is the first thing on its line.
fn starts_line(text: &str, pos: usize) -> bool {
    let line = text[..pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
    text[line..pos].chars().all(|c| c == ' ' || c == '\t')
}

/// One level of indentation as the document writes it.
fn unit(text: &str) -> String {
    for line in text.lines() {
        let lead = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
        if !lead.is_empty() && lead.len() < line.len() {
            return if lead.starts_with('\t') { "\t".to_string() } else { lead.to_string() };
        }
    }
    "  ".to_string()
}

/// Keys in the order a person would write a server or hook entry; the rest follow by name.
const KEY_ORDER: &[&str] =
    &["type", "name", "matcher", "command", "cmd", "args", "env", "environment", "envs", "enabled", "disabled", "tools", "timeout", "hooks", "description"];

fn ordered(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut members: Vec<(&String, &Value)> = map.iter().collect();
    members.sort_by_key(|(k, _)| (KEY_ORDER.iter().position(|o| o == k).unwrap_or(KEY_ORDER.len()), k.as_str()));
    members
}

/// `value` on one line, with a space after every `:` and `,`.
fn compact(value: &Value, out: &mut String) {
    match value {
        Value::Object(m) if !m.is_empty() => {
            out.push('{');
            for (i, (k, v)) in ordered(m).into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push_str(": ");
                compact(v, out);
            }
            out.push('}');
        }
        Value::Array(a) if !a.is_empty() => {
            out.push('[');
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                compact(v, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// `value` on several lines; each line starts with one tab per nesting level.
fn pretty(value: &Value, level: usize, out: &mut String) {
    let line = |out: &mut String, level: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n('\t', level));
    };
    match value {
        Value::Object(m) if !m.is_empty() => {
            out.push('{');
            let members = ordered(m);
            let n = members.len();
            for (i, (k, v)) in members.into_iter().enumerate() {
                line(out, level + 1);
                out.push_str(&Value::String(k.clone()).to_string());
                out.push_str(": ");
                pretty(v, level + 1, out);
                if i + 1 < n {
                    out.push(',');
                }
            }
            line(out, level);
            out.push('}');
        }
        // A short list of plain values reads best on one line: `"args": ["mcp"]`.
        Value::Array(a) if !a.is_empty() && (a.iter().any(|v| v.is_object() || v.is_array()) || value.to_string().len() > 72) => {
            out.push('[');
            for (i, v) in a.iter().enumerate() {
                line(out, level + 1);
                pretty(v, level + 1, out);
                if i + 1 < a.len() {
                    out.push(',');
                }
            }
            line(out, level);
            out.push(']');
        }
        other => compact(other, out),
    }
}

/// `value` as text. On several lines when `multiline`, with continuation lines indented by
/// `indent` plus the document's own unit per level.
fn render(value: &Value, multiline: bool, indent: &str, unit: &str, nl: &str) -> String {
    let mut text = String::new();
    if !multiline {
        compact(value, &mut text);
        return text;
    }
    pretty(value, 0, &mut text);
    let mut out = String::with_capacity(text.len() + 64);
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            out.push_str(nl);
            out.push_str(indent);
        }
        let body = line.trim_start_matches('\t');
        for _ in 0..line.len() - body.len() {
            out.push_str(unit);
        }
        out.push_str(body);
    }
    out
}

/// `value` as an indented document, keys in the order a person would write them.
pub fn pretty_text(value: &Value) -> String {
    render(value, true, "", "  ", "\n")
}

/// Insert `item` (a rendered member or element) as the last child of the container `node`.
/// `last` is the last existing child: where it starts and ends.
fn insert_child(text: &str, node: &Node, last: Option<(usize, usize)>, item: &dyn Fn(bool, &str) -> String, unit: &str, nl: &str) -> Result<String> {
    let b = text.as_bytes();
    let close = node.end - 1;
    // The whitespace run before the closing bracket stays where it is, after the new child.
    let mut tail = close;
    while tail > node.start + 1 && b[tail - 1].is_ascii_whitespace() {
        tail -= 1;
    }
    let multiline = text[node.start..node.end].contains('\n') || (last.is_none() && text.contains('\n'));
    let outer = indent_at(text, node.start).to_string();
    match last {
        Some((start, end)) => {
            let comma = skip_trivia(b, end)?;
            let has_comma = b.get(comma) == Some(&b',');
            let child_indent = if starts_line(text, start) { indent_at(text, start).to_string() } else { format!("{outer}{unit}") };
            let lead = if multiline { format!("{nl}{child_indent}") } else { " ".to_string() };
            let mut out = String::with_capacity(text.len() + 256);
            out.push_str(&text[..end]);
            if !has_comma {
                out.push(',');
            }
            out.push_str(&text[end..tail]);
            out.push_str(&lead);
            out.push_str(&item(multiline, &child_indent));
            out.push_str(&text[tail..]);
            Ok(out)
        }
        None => {
            let child_indent = format!("{outer}{unit}");
            let mut out = String::with_capacity(text.len() + 256);
            out.push_str(&text[..tail]);
            if multiline {
                out.push_str(nl);
                out.push_str(&child_indent);
                out.push_str(&item(true, &child_indent));
                // An empty container on one line gets its closing bracket on a line of its own.
                if !text[tail..close].contains('\n') {
                    out.push_str(nl);
                    out.push_str(&outer);
                }
                out.push_str(text[tail..].trim_start_matches([' ', '\t']));
            } else {
                out.push_str(&item(false, &child_indent));
                out.push_str(&text[tail..]);
            }
            Ok(out)
        }
    }
}

/// `value` wrapped in one object per key of `path`.
fn nested(path: &[&str], value: &Value) -> Value {
    path.iter().rev().fold(value.clone(), |inner, key| {
        let mut m = Map::new();
        m.insert((*key).to_string(), inner);
        Value::Object(m)
    })
}

/// The document without the value at `path`, and without the parents that only existed for it.
fn without(mut doc: Value, path: &[&str]) -> Value {
    fn strip(v: &mut Value, path: &[&str]) {
        let Some((first, rest)) = path.split_first() else { return };
        let Some(map) = v.as_object_mut() else { return };
        if rest.is_empty() {
            map.remove(*first);
        } else if let Some(child) = map.get_mut(*first) {
            strip(child, rest);
            if child.as_object().is_some_and(Map::is_empty) {
                map.remove(*first);
            }
        }
    }
    strip(&mut doc, path);
    doc
}

fn check(old: &str, new: &str, path: &[&str], value: &Value) -> Result<()> {
    if get(new, path)?.as_ref() != Some(value) || without(parse(old)?, path) != without(parse(new)?, path) {
        bail!("the edit could not be verified; the file was left unchanged");
    }
    Ok(())
}

/// Set the value at `path`, creating the objects on the way, and change nothing else.
pub fn set(text: &str, path: &[&str], value: &Value) -> Result<String> {
    let (nl, unit) = (newline(text), unit(text));
    if body(text).trim().is_empty() {
        return Ok(format!("{}{nl}", render(&nested(path, value), true, "", &unit, nl)));
    }
    let root = parse_document(text)?;
    let mut node = &root;
    let mut new: Option<String> = None;
    for (depth, key) in path.iter().enumerate() {
        let Kind::Object(members) = &node.kind else {
            if depth == 0 {
                bail!("the top level of the file is not a JSON object");
            }
            bail!("`{}` is not an object", path[..depth].join("."));
        };
        let last = depth + 1 == path.len();
        // Parsers keep the last of two members with one name.
        match members.iter().rev().find(|m| m.key == *key) {
            Some(m) if last => {
                let multiline = text[node.start..node.end].contains('\n');
                let rendered = render(value, multiline, indent_at(text, m.key_start), &unit, nl);
                new = Some(format!("{}{}{}", &text[..m.value.start], rendered, &text[m.value.end..]));
                break;
            }
            Some(m) => node = &m.value,
            None => {
                let inner = nested(&path[depth + 1..], value);
                let name = serde_json::to_string(key)?;
                let item = |multiline: bool, indent: &str| format!("{name}: {}", render(&inner, multiline, indent, &unit, nl));
                let last_child = members.last().map(|m| (m.key_start, m.value.end));
                new = Some(insert_child(text, node, last_child, &item, &unit, nl)?);
                break;
            }
        }
    }
    let Some(new) = new else { bail!("empty path") };
    check(text, &new, path, value)?;
    Ok(new)
}

/// Put `item` into the array at `path`: in place of the first element `ours` recognises, or at
/// the end. The array and its parents are created when missing.
pub fn upsert_item(text: &str, path: &[&str], item: &Value, ours: &dyn Fn(&Value) -> bool) -> Result<String> {
    let mut want: Vec<Value> = match get(text, path)? {
        None => return set(text, path, &Value::Array(vec![item.clone()])),
        Some(Value::Array(items)) => items,
        Some(_) => bail!("`{}` is not an array", path.join(".")),
    };
    let at = want.iter().position(ours);
    match at {
        Some(i) if want[i] == *item => return Ok(text.to_string()),
        Some(i) => want[i] = item.clone(),
        None => want.push(item.clone()),
    }
    let (nl, unit) = (newline(text), unit(text));
    let root = parse_document(text)?;
    let mut node = &root;
    for key in path {
        let Kind::Object(members) = &node.kind else { bail!("`{}` is not reachable", path.join(".")) };
        node = &members.iter().rev().find(|m| m.key == *key).ok_or_else(|| anyhow::anyhow!("`{}` is not reachable", path.join(".")))?.value;
    }
    let Kind::Array(items) = &node.kind else { bail!("`{}` is not an array", path.join(".")) };
    let new = match at {
        Some(i) => {
            let old = &items[i];
            let multiline = text[node.start..node.end].contains('\n');
            format!("{}{}{}", &text[..old.start], render(item, multiline, indent_at(text, old.start), &unit, nl), &text[old.end..])
        }
        None => {
            let render_item = |multiline: bool, indent: &str| render(item, multiline, indent, &unit, nl);
            insert_child(text, node, items.last().map(|n| (n.start, n.end)), &render_item, &unit, nl)?
        }
    };
    check(text, &new, path, &Value::Array(want))?;
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry() -> Value {
        json!({"command": "/usr/local/bin/codefast", "args": ["mcp"]})
    }

    #[test]
    fn a_new_file_is_a_plain_pretty_document() {
        let text = set("", &["mcpServers", "codefast"], &entry()).unwrap();
        assert_eq!(
            text,
            "{\n  \"mcpServers\": {\n    \"codefast\": {\n      \"command\": \"/usr/local/bin/codefast\",\n      \"args\": [\"mcp\"]\n    }\n  }\n}\n"
        );
        assert_eq!(set("  \n", &["a"], &json!(1)).unwrap(), "{\n  \"a\": 1\n}\n");
        assert_eq!(set("{}", &["a"], &json!(1)).unwrap(), "{\"a\": 1}");
        assert_eq!(set("{}\n", &["a"], &json!(1)).unwrap(), "{\n  \"a\": 1\n}\n");
    }

    #[test]
    fn everything_else_stays_byte_for_byte() {
        let old = "// Zed settings\n{\n    \"theme\": \"One Dark\", // the best one\n    /* servers */\n    \"context_servers\": {\n        \"other\": {\"command\": \"x\", \"args\": []},\n    },\n    \"vim_mode\": true,\n}\n";
        let new = set(old, &["context_servers", "codefast"], &entry()).unwrap();
        assert_eq!(
            new,
            "// Zed settings\n{\n    \"theme\": \"One Dark\", // the best one\n    /* servers */\n    \"context_servers\": {\n        \"other\": {\"command\": \"x\", \"args\": []},\n        \"codefast\": {\n            \"command\": \"/usr/local/bin/codefast\",\n            \"args\": [\"mcp\"]\n        }\n    },\n    \"vim_mode\": true,\n}\n"
        );
        // Setting it again changes nothing; setting another value replaces only the value.
        assert_eq!(set(&new, &["context_servers", "codefast"], &entry()).unwrap(), new);
        let moved = set(&new, &["context_servers", "codefast"], &json!({"command": "codefast", "args": ["mcp"]})).unwrap();
        assert_eq!(moved.replace("\"command\": \"codefast\"", "\"command\": \"/usr/local/bin/codefast\""), new);
        assert!(moved.starts_with("// Zed settings\n{\n    \"theme\": \"One Dark\", // the best one\n"));
    }

    #[test]
    fn members_are_added_where_the_file_would_put_them() {
        // After the last member, before a closing brace on its own line, tabs kept.
        let old = "{\n\t\"a\": 1,\n\t\"mcp\": {\n\t\t\"x\": {}\n\t}\n}\n";
        assert_eq!(
            set(old, &["mcp", "y"], &json!({"k": true})).unwrap(),
            "{\n\t\"a\": 1,\n\t\"mcp\": {\n\t\t\"x\": {},\n\t\t\"y\": {\n\t\t\t\"k\": true\n\t\t}\n\t}\n}\n"
        );
        // A missing parent is created at the end of the top level.
        assert_eq!(set("{\n  \"a\": 1\n}\n", &["mcp", "y"], &json!(2)).unwrap(), "{\n  \"a\": 1,\n  \"mcp\": {\n    \"y\": 2\n  }\n}\n");
        // An empty parent is opened up.
        assert_eq!(set("{\n  \"mcp\": {}\n}\n", &["mcp", "y"], &json!(2)).unwrap(), "{\n  \"mcp\": {\n    \"y\": 2\n  }\n}\n");
        assert_eq!(set("{\n  \"mcp\": {\n  }\n}\n", &["mcp", "y"], &json!(2)).unwrap(), "{\n  \"mcp\": {\n    \"y\": 2\n  }\n}\n");
        // A one-line document stays on one line.
        assert_eq!(set("{\"a\": 1}", &["b", "c"], &json!([1])).unwrap(), "{\"a\": 1, \"b\": {\"c\": [1]}}");
        // A comment after the last member stays attached to it.
        assert_eq!(set("{\n  \"a\": 1 // one\n}\n", &["b"], &json!(2)).unwrap(), "{\n  \"a\": 1, // one\n  \"b\": 2\n}\n");
        // Windows line endings are kept.
        assert_eq!(set("{\r\n  \"a\": 1\r\n}\r\n", &["b"], &json!(2)).unwrap(), "{\r\n  \"a\": 1,\r\n  \"b\": 2\r\n}\r\n");
        // A key with a dot in it is one key.
        assert_eq!(get(&set("{\n}\n", &["amp.mcpServers", "codefast"], &entry()).unwrap(), &["amp.mcpServers", "codefast"]).unwrap(), Some(entry()));
        // A byte order mark and strings full of brackets are no obstacle.
        let odd = "\u{feff}{\n  \"s\": \"}{][ // not a comment \\\" /*\",\n  \"t\": []\n}\n";
        assert_eq!(set(odd, &["u"], &json!(null)).unwrap(), "\u{feff}{\n  \"s\": \"}{][ // not a comment \\\" /*\",\n  \"t\": [],\n  \"u\": null\n}\n");
    }

    #[test]
    fn what_cannot_be_edited_safely_is_refused() {
        for (text, path) in [
            ("[1, 2]", vec!["a"]),
            ("\"text\"", vec!["a"]),
            ("{\"a\": 1}", vec!["a", "b"]),
            ("{\"a\": [1]}", vec!["a", "b"]),
            ("{\"a\": 1", vec!["b"]),
            ("{\"a\": 1} {}", vec!["b"]),
            ("{a: 1}", vec!["b"]),
            ("{\"a\": 1 /* open", vec!["b"]),
            ("not json at all", vec!["b"]),
        ] {
            assert!(set(text, &path, &json!(1)).is_err(), "{text}");
        }
        assert!(parse("{\"a\": }").is_err());
        assert_eq!(parse("").unwrap(), json!({}));
        assert_eq!(parse("{\"a\": [1, 2,], /* c */ \"b\": \"//x\", }").unwrap(), json!({"a": [1, 2], "b": "//x"}));
        assert_eq!(get("{\"a\": {\"b\": 1}}", &["a", "b"]).unwrap(), Some(json!(1)));
        assert_eq!(get("{\"a\": {\"b\": 1}}", &["a", "c"]).unwrap(), None);
        assert_eq!(get("{\"a\": 3}", &["a", "c"]).unwrap(), None);
    }

    #[test]
    fn array_items_are_added_once_and_replaced_in_place() {
        let ours = |v: &Value| v["hooks"][0]["command"].as_str().is_some_and(|c| c.contains("codefast"));
        let item = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "codefast hook post-tool-use"}]});
        let old = "{\n  \"hooks\": {\n    \"PostToolUse\": [\n      {\"matcher\": \"Edit\", \"hooks\": [{\"type\": \"command\", \"command\": \"fmt\"}]}\n    ]\n  }\n}\n";
        let new = upsert_item(old, &["hooks", "PostToolUse"], &item, &ours).unwrap();
        assert_eq!(
            new,
            "{\n  \"hooks\": {\n    \"PostToolUse\": [\n      {\"matcher\": \"Edit\", \"hooks\": [{\"type\": \"command\", \"command\": \"fmt\"}]},\n      {\n        \"matcher\": \"Bash\",\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"codefast hook post-tool-use\"\n          }\n        ]\n      }\n    ]\n  }\n}\n"
        );
        assert_eq!(upsert_item(&new, &["hooks", "PostToolUse"], &item, &ours).unwrap(), new, "idempotent");
        let changed = json!({"matcher": "Bash|Grep", "hooks": [{"type": "command", "command": "/opt/codefast hook post-tool-use"}]});
        let replaced = upsert_item(&new, &["hooks", "PostToolUse"], &changed, &ours).unwrap();
        assert_eq!(get(&replaced, &["hooks", "PostToolUse"]).unwrap().unwrap().as_array().unwrap().len(), 2);
        assert!(replaced.contains("\"/opt/codefast hook post-tool-use\"") && replaced.contains("\"command\": \"fmt\""));
        // The array and its parents appear when missing; an empty array is filled.
        let fresh = upsert_item("{\n  \"model\": \"x\"\n}\n", &["hooks", "SessionStart"], &item, &ours).unwrap();
        assert_eq!(get(&fresh, &["hooks", "SessionStart"]).unwrap(), Some(json!([item])));
        assert!(fresh.starts_with("{\n  \"model\": \"x\",\n  \"hooks\": {\n    \"SessionStart\": [\n      {"));
        let empty = upsert_item("{\"hooks\": {\"Stop\": []}}", &["hooks", "Stop"], &json!(1), &|_| false).unwrap();
        assert_eq!(empty, "{\"hooks\": {\"Stop\": [1]}}");
        assert!(upsert_item("{\"hooks\": {\"Stop\": {}}}", &["hooks", "Stop"], &json!(1), &|_| false).is_err());
    }
}
