use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use crate::error::Error;

/// A semantic version: MAJOR.MINOR.PATCH with optional pre-release identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<String>,
}

/// Parses a decimal number with no leading zeros that fits in u64.
pub(crate) fn parse_number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if s.len() > 1 && s.starts_with('0') {
        return None;
    }
    s.parse().ok()
}

fn valid_identifier(s: &str) -> bool {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return false;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return parse_number(s).is_some();
    }
    true
}

/// Parses `N`, `N.N` or `N.N.N`, optionally followed by `-PRE` (only allowed with
/// three parts when `allow_partial` is set, and always allowed with three parts).
pub(crate) fn parse_parts(s: &str, allow_partial: bool) -> Option<(Vec<u64>, Vec<String>)> {
    let (core, pre) = match s.find('-') {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let parts: Vec<u64> = core
        .split('.')
        .map(parse_number)
        .collect::<Option<Vec<u64>>>()?;
    let ok_len = if allow_partial { (1..=3).contains(&parts.len()) } else { parts.len() == 3 };
    if !ok_len {
        return None;
    }
    let mut ids = Vec::new();
    if let Some(pre) = pre {
        if parts.len() != 3 {
            return None;
        }
        for id in pre.split('.') {
            if !valid_identifier(id) {
                return None;
            }
            ids.push(id.to_string());
        }
    }
    Some((parts, ids))
}

impl Version {
    pub fn new(major: u64, minor: u64, patch: u64) -> Version {
        Version { major, minor, patch, pre: Vec::new() }
    }

    pub fn parse(s: &str) -> Result<Version, Error> {
        let (parts, pre) =
            parse_parts(s, false).ok_or_else(|| Error::InvalidVersion(s.to_string()))?;
        Ok(Version { major: parts[0], minor: parts[1], patch: parts[2], pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    pub(crate) fn triple(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

fn compare_identifier(a: &str, b: &str) -> Ordering {
    let na = a.bytes().all(|c| c.is_ascii_digit());
    let nb = b.bytes().all(|c| c.is_ascii_digit());
    match (na, nb) {
        (true, true) => {
            let x: u64 = a.parse().unwrap_or(u64::MAX);
            let y: u64 = b.parse().unwrap_or(u64::MAX);
            x.cmp(&y)
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.as_bytes().cmp(b.as_bytes()),
    }
}

fn compare_pre(a: &[String], b: &[String]) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    for (x, y) in a.iter().zip(b.iter()) {
        let o = compare_identifier(x, y);
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.triple()
            .cmp(&other.triple())
            .then_with(|| compare_pre(&self.pre, &other.pre))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

impl FromStr for Version {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Version::parse(s)
    }
}
