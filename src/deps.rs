//! Locating the source of a project's dependencies, so that their APIs can be looked up the
//! same way as the project's own code. Pure file system work: no package manager is run, and
//! nothing is ever written into a package directory.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dep {
    /// `python`, `node`, `go` or `rust`.
    pub eco: &'static str,
    pub name: String,
    pub version: String,
    /// Directory (or single file) holding the package's source.
    pub dir: PathBuf,
    /// Named by the project's own manifest, as opposed to pulled in by another dependency.
    pub direct: bool,
}

fn read(path: &Path) -> Option<String> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() || md.len() > 16 * 1024 * 1024 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// `Foo-Bar` and `foo_bar` are the same package name.
fn norm(name: &str) -> String {
    name.to_ascii_lowercase().replace(['-', '.'], "_")
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

/// Every dependency whose source can be found, direct ones first, then by name.
pub fn discover(root: &Path) -> Vec<Dep> {
    let mut out: Vec<Dep> = Vec::new();
    python(root, &mut out);
    node(root, &mut out);
    go(root, &mut out);
    rust(root, &mut out);
    out.sort_by(|a, b| b.direct.cmp(&a.direct).then_with(|| a.eco.cmp(b.eco)).then_with(|| a.name.cmp(&b.name)));
    out.dedup_by(|a, b| a.eco == b.eco && a.name == b.name && a.version == b.version);
    out
}

/// Dependencies matching `name`: the exact name first, then a path's last element, then a
/// suffix (`cobra` finds `github.com/spf13/cobra`).
pub fn lookup<'a>(deps: &'a [Dep], name: &str) -> Vec<&'a Dep> {
    let want = norm(name);
    let exact: Vec<&Dep> = deps.iter().filter(|d| norm(&d.name) == want).collect();
    if !exact.is_empty() {
        return exact;
    }
    let last: Vec<&Dep> = deps.iter().filter(|d| norm(d.name.rsplit('/').next().unwrap_or(&d.name)) == want).collect();
    if !last.is_empty() {
        return last;
    }
    deps.iter().filter(|d| norm(&d.name).ends_with(&format!("/{want}"))).collect()
}

/// Names close to `name`, for a "did you mean" line.
pub fn similar<'a>(deps: &'a [Dep], name: &str) -> Vec<&'a str> {
    let want = norm(name);
    let mut out: Vec<&str> = deps.iter().filter(|d| norm(&d.name).contains(&want) || want.contains(&norm(&d.name))).map(|d| d.name.as_str()).collect();
    out.dedup();
    out.truncate(8);
    out
}

/// A directory name for the dependency's index, safe on every file system.
pub fn index_key(d: &Dep) -> String {
    let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '_' }).collect::<String>();
    format!("{}-{}-{}", d.eco, clean(&d.name), clean(&d.version))
}

// ---------------------------------------------------------------------------------------------
// Python: virtual environments inside or around the project
// ---------------------------------------------------------------------------------------------

