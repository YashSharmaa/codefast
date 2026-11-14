use std::collections::{BTreeMap, BTreeSet};

use crate::constraint::Constraint;
use crate::error::Error;
use crate::registry::Registry;
use crate::version::Version;

type Assignment = BTreeMap<String, Option<Version>>;

struct Search<'a> {
    registry: &'a Registry,
    root: &'a [(String, Constraint)],
    names: Vec<String>,
    assign: Assignment,
}

fn satisfied(assign: &Assignment, name: &str, c: &Constraint) -> Option<bool> {
    assign.get(name).map(|choice| choice.as_ref().is_some_and(|v| c.matches(v)))
}

impl Search<'_> {
    /// False when some requirement between already decided packages fails.
    fn consistent(&self) -> bool {
        for (name, c) in self.root {
            if satisfied(&self.assign, name, c) == Some(false) {
                return false;
            }
        }
        for (pkg, choice) in &self.assign {
            let Some(v) = choice else { continue };
            for (dep, c) in self.registry.dependencies(pkg, v).unwrap_or(&[]) {
                if !self.registry.contains(dep) || satisfied(&self.assign, dep, c) == Some(false) {
                    return false;
                }
            }
        }
        true
    }

    /// True when every present package is reachable from the root requirements.
    fn all_reachable(&self) -> bool {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut stack: Vec<&str> = self.root.iter().map(|(n, _)| n.as_str()).collect();
        while let Some(name) = stack.pop() {
            if !seen.insert(name) {
                continue;
            }
            if let Some(Some(v)) = self.assign.get(name) {
                for (dep, _) in self.registry.dependencies(name, v).unwrap_or(&[]) {
                    stack.push(dep);
                }
            }
        }
        self.assign
            .iter()
            .all(|(n, choice)| choice.is_none() || seen.contains(n.as_str()))
    }

    fn search(&mut self, index: usize) -> bool {
        if index == self.names.len() {
            return self.all_reachable();
        }
        let name = self.names[index].clone();
        let mut options: Vec<Option<Version>> =
            self.registry.versions(&name).into_iter().rev().map(Some).collect();
        options.push(None);
        for option in options {
            self.assign.insert(name.clone(), option);
            if self.consistent() && self.search(index + 1) {
                return true;
            }
        }
        self.assign.remove(&name);
        false
    }
}

/// Finds the greatest solution for the root requirements (see SPEC.md).
pub fn resolve(
    registry: &Registry,
    root: &[(String, Constraint)],
) -> Result<BTreeMap<String, Version>, Error> {
    if let Some((name, _)) = root.iter().find(|(n, _)| !registry.contains(n)) {
        return Err(Error::UnknownPackage(name.clone()));
    }
    let mut universe: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = root.iter().map(|(n, _)| n.clone()).collect();
    while let Some(name) = stack.pop() {
        if !registry.contains(&name) || !universe.insert(name.clone()) {
            continue;
        }
        for v in registry.versions(&name) {
            for (dep, _) in registry.dependencies(&name, &v).unwrap_or(&[]) {
                stack.push(dep.clone());
            }
        }
    }
    let mut search = Search {
        registry,
        root,
        names: universe.into_iter().collect(),
        assign: BTreeMap::new(),
    };
    if !search.search(0) {
        return Err(Error::NoSolution);
    }
    Ok(search
        .assign
        .into_iter()
        .filter_map(|(n, v)| v.map(|v| (n, v)))
        .collect())
}
