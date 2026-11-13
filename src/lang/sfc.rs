//! Single-file components (`.vue`, `.svelte`, `.astro`): markup with script blocks in it.
//!
//! The script blocks are where a component's code is: its imports, functions and state. They
//! are JavaScript or TypeScript. Everything else is blanked out byte for byte, so the
//! JavaScript extractor reads the file with every line and offset where it is in the file.
//!
//! The markup then uses what the script declares: a handler, a helper, another component.
//! Those uses are added as references, read from the places where the markup holds an
//! expression (`{{ x }}`, `:prop="x"`, `@click="x"`, `{x}`, `<Component>`), not from its
//! text.

use super::{Lang, Parsers, Scratch, js};
use crate::facts::{Def, DefKind, FileFacts, Import, NONE, Ref, RefKind, dflag, iflag};
use rustc_hash::FxHashMap;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Vue,
    Svelte,
    Astro,
}

/// The kind of component file `path` is, by its extension.
pub fn kind(path: &str) -> Option<Kind> {
    let file = path.rsplit('/').next().unwrap_or(path);
    match file.rsplit_once('.')?.1 {
        "vue" => Some(Kind::Vue),
        "svelte" => Some(Kind::Svelte),
        "astro" => Some(Kind::Astro),
        _ => None,
    }
}

type Span = (usize, usize);

#[derive(Default, Debug, PartialEq)]
struct Parts {
    /// The contents of the script blocks (and of Astro's front matter).
    scripts: Vec<Span>,
    /// Neither script nor markup: styles, comments, scripts in another language.
    dead: Vec<Span>,
    /// A script block declares JSX.
    jsx: bool,
}

fn starts_with_at(src: &[u8], i: usize, what: &[u8]) -> bool {
    src.len() >= i + what.len() && src[i..i + what.len()].eq_ignore_ascii_case(what)
}

/// The end of the tag that opens at `i` (the index after its `>`), quotes respected.
fn tag_end(src: &[u8], mut i: usize) -> usize {
    let mut quote = 0u8;
    while i < src.len() {
        let b = src[i];
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            return i + 1;
        }
        i += 1;
    }
    src.len()
}

/// The value of attribute `name` in the text of an opening tag.
fn attr<'a>(tag: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    while let Some(at) = memchr::memmem::find(&tag[i..], name) {
        let s = i + at;
        i = s + name.len();
        let lead = s.checked_sub(1).map(|p| tag[p]);
        if !lead.is_some_and(|b| b.is_ascii_whitespace()) {
            continue;
        }
        let mut j = i;
        while tag.get(j).is_some_and(|b| b.is_ascii_whitespace()) {
            j += 1;
        }
        if tag.get(j) != Some(&b'=') {
            continue;
        }
        j += 1;
        while tag.get(j).is_some_and(|b| b.is_ascii_whitespace()) {
            j += 1;
        }
        return match tag.get(j) {
            Some(&q) if q == b'"' || q == b'\'' => {
                let end = memchr::memchr(q, &tag[j + 1..]).map_or(tag.len(), |e| j + 1 + e);
                Some(&tag[j + 1..end])
            }
            _ => {
                let end = tag[j..].iter().position(|b| b.is_ascii_whitespace() || *b == b'>').map_or(tag.len(), |e| j + e);
                Some(&tag[j..end])
            }
        };
    }
    None
}