fn site_packages(root: &Path) -> Vec<PathBuf> {
    let mut envs: Vec<PathBuf> = Vec::new();
    if let Some(v) = std::env::var_os("VIRTUAL_ENV") {
        envs.push(PathBuf::from(v));
    }
    for n in [".venv", "venv", "env", ".env"] {
        envs.push(root.join(n));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for e in envs {
        if !e.join("pyvenv.cfg").is_file() {
            continue;
        }
        if let Ok(rd) = std::fs::read_dir(e.join("lib")) {
            for ent in rd.flatten() {
                let sp = ent.path().join("site-packages");
                if ent.file_name().to_string_lossy().starts_with("python") && sp.is_dir() && !out.contains(&sp) {
                    out.push(sp);
                }
            }
        }
        let win = e.join("Lib").join("site-packages");
        if win.is_dir() && !out.contains(&win) {
            out.push(win);
        }
    }
    out
}

fn python(root: &Path, out: &mut Vec<Dep>) {
    let sps = site_packages(root);
    if sps.is_empty() {
        return;
    }
    // What the project itself asks for.
    let mut wanted = String::new();
    for f in ["pyproject.toml", "requirements.txt", "requirements-dev.txt", "setup.cfg", "setup.py", "Pipfile"] {
        if let Some(t) = read(&root.join(f)) {
            wanted.push_str(&t.to_ascii_lowercase());
            wanted.push('\n');
        }
    }
    let wanted = wanted.replace(['-', '.'], "_");
    for sp in sps {
        let Ok(rd) = std::fs::read_dir(&sp) else { continue };
        for ent in rd.flatten() {
            let fname = ent.file_name().to_string_lossy().into_owned();
            let Some(stem) = fname.strip_suffix(".dist-info") else { continue };
            let Some((name, version)) = stem.rsplit_once('-') else { continue };
            // The importable package: named in top_level.txt, else after the distribution.
            let mut tops: Vec<String> = read(&ent.path().join("top_level.txt"))
                .map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
                .unwrap_or_default();
            if tops.is_empty() {
                tops.push(name.replace('-', "_"));
            }
            for top in tops {
                let dir = sp.join(&top);
                let file = sp.join(format!("{top}.py"));
                let path = if dir.is_dir() {
                    dir
                } else if file.is_file() {
                    file
                } else {
                    continue;
                };
                let key = norm(name);
                let direct = wanted.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).any(|w| w == key);
                out.push(Dep { eco: "python", name: name.to_string(), version: version.to_string(), dir: path, direct });
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Node: node_modules of the project or of a parent directory (hoisted workspaces)
// ---------------------------------------------------------------------------------------------

fn node(root: &Path, out: &mut Vec<Dep>) {
    let Some(pkg) = read(&root.join("package.json")).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return };
    let mut names: Vec<String> = Vec::new();
    for sect in ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"] {
        if let Some(m) = pkg.get(sect).and_then(|v| v.as_object()) {
            for k in m.keys() {
                if !names.contains(k) {
                    names.push(k.clone());
                }
            }
        }
    }
    for name in names {
        if let Some(d) = node_package(root, &name) {
            out.push(Dep { direct: true, ..d });
        }
    }
}

/// `node_modules/NAME` in `root` or one of its parents.
pub fn node_package(root: &Path, name: &str) -> Option<Dep> {
    if name.is_empty() || name.contains("..") || name.starts_with('/') {
        return None;
    }
    let mut dir = Some(root);
    for _ in 0..6 {
        let d = dir?;
        let p = d.join("node_modules").join(name);
        if p.join("package.json").is_file() {
            let version = read(&p.join("package.json"))
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v.get("version").and_then(|x| x.as_str()).map(|s| s.to_string()))
                .unwrap_or_default();
            return Some(Dep { eco: "node", name: name.to_string(), version, dir: p, direct: false });
        }
        dir = d.parent();
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Go: vendor directory, else the module cache
// ---------------------------------------------------------------------------------------------

fn go_mod_cache() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GOMODCACHE").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(p) = std::env::var_os("GOPATH").filter(|p| !p.is_empty()) {
        let first = std::env::split_paths(&p).next()?;
        return Some(first.join("pkg").join("mod"));
    }
    Some(home()?.join("go").join("pkg").join("mod"))
}

/// The module cache spells an upper-case letter as `!` plus the lower-case letter.
fn go_escape(path: &str) -> String {
    let mut s = String::with_capacity(path.len() + 4);
    for c in path.chars() {
        if c.is_ascii_uppercase() {
            s.push('!');
            s.push(c.to_ascii_lowercase());
        } else {
            s.push(c);
        }
    }
    s
}

fn go(root: &Path, out: &mut Vec<Dep>) {
    let Some(text) = read(&root.join("go.mod")) else { return };
    let cache = go_mod_cache();
    let mut in_block = false;
    for line in text.lines() {
        let l = line.trim();
        let spec = if in_block {
            if l.starts_with(')') {
                in_block = false;
                continue;
            }
            l
        } else if l.starts_with("require (") || l == "require(" {
            in_block = true;
            continue;
        } else if let Some(rest) = l.strip_prefix("require ") {
            rest.trim()
        } else {
            continue;
        };
        let direct = !spec.contains("// indirect");
        let mut parts = spec.split("//").next().unwrap_or("").split_whitespace();
        let (Some(path), Some(version)) = (parts.next(), parts.next()) else { continue };
        let vendored = root.join("vendor").join(path);
        let dir = if vendored.is_dir() {
            vendored
        } else {
            let Some(c) = &cache else { continue };
            let p = c.join(format!("{}@{}", go_escape(path), go_escape(version)));
            if !p.is_dir() {
                continue;
            }
            p
        };
        out.push(Dep { eco: "go", name: path.to_string(), version: version.to_string(), dir, direct });
    }
}

// ---------------------------------------------------------------------------------------------
// Rust: the cargo registry sources named by Cargo.lock
// ---------------------------------------------------------------------------------------------

fn cargo_registry_src() -> Vec<PathBuf> {
    let base = std::env::var_os("CARGO_HOME").filter(|p| !p.is_empty()).map(PathBuf::from).or_else(|| home().map(|h| h.join(".cargo")));
    let Some(base) = base else { return Vec::new() };
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(base.join("registry").join("src")) {
        for ent in rd.flatten() {
            if ent.path().is_dir() {
                out.push(ent.path());
            }
        }
    }
    out
}

fn rust(root: &Path, out: &mut Vec<Dep>) {
    let Some(lock) = read(&root.join("Cargo.lock")) else { return };
    let srcs = cargo_registry_src();
    if srcs.is_empty() {
        return;
    }
    // Names the manifest lists under a dependencies table.
    let mut direct: Vec<String> = Vec::new();
    if let Some(toml) = read(&root.join("Cargo.toml")) {
        let mut in_deps = false;
        for line in toml.lines() {
            let l = line.trim();
            if l.starts_with('[') {
                in_deps = l.contains("dependencies");
                // `[dependencies.foo]` names the dependency in the header
                if let Some(name) = l.trim_matches(['[', ']']).rsplit_once("dependencies.").map(|x| x.1) {
                    direct.push(norm(name));
                }
                continue;
            }
            if in_deps {
                if let Some((k, _)) = l.split_once('=') {
                    let k = k.trim().trim_matches('"');
                    if !k.is_empty() && !k.starts_with('#') {
                        direct.push(norm(k.split('.').next().unwrap_or(k)));
                    }
                }
            }
        }
    }
    let (mut name, mut version, mut registry) = (String::new(), String::new(), false);
    let flush = |name: &mut String, version: &mut String, registry: &mut bool, out: &mut Vec<Dep>| {
        if *registry && !name.is_empty() && !version.is_empty() {
            for s in &srcs {
                let dir = s.join(format!("{name}-{version}"));
                if dir.is_dir() {
                    out.push(Dep { eco: "rust", name: name.clone(), version: version.clone(), dir, direct: direct.contains(&norm(name)) });
                    break;
                }
            }
        }
        name.clear();
        version.clear();
        *registry = false;
    };
    for line in lock.lines() {
        let l = line.trim();
        if l == "[[package]]" {
            flush(&mut name, &mut version, &mut registry, out);
        } else if let Some(v) = l.strip_prefix("name = ") {
            name = v.trim_matches('"').to_string();
        } else if let Some(v) = l.strip_prefix("version = ") {
            version = v.trim_matches('"').to_string();
        } else if l.starts_with("source = \"registry+") {
            registry = true;
        }
    }
    flush(&mut name, &mut version, &mut registry, out);
}

/// Files of a package that state its public surface, relative to the package directory.
pub fn entry_files(d: &Dep) -> Vec<String> {
    let has = |rel: &str| d.dir.join(rel).is_file();
    let mut out: Vec<String> = Vec::new();
    match d.eco {
        "python" => {
            if has("__init__.py") {
                out.push("__init__.py".into());
            }
            if has("__init__.pyi") {
                out.push("__init__.pyi".into());
            }
        }
        "node" => {
            if let Some(v) = read(&d.dir.join("package.json")).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
                for key in ["types", "typings", "module", "main"] {
                    if let Some(p) = v.get(key).and_then(|x| x.as_str()) {
                        let rel = p.trim_start_matches("./").to_string();
                        if has(&rel) && !out.contains(&rel) {
                            out.push(rel);
                            break;
                        }
                    }
                }
            }
            for cand in ["index.d.ts", "index.ts", "index.js"] {
                if out.is_empty() && has(cand) {
                    out.push(cand.into());
                }
            }
        }
        "rust" => {
            if has("src/lib.rs") {
                out.push("src/lib.rs".into());
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codefast-deps-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn node_direct_dependencies() {
        let r = tmp("node");
        write(&r.join("package.json"), r#"{"dependencies":{"left-pad":"^1.0.0","@scope/pkg":"2"},"devDependencies":{"missing":"1"}}"#);
        write(&r.join("node_modules/left-pad/package.json"), r#"{"name":"left-pad","version":"1.3.0","main":"index.js"}"#);
        write(&r.join("node_modules/left-pad/index.js"), "module.exports = function leftPad() {}\n");
        write(&r.join("node_modules/@scope/pkg/package.json"), r#"{"version":"2.0.1","types":"./dist/index.d.ts"}"#);
        write(&r.join("node_modules/@scope/pkg/dist/index.d.ts"), "export declare function f(): void;\n");
        let deps = discover(&r);
        assert_eq!(deps.iter().map(|d| (d.name.as_str(), d.version.as_str())).collect::<Vec<_>>(), vec![("@scope/pkg", "2.0.1"), ("left-pad", "1.3.0")]);
        assert!(deps.iter().all(|d| d.direct && d.eco == "node"));
        assert_eq!(lookup(&deps, "left_pad").len(), 1);
        assert_eq!(lookup(&deps, "pkg").len(), 1);
        assert_eq!(entry_files(lookup(&deps, "@scope/pkg")[0]), vec!["dist/index.d.ts".to_string()]);
        assert_eq!(entry_files(lookup(&deps, "left-pad")[0]), vec!["index.js".to_string()]);
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn python_virtualenv() {
        let r = tmp("py");
        write(&r.join("requirements.txt"), "Requests>=2\n");
        write(&r.join(".venv/pyvenv.cfg"), "home = /usr/bin\n");
        let sp = r.join(".venv/lib/python3.12/site-packages");
        write(&sp.join("requests/__init__.py"), "def get(url): ...\n");
        write(&sp.join("requests-2.32.3.dist-info/METADATA"), "Name: requests\n");
        write(&sp.join("six.py"), "PY3 = True\n");
        write(&sp.join("six-1.16.0.dist-info/top_level.txt"), "six\n");
        write(&sp.join("PyYAML-6.0.dist-info/top_level.txt"), "_yaml\nyaml\n");
        write(&sp.join("yaml/__init__.py"), "def load(s): ...\n");
        let deps = discover(&r);
        let names: Vec<(&str, &str, bool)> = deps.iter().map(|d| (d.name.as_str(), d.version.as_str(), d.direct)).collect();
        assert_eq!(names, vec![("requests", "2.32.3", true), ("PyYAML", "6.0", false), ("six", "1.16.0", false)]);
        assert!(lookup(&deps, "pyyaml")[0].dir.ends_with("yaml"));
        assert!(lookup(&deps, "six")[0].dir.ends_with("six.py"));
        assert_eq!(entry_files(lookup(&deps, "requests")[0]), vec!["__init__.py".to_string()]);
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn go_vendor_and_names() {
        let r = tmp("go");
        write(
            &r.join("go.mod"),
            "module example.com/app\n\ngo 1.22\n\nrequire (\n\tgithub.com/spf13/cobra v1.9.1\n\tgithub.com/Foo/Bar v0.1.0 // indirect\n)\n\nrequire example.invalid/x/sys v0.0.1\n",
        );
        write(&r.join("vendor/github.com/spf13/cobra/command.go"), "package cobra\n");
        write(&r.join("vendor/github.com/Foo/Bar/x.go"), "package bar\n");
        let deps = discover(&r);
        let names: Vec<(&str, bool)> = deps.iter().map(|d| (d.name.as_str(), d.direct)).collect();
        assert_eq!(names, vec![("github.com/spf13/cobra", true), ("github.com/Foo/Bar", false)]);
        assert_eq!(lookup(&deps, "cobra").len(), 1);
        assert_eq!(go_escape("github.com/Foo/Bar"), "github.com/!foo/!bar");
        assert_eq!(index_key(&deps[0]), "go-github.com_spf13_cobra-v1.9.1");
        let _ = std::fs::remove_dir_all(&r);
    }
}
