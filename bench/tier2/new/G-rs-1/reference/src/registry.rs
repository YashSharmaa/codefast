use std::collections::BTreeMap;

use crate::constraint::Constraint;
use crate::error::Error;
use crate::version::Version;

/// The dependencies of one package version, in the order they were written.
pub type Dependencies = Vec<(String, Constraint)>;

/// All known packages, their versions and the dependencies of each version.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    packages: BTreeMap<String, BTreeMap<Version, Dependencies>>,
}

/// A package name: one or more ASCII letters, digits, `-` or `_`.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Registry {
    pub fn parse(text: &str) -> Result<Registry, Error> {
        let mut reg = Registry::default();
        let mut current: Option<(String, Version)> = None;
        for (i, raw) in text.lines().enumerate() {
            let line_no = i + 1;
            let fail = |message: &str| Error::Parse { line: line_no, message: message.to_string() };
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(inner) = line.strip_prefix('[') {
                let inner = inner.strip_suffix(']').ok_or_else(|| fail("missing ]"))?;
                let words: Vec<&str> = inner.split_whitespace().collect();
                if words.len() != 2 || !valid_name(words[0]) {
                    return Err(fail("expected [name version]"));
                }
                let version = Version::parse(words[1]).map_err(|e| fail(&e.to_string()))?;
                let versions = reg.packages.entry(words[0].to_string()).or_default();
                if versions.contains_key(&version) {
                    return Err(fail("duplicate package version"));
                }
                versions.insert(version.clone(), Vec::new());
                current = Some((words[0].to_string(), version));
                continue;
            }
            let (name, version) = current.as_ref().ok_or_else(|| fail("dependency outside a section"))?;
            let (dep, cons) = line.split_once('=').ok_or_else(|| fail("expected dep = constraint"))?;
            let dep = dep.trim();
            if !valid_name(dep) {
                return Err(fail("invalid package name"));
            }
            let cons = Constraint::parse(cons.trim()).map_err(|e| fail(&e.to_string()))?;
            let deps = reg.packages.get_mut(name).and_then(|m| m.get_mut(version)).expect("section");
            if deps.iter().any(|(d, _)| d == dep) {
                return Err(fail("duplicate dependency"));
            }
            deps.push((dep.to_string(), cons));
        }
        Ok(reg)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }

    pub fn packages(&self) -> Vec<String> {
        self.packages.keys().cloned().collect()
    }

    pub fn versions(&self, name: &str) -> Vec<Version> {
        self.packages
            .get(name)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn dependencies(&self, name: &str, version: &Version) -> Option<&[(String, Constraint)]> {
        self.packages.get(name)?.get(version).map(|d| d.as_slice())
    }
}