/// Where the scripts of a component file are, and what is neither script nor markup.
fn parts(kind: Kind, src: &[u8]) -> Parts {
    let mut p = Parts::default();
    let mut i = 0;
    if kind == Kind::Astro {
        // Front matter: TypeScript between two `---` lines at the top of the file.
        let start = src.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(src.len());
        if starts_with_at(src, start, b"---") {
            let open = memchr::memchr(b'\n', &src[start..]).map_or(src.len(), |e| start + e + 1);
            let mut at = open;
            let mut close = None;
            while at < src.len() {
                let end = memchr::memchr(b'\n', &src[at..]).map_or(src.len(), |e| at + e);
                if src[at..end].trim_ascii() == b"---" {
                    close = Some((at, end));
                    break;
                }
                at = end + 1;
            }
            if let Some((c, end)) = close {
                p.scripts.push((open, c));
                i = end;
            }
        }
    }
    while let Some(at) = memchr::memchr(b'<', &src[i..]) {
        let s = i + at;
        if starts_with_at(src, s, b"<!--") {
            let end = memchr::memmem::find(&src[s..], b"-->").map_or(src.len(), |e| s + e + 3);
            p.dead.push((s, end));
            i = end;
            continue;
        }
        let block = [(&b"<script"[..], &b"</script"[..]), (&b"<style"[..], &b"</style"[..])]
            .into_iter()
            .find(|(open, _)| starts_with_at(src, s, open) && src.get(s + open.len()).is_some_and(|b| b.is_ascii_whitespace() || *b == b'>' || *b == b'/'));
        let Some((open, close)) = block else {
            i = s + 1;
            continue;
        };
        let body = tag_end(src, s);
        let tag = &src[s..body];
        let mut end = body;
        while end < src.len() && !starts_with_at(src, end, close) {
            end = memchr::memchr(b'<', &src[end + 1..]).map_or(src.len(), |e| end + 1 + e);
        }
        let lang = attr(tag, b"lang").or_else(|| attr(tag, b"type")).map(|v| v.to_ascii_lowercase());
        let script = open == b"<script"
            && !tag.ends_with(b"/>")
            && lang.as_deref().is_none_or(|l| {
                matches!(
                    l,
                    b"ts"
                        | b"tsx"
                        | b"js"
                        | b"jsx"
                        | b"typescript"
                        | b"javascript"
                        | b"module"
                        | b"text/javascript"
                        | b"text/typescript"
                        | b"application/javascript"
                )
            });
        if script {
            p.jsx |= lang.as_deref().is_some_and(|l| l == b"tsx" || l == b"jsx");
            p.scripts.push((body, end));
        } else {
            p.dead.push((body, end));
        }
        i = tag_end(src, end.min(src.len()));
    }
    p
}

/// The file with everything but its scripts blanked: the same length, the same lines.
fn masked(src: &[u8], scripts: &[Span]) -> Vec<u8> {
    let mut out: Vec<u8> = src.iter().map(|&b| if b == b'\n' || b == b'\r' { b } else { b' ' }).collect();
    for &(s, e) in scripts {
        out[s..e].copy_from_slice(&src[s..e]);
    }
    out
}

