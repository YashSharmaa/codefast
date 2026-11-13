//! Output assembly under a token budget.
//!
//! Every answer is plain text meant to be read by a model: dense, grouped by file, with
//! `path:line` handles that feed straight into a harness's read / edit tools. Nothing is
//! silently cut: when the budget runs out the answer says how much was left out and how to
//! narrow the question.

/// Rough token estimate for code-like text. Measured on real answers, `o200k_base` gives about
/// 3.1 bytes per token and Claude's tokenizers noticeably fewer bytes per token; 2.8 keeps
/// budgets honest for both.
pub fn est_tokens(s: &str) -> usize {
    (s.len() as f64 / 2.8).ceil() as usize
}

pub struct Out {
    buf: String,
    budget: usize,
    used: usize,
    /// Set once a line was refused; later lines are counted, not printed.
    pub full: bool,
    pub dropped: usize,
}

impl Out {
    /// `budget` is in tokens; 0 means unlimited.
    pub fn new(budget: usize) -> Out {
        Out { buf: String::with_capacity(4096), budget, used: 0, full: false, dropped: 0 }
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn remaining(&self) -> usize {
        if self.budget == 0 { usize::MAX } else { self.budget.saturating_sub(self.used) }
    }

    /// Append a line unconditionally (headers, footers).
    pub fn push(&mut self, line: &str) {
        self.used += est_tokens(line) + 1;
        self.buf.push_str(line);
        self.buf.push('\n');
    }

    /// Append a line if it fits. Returns false, and counts the line as dropped, when it does not.
    pub fn line(&mut self, line: &str) -> bool {
        let cost = est_tokens(line) + 1;
        if self.full || (self.budget != 0 && self.used + cost > self.budget) {
            self.full = true;
            self.dropped += 1;
            return false;
        }
        self.used += cost;
        self.buf.push_str(line);
        self.buf.push('\n');
        true
    }

    /// Whether `tokens` more would still fit.
    pub fn fits(&self, tokens: usize) -> bool {
        !self.full && (self.budget == 0 || self.used + tokens <= self.budget)
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn finish(self) -> String {
        self.buf
    }
}

/// Thousands separators make large counts readable at no token cost.
pub fn count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Truncate to at most `max` characters, marking the cut.
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
