/// Splits text into lines. Each line keeps its trailing `\n`, the last line
/// may lack one, and empty text has no lines.
pub fn split_lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_string).collect()
}

/// Joins lines back into text (the inverse of `split_lines`).
pub fn join_lines(lines: &[String]) -> String {
    lines.concat()
}

/// The line without its trailing `\n`, and whether it had one.
pub(crate) fn strip_newline(line: &str) -> (&str, bool) {
    match line.strip_suffix('\n') {
        Some(body) => (body, true),
        None => (line, false),
    }
}