fn ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn ident_part(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// The index after the string that opens at `i`.
fn string_end(src: &[u8], i: usize, hi: usize) -> usize {
    let q = src[i];
    let mut j = i + 1;
    while j < hi {
        match src[j] {
            b'\\' => j += 1,
            b if b == q => return j + 1,
            _ => {}
        }
        j += 1;
    }
    hi
}

/// The index after the `}` that closes the `{` at `i`.
fn brace_end(src: &[u8], i: usize, hi: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < hi {
        match src[j] {
            b'"' | b'\'' | b'`' => {
                j = string_end(src, j, hi);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    hi
}

/// A use of a name in the markup.
struct Use {
    at: usize,
    end: usize,
    kind: RefKind,
}

/// The names an expression of the markup reads or calls. Property names (`a.b`, `{ b: 1 }`)
/// and the contents of strings are not uses.
fn expression(src: &[u8], lo: usize, hi: usize, handler: bool, out: &mut Vec<Use>) {
    let text = &src[lo..hi];
    let trimmed = text.trim_ascii();
    if handler && !trimmed.is_empty() && ident_start(trimmed[0]) && trimmed.iter().all(|&b| ident_part(b)) {
        // `@click="save"`: the handler is called when the event fires.
        let at = lo + (text.len() - text.trim_ascii_start().len());
        out.push(Use { at, end: at + trimmed.len(), kind: RefKind::Call });
        return;
    }
    let mut p = lo;
    // The last byte that was not white space, and where it stood.
    let mut prev = (0u8, 0usize);
    while p < hi {
        let c = src[p];
        if c == b'"' || c == b'\'' || c == b'`' {
            let end = string_end(src, p, hi);
            prev = (c, end - 1);
            p = end;
        } else if ident_start(c) {
            let mut q = p + 1;
            while q < hi && ident_part(src[q]) {
                q += 1;
            }
            let mut n = q;
            while n < hi && src[n].is_ascii_whitespace() {
                n += 1;
            }
            let next = if n < hi { src[n] } else { 0 };
            let spread = prev.0 == b'.' && prev.1 >= 2 && src[prev.1 - 1] == b'.' && src[prev.1 - 2] == b'.';
            let member = prev.0 == b'.' && !spread;
            let key = (prev.0 == b'{' || prev.0 == b',') && next == b':';
            if !member && !key {
                out.push(Use { at: p, end: q, kind: if next == b'(' { RefKind::Call } else { RefKind::Read } });
            }
            prev = (src[q - 1], q - 1);
            p = q;
        } else if c.is_ascii_digit() {
            // A number, with whatever letters belong to it.
            while p < hi && (ident_part(src[p]) || src[p] == b'.') {
                p += 1;
            }
            prev = (b'0', p - 1);
        } else {
            if !c.is_ascii_whitespace() {
                prev = (c, p);
            }
            p += 1;
        }
    }
}

/// `my-button` -> `MyButton`.
fn pascal(tag: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tag.len());
    let mut up = true;
    for &b in tag {
        if b == b'-' {
            up = true;
        } else if up {
            out.push(b.to_ascii_uppercase());
            up = false;
        } else {
            out.push(b);
        }
    }
    out
}

/// A component used as a tag: `<UserCard>`, `<user-card>`, `<Menu.Item>`.
struct Tag {
    at: usize,
    name: Vec<u8>,
    /// Written as a component name. A tag in kebab case is one only if the script knows it.
    given: bool,
}

/// Every use of a name in the markup between `lo` and `hi`.
fn markup(kind: Kind, src: &[u8], lo: usize, hi: usize, uses: &mut Vec<Use>, tags: &mut Vec<Tag>) {
    let mut i = lo;
    while i < hi {
        match src[i] {
            b'<' if src.get(i + 1).is_some_and(|b| b.is_ascii_alphabetic()) => {
                let mut j = i + 1;
                while j < hi && (src[j].is_ascii_alphanumeric() || matches!(src[j], b'_' | b'.' | b':' | b'-')) {
                    j += 1;
                }
                let name = &src[i + 1..j];
                if !name.contains(&b':') {
                    let first = name.split(|b| *b == b'.').next().unwrap_or(name);
                    if first[0].is_ascii_uppercase() {
                        tags.push(Tag { at: i + 1, name: first.to_vec(), given: true });
                    } else if first.contains(&b'-') && kind == Kind::Vue {
                        tags.push(Tag { at: i + 1, name: pascal(first), given: false });
                    }
                }
                i = attributes(kind, src, j, hi, uses);
            }
            b'{' if kind == Kind::Vue => {
                if src.get(i + 1) == Some(&b'{') {
                    let end = memchr::memmem::find(&src[i..hi], b"}}").map_or(hi, |e| i + e);
                    expression(src, i + 2, end, false, uses);
                    i = (end + 2).min(hi);
                } else {
                    i += 1;
                }
            }
            b'{' => {
                let end = brace_end(src, i, hi);
                expression(src, i + 1, end.saturating_sub(1).max(i + 1), false, uses);
                i = end;
            }
            _ => i += 1,
        }
    }
}

/// The attributes of the tag whose name ends at `i`. Returns the index after the tag's `>`.
fn attributes(kind: Kind, src: &[u8], mut i: usize, hi: usize, uses: &mut Vec<Use>) -> usize {
    while i < hi {
        let b = src[i];
        if b == b'>' {
            return i + 1;
        }
        if b.is_ascii_whitespace() || b == b'/' {
            i += 1;
            continue;
        }
        if b == b'{' {
            // `{name}` and `{...props}`.
            let end = brace_end(src, i, hi);
            expression(src, i + 1, end.saturating_sub(1).max(i + 1), false, uses);
            i = end;
            continue;
        }
        let start = i;
        while i < hi && !src[i].is_ascii_whitespace() && !matches!(src[i], b'=' | b'>') && !(src[i] == b'/' && src.get(i + 1) == Some(&b'>')) {
            i += 1;
        }
        if i == start {
            i += 1;
            continue;
        }
        let name = &src[start..i];
        if kind == Kind::Svelte {
            // `use:tooltip`, `transition:fade|local`: the name after the colon is a binding.
            for lead in [&b"use:"[..], b"transition:", b"in:", b"out:", b"animate:"] {
                if let Some(rest) = name.strip_prefix(lead) {
                    let len = rest.iter().position(|b| !ident_part(*b)).unwrap_or(rest.len());
                    if len > 0 {
                        let at = start + lead.len();
                        uses.push(Use { at, end: at + len, kind: RefKind::Call });
                    }
                }
            }
        }
        let mut j = i;
        while j < hi && src[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= hi || src[j] != b'=' {
            continue;
        }
        j += 1;
        while j < hi && src[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= hi {
            return hi;
        }
        let handler = match kind {
            Kind::Vue => name.starts_with(b"@") || name.starts_with(b"v-on:"),
            _ => name.starts_with(b"on"),
        };
        match src[j] {
            q @ (b'"' | b'\'') => {
                let end = memchr::memchr(q, &src[j + 1..hi]).map_or(hi, |e| j + 1 + e);
                match kind {
                    Kind::Vue => {
                        if name.starts_with(b"v-") || matches!(name[0], b':' | b'@' | b'#') {
                            // `v-for="item in items"` and `v-slot="{ item }"` declare names of
                            // their own, which no script name is meant by: their uses are few
                            // and the names of the script that follow are still read.
                            expression(src, j + 1, end, handler, uses);
                        }
                    }
                    // `class="item {active}"`: expressions inside a quoted value.
                    _ => markup_text(src, j + 1, end, uses),
                }
                i = (end + 1).min(hi);
            }
            b'{' => {
                let end = brace_end(src, j, hi);
                expression(src, j + 1, end.saturating_sub(1).max(j + 1), handler, uses);
                i = end;
            }
            _ => {
                while j < hi && !src[j].is_ascii_whitespace() && src[j] != b'>' {
                    j += 1;
                }
                i = j;
            }
        }
    }
    hi
}

/// The `{expression}` parts of a piece of text.
fn markup_text(src: &[u8], lo: usize, hi: usize, uses: &mut Vec<Use>) {
    let mut i = lo;
    while let Some(at) = memchr::memchr(b'{', &src[i..hi]) {
        let s = i + at;
        let end = brace_end(src, s, hi);
        expression(src, s + 1, end.saturating_sub(1).max(s + 1), false, uses);
        i = end;
    }
}

/// Parse the scripts of a component file and add what its markup uses of them.
pub fn extract(kind: Kind, src: &[u8], path: &str, parsers: &mut Parsers, facts: &mut FileFacts, scratch: &mut Scratch) -> bool {
    let parts = parts(kind, src);
    let lang = if parts.jsx { Lang::Tsx } else { Lang::TypeScript };
    let text = masked(src, &parts.scripts);
    let Some(parser) = parsers.get(lang) else { return false };
    let Some(tree) = parser.parse(&text, None) else { return false };
    js::extract(lang, &text, &tree, path, facts, scratch);

    let component = component_def(src, path, facts);

    // What the markup can mean by a bare name: a top-level declaration of the script or one of
    // its imports. In Vue's options API also a member of the component object (a method, a
    // computed value), which is then matched by name.
    let mut known: FxHashMap<Vec<u8>, bool> = FxHashMap::default();
    for (i, d) in facts.defs.iter().enumerate() {
        let top = d.parent == NONE;
        if Some(i as u32) != component && (top || kind == Kind::Vue) {
            let e = known.entry(facts.names.get(d.name).to_vec()).or_insert(top);
            *e |= top;
        }
    }
    for i in &facts.imports {
        let local = if i.alias != NONE { i.alias } else { i.name };
        if local != NONE && i.flags & iflag::REEXPORT == 0 {
            known.insert(facts.names.get(local).to_vec(), true);
        }
    }
    let mut skip: Vec<Span> = parts.scripts.iter().chain(parts.dead.iter()).copied().collect();
    skip.sort_unstable();
    let (mut uses, mut tags) = (Vec::new(), Vec::new());
    let mut at = 0;
    for &(s, e) in skip.iter().chain(std::iter::once(&(src.len(), src.len()))) {
        if s > at {
            markup(kind, src, at, s, &mut uses, &mut tags);
        }
        at = at.max(e);
    }
    let starts = crate::query::show::line_starts(src);
    let line_of = |off: usize| starts.partition_point(|&s| s <= off) as u32;
    let encl = component.unwrap_or(NONE);
    for u in uses {
        let name = &src[u.at..u.end];
        let Some(&top) = known.get(name) else { continue };
        let kind = match (top, u.kind) {
            (true, k) => k,
            (false, RefKind::Call) => RefKind::MemberCall,
            (false, _) => RefKind::MemberRead,
        };
        let name = facts.names.intern(name);
        facts.refs.push(Ref { name, qual: NONE, line: line_of(u.at), encl, kind, nargs: 255 });
    }
    for t in tags {
        // A component nobody imported is still one: frameworks register components by file name.
        if t.given || known.get(&t.name).copied() == Some(true) {
            let name = facts.names.intern(&t.name);
            facts.refs.push(Ref { name, qual: NONE, line: line_of(t.at), encl, kind: RefKind::New, nargs: 255 });
        }
    }
    true
}

/// The component a file is: a definition named after the file, which is what other files
/// import and what their markup uses as a tag. It is also what encloses the markup. A file
/// whose script exports a default of its own (a named class or function) has one already.
fn component_def(src: &[u8], path: &str, facts: &mut FileFacts) -> Option<u32> {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    // `index.vue` and `+page.svelte` are named by their place, not by a name of their own.
    if stem.is_empty() || !stem.bytes().all(ident_part) || !stem.as_bytes()[0].is_ascii_uppercase() {
        return None;
    }
    let name = facts.names.intern(stem.as_bytes());
    let default = facts.names.intern(b"default");
    let has_default =
        facts.defs.iter().any(|d| d.parent == NONE && d.name == default) || facts.imports.iter().any(|i| i.flags & iflag::REEXPORT != 0 && i.alias == default);
    if has_default || facts.defs.iter().any(|d| d.parent == NONE && d.name == name) {
        return None;
    }
    // The first line that says anything stands for the signature.
    let start = src.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
    let len = memchr::memchr(b'\n', &src[start..]).unwrap_or(src.len() - start).min(120);
    // It goes first, where the file starts: the others move up by one.
    for d in &mut facts.defs {
        if d.parent != NONE {
            d.parent += 1;
        }
    }
    for r in &mut facts.refs {
        if r.encl != NONE {
            r.encl += 1;
        }
    }
    facts.defs.insert(
        0,
        Def {
            name,
            parent: NONE,
            qual: NONE,
            line: 1,
            end_line: facts.nlines.max(1),
            sig_off: start as u32,
            sig_len: len as u16,
            kind: DefKind::Class,
            flags: dflag::EXPORTED,
            type_name: NONE,
            arity_min: 0,
            arity_max: 0,
        },
    );
    // Whatever name an importer gives it, it is this file's default export.
    let this = facts.names.intern(b"");
    facts.imports.push(Import { target: this, name, alias: default, line: 1, flags: iflag::REEXPORT });
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str, kind: Kind) -> Vec<(String, RefKind)> {
        let (mut uses, mut tags) = (Vec::new(), Vec::new());
        markup(kind, src.as_bytes(), 0, src.len(), &mut uses, &mut tags);
        let mut out: Vec<(String, RefKind)> = uses.iter().map(|u| (src[u.at..u.end].to_string(), u.kind)).collect();
        out.extend(tags.iter().map(|t| (String::from_utf8_lossy(&t.name).to_string(), RefKind::New)));
        out
    }

    fn has(v: &[(String, RefKind)], name: &str, kind: RefKind) -> bool {
        v.iter().any(|(n, k)| n == name && *k == kind)
    }

    #[test]
    fn the_scripts_of_a_vue_file_are_found_and_the_rest_is_blanked() {
        let src =
            "<template>\n  <p>{{ a }}</p>\n</template>\n<script setup lang=\"ts\">\nconst a = 1\n</script>\n<style scoped>\n.a { color: red }\n</style>\n";
        let p = parts(Kind::Vue, src.as_bytes());
        assert_eq!(p.scripts.len(), 1);
        assert_eq!(&src[p.scripts[0].0..p.scripts[0].1], "\nconst a = 1\n");
        assert_eq!(p.dead.len(), 1, "the style block");
        let m = masked(src.as_bytes(), &p.scripts);
        assert_eq!(m.len(), src.len());
        assert_eq!(String::from_utf8(m).unwrap().trim(), "const a = 1");
    }

    #[test]
    fn astro_front_matter_is_a_script() {
        let src = "---\nimport Card from './Card.astro'\nconst n = 1\n---\n<Card n={n} />\n<script>\n  go()\n</script>\n";
        let p = parts(Kind::Astro, src.as_bytes());
        assert_eq!(p.scripts.len(), 2);
        assert!(src[p.scripts[0].0..p.scripts[0].1].contains("const n = 1"));
        assert!(src[p.scripts[1].0..p.scripts[1].1].contains("go()"));
    }

    #[test]
    fn a_script_in_another_language_and_a_comment_are_left_alone() {
        let src = "<!-- <script>no()</script> -->\n<script type=\"application/ld+json\">{\"a\": 1}</script>\n<script lang=\"tsx\">yes()</script>\n";
        let p = parts(Kind::Vue, src.as_bytes());
        assert_eq!(p.scripts.len(), 1);
        assert!(p.jsx);
        assert_eq!(&src[p.scripts[0].0..p.scripts[0].1], "yes()");
    }

    #[test]
    fn vue_markup_uses_names_in_expressions_only() {
        let v = names(
            "<div class=\"save\" :title=\"label\" @click=\"save\" v-if=\"user.ready && check(item)\">save {{ format(total) }} <user-card :u=\"{ name: who }\" /></div>",
            Kind::Vue,
        );
        assert!(has(&v, "save", RefKind::Call), "{v:?}");
        assert_eq!(v.iter().filter(|(n, _)| n == "save").count(), 1, "the class and the text are not uses: {v:?}");
        assert!(has(&v, "label", RefKind::Read) && has(&v, "user", RefKind::Read) && has(&v, "check", RefKind::Call), "{v:?}");
        assert!(has(&v, "format", RefKind::Call) && has(&v, "total", RefKind::Read) && has(&v, "who", RefKind::Read), "{v:?}");
        assert!(!v.iter().any(|(n, _)| n == "ready" || n == "name"), "property names: {v:?}");
        assert!(has(&v, "UserCard", RefKind::New), "{v:?}");
    }

    #[test]
    fn svelte_markup_uses_names_in_braces_and_directives() {
        let v = names("<Button on:click={save} use:tooltip class=\"a {kind}\">{#each items as item}{label(item)}{/each}</Button>", Kind::Svelte);
        assert!(has(&v, "save", RefKind::Call) && has(&v, "tooltip", RefKind::Call), "{v:?}");
        assert!(has(&v, "kind", RefKind::Read) && has(&v, "items", RefKind::Read) && has(&v, "label", RefKind::Call), "{v:?}");
        assert!(has(&v, "Button", RefKind::New), "{v:?}");
    }
}
