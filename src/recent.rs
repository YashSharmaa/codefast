//! What was answered a moment ago.
//!
//! A hook that speaks unasked must not repeat what a command has just printed. Commands and
//! hooks are separate processes that share nothing but the index directory, so a command leaves
//! a line here when its answer listed every use of a symbol, and a hook that is about to list
//! the same uses reads it and says so in one line instead.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FILE: &str = "recent";
/// The file is rewritten when it grows past this: only the last answers matter.
const MAX_BYTES: u64 = 16 << 10;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Record that the uses of `symbol` were just listed in full. Failing to write is not an error.
pub fn listed(index_dir: &Path, symbol: &str) {
    if symbol.is_empty() || symbol.contains(['\n', '\t']) || !index_dir.is_dir() {
        return;
    }
    let path = index_dir.join(FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(format!("{}\t{symbol}\n", now()).as_bytes());
    }
}

/// Whether the uses of `symbol` were listed in full within the last `within`.
pub fn was_listed(index_dir: &Path, symbol: &str, within: Duration) -> bool {
    let Ok(text) = std::fs::read_to_string(index_dir.join(FILE)) else { return false };
    let since = now().saturating_sub(within.as_secs());
    text.lines().rev().any(|l| l.split_once('\t').is_some_and(|(t, s)| s == symbol && t.parse::<u64>().is_ok_and(|t| t >= since)))
}

const HOOKED: &str = "hooked";

/// Record that a hook of the plugin has just run for this repository: the side that speaks
/// unasked is at work in a session here. Failing to write is not an error.
pub fn hooked(index_dir: &Path) {
    if index_dir.is_dir() {
        let _ = std::fs::write(index_dir.join(HOOKED), now().to_string());
    }
}

/// Whether a hook ran for this repository within the last `within`.
pub fn was_hooked(index_dir: &Path, within: Duration) -> bool {
    let at = std::fs::read_to_string(index_dir.join(HOOKED)).ok().and_then(|t| t.trim().parse::<u64>().ok());
    at.is_some_and(|t| now().saturating_sub(t) <= within.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hook_that_ran_is_known_for_a_while_and_not_for_ever() {
        let dir = std::env::temp_dir().join(format!("codefast-hooked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!was_hooked(&dir, Duration::from_secs(600)));
        hooked(&dir);
        assert!(was_hooked(&dir, Duration::from_secs(600)));
        // An old mark does not count.
        std::fs::write(dir.join(HOOKED), (now() - 3600).to_string()).unwrap();
        assert!(!was_hooked(&dir, Duration::from_secs(600)));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
