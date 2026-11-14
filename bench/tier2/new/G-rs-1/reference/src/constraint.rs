use std::str::FromStr;

use crate::error::Error;
use crate::version::{parse_parts, Version};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Op {
    fn holds(self, v: &Version, bound: &Version) -> bool {
        match self {
            Op::Eq => v == bound,
            Op::Gt => v > bound,
            Op::Ge => v >= bound,
            Op::Lt => v < bound,
            Op::Le => v <= bound,
        }
    }
}

/// One AND group: every bound must hold, and a pre-release version is only
/// accepted when its MAJOR.MINOR.PATCH appears in `pre_ok`.
#[derive(Debug, Clone, Default)]
struct Group {
    bounds: Vec<(Op, Version)>,
    pre_ok: Vec<(u64, u64, u64)>,
}

/// A version constraint: an OR of AND groups of comparators.
#[derive(Debug, Clone)]
pub struct Constraint {
    groups: Vec<Group>,
}

fn upper(major: u64, minor: u64, patch: u64) -> Option<Version> {
    Some(Version::new(major, minor, patch))
}

/// Adds one comparator to `group`. Returns None when the comparator is invalid.
fn add_comparator(group: &mut Group, text: &str) -> Option<()> {
    if text == "*" {
        return Some(());
    }
    let ops: [(&str, char); 7] = [
        (">=", 'G'),
        ("<=", 'L'),
        (">", '>'),
        ("<", '<'),
        ("=", '='),
        ("^", '^'),
        ("~", '~'),
    ];
    let (kind, rest) = ops
        .iter()
        .find_map(|(p, k)| text.strip_prefix(p).map(|r| (*k, r.trim_start())))
        .unwrap_or(('=', text));
    let partial = kind == '^' || kind == '~';
    let (parts, pre) = parse_parts(rest, partial)?;
    let get = |i: usize| parts.get(i).copied().unwrap_or(0);
    let (ma, mi, pa) = (get(0), get(1), get(2));
    let lower = Version { major: ma, minor: mi, patch: pa, pre: pre.clone() };
    if !pre.is_empty() {
        group.pre_ok.push((ma, mi, pa));
    }
    let op = match kind {
        'G' => Op::Ge,
        'L' => Op::Le,
        '>' => Op::Gt,
        '<' => Op::Lt,
        '=' => Op::Eq,
        _ => {
            let up = if kind == '^' {
                if parts.len() == 1 || ma > 0 {
                    ma.checked_add(1).and_then(|m| upper(m, 0, 0))
                } else if parts.len() == 2 || mi > 0 {
                    mi.checked_add(1).and_then(|m| upper(0, m, 0))
                } else {
                    pa.checked_add(1).and_then(|p| upper(0, 0, p))
                }
            } else if parts.len() == 1 {
                ma.checked_add(1).and_then(|m| upper(m, 0, 0))
            } else {
                mi.checked_add(1).and_then(|m| upper(ma, m, 0))
            };
            group.bounds.push((Op::Ge, lower));
            if let Some(up) = up {
                group.bounds.push((Op::Lt, up));
            }
            return Some(());
        }
    };
    group.bounds.push((op, lower));
    Some(())
}

impl Constraint {
    pub fn parse(s: &str) -> Result<Constraint, Error> {
        let err = || Error::InvalidConstraint(s.to_string());
        let mut groups = Vec::new();
        for group_text in s.split("||") {
            let mut group = Group::default();
            for comp in group_text.split(',') {
                let comp = comp.trim();
                if comp.is_empty() {
                    return Err(err());
                }
                add_comparator(&mut group, comp).ok_or_else(err)?;
            }
            groups.push(group);
        }
        Ok(Constraint { groups })
    }

    pub fn matches(&self, v: &Version) -> bool {
        self.groups.iter().any(|g| {
            g.bounds.iter().all(|(op, b)| op.holds(v, b))
                && (v.pre.is_empty() || g.pre_ok.contains(&v.triple()))
        })
    }

    pub fn max_satisfying<'a>(&self, versions: &'a [Version]) -> Option<&'a Version> {
        versions.iter().filter(|v| self.matches(v)).max()
    }
}

impl FromStr for Constraint {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Constraint::parse(s)
    }
}
