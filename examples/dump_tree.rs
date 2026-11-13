// Debug helper: print the tree-sitter tree of a C file (optionally a line range) with field names.
use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let lo: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let hi: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let maxdepth: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(99);
    let src = fs::read(path).unwrap();
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_c::LANGUAGE.into()).unwrap();
    let tree = p.parse(&src, None).unwrap();
    let mut c = tree.walk();
    let mut depth = 0usize;
    'outer: loop {
        let n = c.node();
        let (sl, el) = (n.start_position().row + 1, n.end_position().row + 1);
        let visible = el >= lo && sl <= hi && depth <= maxdepth && (depth == 0 || sl >= lo || el <= hi);
        if visible && n.is_named() {
            let field = c.field_name().map(|f| format!("{f}: ")).unwrap_or_default();
            let text = if n.child_count() == 0 || (el == sl && n.end_byte() - n.start_byte() < 50) {
                format!("  `{}`", String::from_utf8_lossy(&src[n.start_byte()..n.end_byte()]).replace('\n', "\\n"))
            } else {
                String::new()
            };
            println!("{}{}{} [{}-{}]{}", "  ".repeat(depth), field, n.kind(), sl, el, text);
        }
        if el >= lo && sl <= hi && depth < maxdepth && c.goto_first_child() {
            depth += 1;
            continue;
        }
        loop {
            if c.goto_next_sibling() {
                break;
            }
            if !c.goto_parent() {
                break 'outer;
            }
            depth -= 1;
        }
    }
}
