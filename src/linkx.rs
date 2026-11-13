//! Module and package matching: does a definition's file belong to the module an import names?
//!
//! Pure path arithmetic per language, with no knowledge of build systems. An import target is
//! matched against file paths by suffix, which is how module roots (`src/`, `lib/`, a Go module
//! prefix, a Java source root) drop out without being configured.

use crate::lang::Lang;

/// How well a file fits an import target.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Fit {
    No = 0,
    /// The file lives inside the named package (re-exports, sibling classes of a package).
    Inside = 1,
    /// The file is the named module.
    Exact = 2,
}

fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

fn strip_ext(path: &str) -> &str {
    let file_start = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    match path[file_start..].rfind('.') {
        Some(i) if i > 0 => &path[..file_start + i],
        _ => path,
    }
}

/// Is `suffix` equal to `path` or a whole-component suffix of it?
fn ends_with_components(path: &str, suffix: &str) -> bool {
    if suffix.is_empty() {
        return false;
    }
    path == suffix || (path.len() > suffix.len() && path.ends_with(suffix) && path.as_bytes()[path.len() - suffix.len() - 1] == b'/')
}

/// Join a directory and a relative path, resolving `.` and `..`.
pub fn norm_join(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for p in rel.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            x => parts.push(x),
        }
    }
    parts.join("/")
}

/// The module path a file answers to: its path without extension and without the index-file
/// name of its language (`__init__`, `index`, `mod`).
pub fn module_stem(lang: Lang, path: &str) -> &str {
    let mut s = strip_ext(path);
    if lang == Lang::TypeScript || lang == Lang::Tsx {
        s = s.strip_suffix(".d").unwrap_or(s);
    }
    let index = match lang {
        Lang::Python => "/__init__",
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => "/index",
        Lang::Rust => "/mod",
        _ => "",
    };
    if !index.is_empty() {
        s = s.strip_suffix(index).unwrap_or(s);
    }
    s
}

fn strip_js_ext(t: &str) -> &str {
    for ext in [".js", ".ts", ".tsx", ".jsx", ".mjs", ".cjs", ".mts", ".cts"] {
        if let Some(s) = t.strip_suffix(ext) {
            return s;
        }
    }
    t
}

/// The `src` directory of the crate a Rust file belongs to: `compiler/a/src/x/y.rs` -> `compiler/a/src`.
fn crate_src_root(path: &str) -> Option<&str> {
    if let Some(i) = path.rfind("/src/") {
        return Some(&path[..i + 4]);
    }
    if path.starts_with("src/") { Some("src") } else { None }
}

/// Files that a Rust path made only of `crate`, `self` and `super` can name, seen from
/// `from_path`: the crate root, the module of the file itself, or an enclosing module.
pub fn rust_anchor_files(from_path: &str, target: &str) -> Vec<String> {
    let mut out = Vec::new();
    let segs: Vec<&str> = target.split("::").collect();
    if segs.is_empty() || !segs.iter().all(|s| matches!(*s, "crate" | "self" | "super")) {
        return out;
    }
    let module = |dir: &str, out: &mut Vec<String>| {
        // A module directory is described by `dir.rs`, `dir/mod.rs`, or for a crate root by
        // `lib.rs` / `main.rs` inside it.
        out.push(format!("{dir}.rs"));
        out.push(format!("{dir}/mod.rs"));
        out.push(format!("{dir}/lib.rs"));
        out.push(format!("{dir}/main.rs"));
    };
    if segs[0] == "crate" {
        if segs.len() == 1 {
            if let Some(root) = crate_src_root(from_path) {
                out.push(format!("{root}/lib.rs"));
                out.push(format!("{root}/main.rs"));
            }
        }
        return out;
    }
    // The module directory of the file: `a/b.rs` -> `a/b`, `a/b/mod.rs` -> `a/b`.
    let stem = module_stem(Lang::Rust, from_path);
    let mut dir = if from_path.ends_with("/lib.rs") || from_path.ends_with("/main.rs") { parent_dir(from_path).to_string() } else { stem.to_string() };
    for s in &segs {
        if *s == "super" {
            dir = parent_dir(&dir).to_string();
        }
    }
    if segs.iter().all(|s| *s == "self") {
        out.push(from_path.to_string());
    } else {
        module(&dir, &mut out);
    }
    out
}

/// Does the file at `def_path` belong to the module `target`, as imported from `from_path`?
pub fn module_match(lang: Lang, from_path: &str, target: &str, def_path: &str) -> Fit {
    if target.is_empty() {
        return Fit::No;
    }
    match lang {
        Lang::Go => {
            // An import path names a directory: the repository-relative directory is a suffix of
            // the import path (module prefix dropped), or the import path is a suffix of the
            // directory (vendored and staged copies).
            let dir = parent_dir(def_path);
            if dir.is_empty() {
                return Fit::No;
            }
            if ends_with_components(target, dir) || ends_with_components(dir, target) { Fit::Exact } else { Fit::No }
        }
        Lang::Python => {
            let stem = module_stem(lang, def_path);
            let dots = target.bytes().take_while(|&b| b == b'.').count();
            let rest = target[dots..].replace('.', "/");
            if dots > 0 {
                let mut base = parent_dir(from_path).to_string();
                for _ in 1..dots {
                    base = parent_dir(&base).to_string();
                }
                let full = if rest.is_empty() { base } else { norm_join(&base, &rest) };
                if stem == full {
                    Fit::Exact
                } else if stem.starts_with(&full) && stem.as_bytes().get(full.len()) == Some(&b'/') {
                    Fit::Inside
                } else {
                    Fit::No
                }
            } else if ends_with_components(stem, &rest) {
                Fit::Exact
            } else if stem.starts_with(&format!("{rest}/")) || stem.contains(&format!("/{rest}/")) {
                Fit::Inside
            } else {
                Fit::No
            }
        }
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
            let stem = module_stem(lang, def_path);
            let t = strip_js_ext(target);
            if t.starts_with('.') {
                let full = norm_join(parent_dir(from_path), t);
                let full = full.strip_suffix("/index").unwrap_or(&full);
                if stem == full {
                    Fit::Exact
                } else if stem.starts_with(full) && stem.as_bytes().get(full.len()) == Some(&b'/') {
                    Fit::Inside
                } else {
                    Fit::No
                }
            } else {
                let t = t.trim_start_matches("@/").trim_start_matches("~/");
                let t = t.strip_suffix("/index").unwrap_or(t);
                if ends_with_components(stem, t) { Fit::Exact } else { Fit::No }
            }
        }
        Lang::Rust => {
            let stem = module_stem(lang, def_path);
            let mut t = target.replace("::", "/");
            let from_mod = module_stem(lang, from_path).to_string();
            // `mod.rs`, `lib.rs` and `main.rs` are the module of their directory.
            let from_dir = if from_path.ends_with("/mod.rs")
                || from_path.ends_with("/lib.rs")
                || from_path.ends_with("/main.rs")
                || from_path == "lib.rs"
                || from_path == "main.rs"
            {
                parent_dir(from_path).to_string()
            } else {
                from_mod.clone()
            };
            if let Some(rest) = t.strip_prefix("crate/") {
                // `crate::` starts at the root of the crate the importing file belongs to, which
                // is its `src` directory. A module of the same name elsewhere does not count.
                if let Some(root) = crate_src_root(from_path) {
                    let full = format!("{root}/{rest}");
                    let stem_dir = stem.strip_suffix("/lib").or_else(|| stem.strip_suffix("/main")).unwrap_or(stem);
                    if stem == full || stem_dir == full {
                        return Fit::Exact;
                    }
                    let mut cut = full.as_str();
                    for _ in 0..2 {
                        match cut.rfind('/') {
                            Some(i) if i > root.len() => cut = &cut[..i],
                            _ => break,
                        }
                        if stem == cut {
                            return Fit::Inside;
                        }
                    }
                    return Fit::No;
                }
                t = rest.to_string();
            } else if t == "crate" || t == "self" {
                return if stem == from_dir { Fit::Exact } else { Fit::No };
            } else if let Some(rest) = t.strip_prefix("self/") {
                t = norm_join(&from_dir, rest);
            } else if t.starts_with("super") {
                let mut base = parent_dir(&from_dir).to_string();
                let mut rest = t.strip_prefix("super").unwrap_or("").trim_start_matches('/').to_string();
                while let Some(r) = rest.strip_prefix("super") {
                    base = parent_dir(&base).to_string();
                    rest = r.trim_start_matches('/').to_string();
                }
                t = if rest.is_empty() { base } else { norm_join(&base, &rest) };
            }
            let stem_dir = stem.strip_suffix("/lib").or_else(|| stem.strip_suffix("/main")).unwrap_or(stem);
            if ends_with_components(stem, &t) || ends_with_components(stem_dir, &t) {
                return Fit::Exact;
            }
            // The path may name an item inside a module (`a::b::Type`): drop trailing segments.
            let mut cut = t.as_str();
            for _ in 0..2 {
                match cut.rfind('/') {
                    Some(i) => cut = &cut[..i],
                    None => break,
                }
                if ends_with_components(stem, cut) || ends_with_components(stem_dir, cut) {
                    return Fit::Inside;
                }
            }
            // A crate name as the first segment: the rest names the module (`foo::bar` -> `bar`).
            if let Some(i) = t.find('/') {
                let rest = &t[i + 1..];
                if ends_with_components(stem, rest) || ends_with_components(stem_dir, rest) {
                    return Fit::Exact;
                }
            }
            Fit::No
        }
        Lang::Java | Lang::Kotlin | Lang::Scala => {
            let t = target.replace('.', "/");
            let stem = strip_ext(def_path);
            if ends_with_components(stem, &t) {
                Fit::Exact
            } else if ends_with_components(parent_dir(def_path), &t) {
                Fit::Inside
            } else {
                Fit::No
            }
        }
        _ => Fit::No,
    }
}

/// The key under which a file is indexed for module lookup: the last component of the module
/// path it answers to (`a/b/util.ts` -> `util`, `a/b/index.ts` -> `b`), or of its directory for
/// languages where a directory is the package (Go).
pub fn module_key(lang: Lang, path: &str) -> &str {
    let s = if lang == Lang::Go { parent_dir(path) } else { module_stem(lang, path) };
    match s.rfind('/') {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

/// The module-lookup key of an import target written in `from_path`: the last component of the
/// path it names, with relative forms resolved. `None` when the target names no single
/// component (`.`, `crate`, `super`).
pub fn target_key(lang: Lang, from_path: &str, target: &str) -> Option<String> {
    let full: String = match lang {
        Lang::Go => target.trim_end_matches('/').to_string(),
        Lang::Python => {
            let dots = target.bytes().take_while(|&b| b == b'.').count();
            let rest = target[dots..].replace('.', "/");
            if dots > 0 {
                let mut base = parent_dir(from_path).to_string();
                for _ in 1..dots {
                    base = parent_dir(&base).to_string();
                }
                if rest.is_empty() { base } else { norm_join(&base, &rest) }
            } else {
                rest
            }
        }
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
            let t = strip_js_ext(target);
            let full = if t.starts_with('.') { norm_join(parent_dir(from_path), t) } else { t.trim_start_matches("@/").trim_start_matches("~/").to_string() };
            full.strip_suffix("/index").unwrap_or(&full).to_string()
        }
        Lang::Rust => {
            let t = target.replace("::", "/");
            let last = t.rsplit('/').next().unwrap_or("");
            if matches!(last, "crate" | "self" | "super" | "") {
                return None;
            }
            t
        }
        Lang::Java | Lang::Kotlin | Lang::Scala => target.replace('.', "/"),
        // `import Darwin.C` names the module `Darwin`.
        Lang::Swift => return target.split('.').next().filter(|m| !m.is_empty()).map(|m| m.to_string()),
        _ => return None,
    };
    let last = full.rsplit('/').next().unwrap_or("");
    if last.is_empty() || last == "." || last == ".." { None } else { Some(last.to_string()) }
}

/// Languages where every file of a directory shares one namespace (unqualified names resolve
/// across the files of the package). PHP files share the namespace they declare instead.
pub fn dir_is_package(lang: Lang) -> bool {
    matches!(lang, Lang::Go | Lang::Java | Lang::Kotlin | Lang::Scala | Lang::Php | Lang::Swift)
}

/// The targets a `Package.swift` in directory `dir` declares: (directory of the target's
/// sources, module name). A target without `path:` lives where SwiftPM looks for it:
/// `Sources/NAME`, `Tests/NAME` for a test target, `Plugins/NAME` for a plugin. The manifest
/// is Swift code, so this is a scan for the target constructors, not an evaluation.
pub fn swift_manifest_targets(dir: &str, text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let b = text.as_bytes();
    let string_after = |region: &str, label: &str| -> Option<String> {
        let i = region.find(label)?;
        let rest = region[i + label.len()..].trim_start();
        let rest = rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    };
    for (ctor, default_dir) in [
        (".target(", "Sources"),
        (".executableTarget(", "Sources"),
        (".macro(", "Sources"),
        (".systemLibrary(", "Sources"),
        (".testTarget(", "Tests"),
        (".plugin(", "Plugins"),
    ] {
        let mut from = 0;
        while let Some(i) = text[from..].find(ctor) {
            let start = from + i + ctor.len();
            from = start;
            // The arguments, up to the matching parenthesis.
            let mut depth = 1usize;
            let mut j = start;
            while j < b.len() && depth > 0 {
                match b[j] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let region = &text[start..j.saturating_sub(1).max(start)];
            let Some(name) = string_after(region, "name:") else { continue };
            let rel = string_after(region, "path:").unwrap_or_else(|| format!("{default_dir}/{name}"));
            let rel = rel.trim_end_matches('/');
            let full = if dir.is_empty() {
                rel.to_string()
            } else if rel.is_empty() {
                dir.to_string()
            } else {
                format!("{dir}/{rel}")
            };
            if !out.iter().any(|(d, n)| *d == full && *n == name) {
                out.push((full, name));
            }
        }
    }
    out
}

/// The Swift module of a file no manifest places: the directory below `Sources/` or `Tests/`
/// (the SwiftPM layout), else the top directory of the path (the folder of an Xcode target).
/// Returns (the module's directory, its name).
pub fn swift_module_by_convention(path: &str) -> (String, String) {
    let dir = parent_dir(path);
    let comps: Vec<&str> = dir.split('/').filter(|c| !c.is_empty()).collect();
    if let Some(i) = comps.iter().rposition(|c| matches!(*c, "Sources" | "Tests" | "Source")) {
        if i + 1 < comps.len() {
            return (comps[..=i + 1].join("/"), comps[i + 1].to_string());
        }
        let name = if i > 0 { comps[i - 1] } else { comps[i] };
        return (comps[..=i].join("/"), name.to_string());
    }
    match comps.first() {
        Some(first) => (first.to_string(), first.to_string()),
        None => (String::new(), String::new()),
    }
}

pub fn same_dir(a: &str, b: &str) -> bool {
    parent_dir(a) == parent_dir(b)
}

/// The last segment of a module path (`a.b.c`, `a/b/c`, `a::b::c`): what a whole-module import
/// binds locally by default.
pub fn last_segment(target: &str) -> &str {
    let t = target.trim_end_matches('/');
    let i = t.rfind(|c: char| c == '/' || c == '.' || c == ':').map(|i| i + 1).unwrap_or(0);
    &t[i..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_packages() {
        let m = |t: &str, d: &str| module_match(Lang::Go, "cmd/root.go", t, d);
        assert_eq!(m("github.com/zricethezav/gitleaks/v8/report", "report/finding.go"), Fit::Exact);
        assert_eq!(m("github.com/zricethezav/gitleaks/v8/report", "cmd/report/x.go"), Fit::No);
        assert_eq!(m("k8s.io/api/core/v1", "staging/src/k8s.io/api/core/v1/types.go"), Fit::Exact);
        assert_eq!(m("k8s.io/api/core/v1", "vendor/k8s.io/api/core/v1/types.go"), Fit::Exact);
        assert_eq!(m("fmt", "pkg/fmt2/x.go"), Fit::No);
    }

    #[test]
    fn python_modules() {
        let m = |from: &str, t: &str, d: &str| module_match(Lang::Python, from, t, d);
        assert_eq!(m("django/shortcuts.py", "django.http", "django/http/__init__.py"), Fit::Exact);
        assert_eq!(m("django/shortcuts.py", "django.http", "django/http/response.py"), Fit::Inside);
        assert_eq!(m("django/shortcuts.py", "django.template.loader", "django/template/loader.py"), Fit::Exact);
        assert_eq!(m("django/db/models/query.py", ".sql", "django/db/models/sql/query.py"), Fit::Inside);
        assert_eq!(m("django/db/models/query.py", "..utils", "django/db/utils.py"), Fit::Exact);
        assert_eq!(m("a/b.py", "os.path", "django/utils/os.py"), Fit::No);
        assert_eq!(m("pkg/a.py", "pkg.util", "src/pkg/util.py"), Fit::Exact);
    }

    #[test]
    fn js_modules() {
        let m = |from: &str, t: &str, d: &str| module_match(Lang::TypeScript, from, t, d);
        assert_eq!(m("src/vs/a/b.ts", "./c", "src/vs/a/c.ts"), Fit::Exact);
        assert_eq!(m("src/vs/a/b.ts", "../base/common/async.js", "src/vs/base/common/async.ts"), Fit::Exact);
        assert_eq!(m("src/vs/a/b.ts", "./widgets", "src/vs/a/widgets/index.ts"), Fit::Exact);
        assert_eq!(m("src/vs/a/b.ts", "vs/base/common/event", "src/vs/base/common/event.ts"), Fit::Exact);
        assert_eq!(m("src/vs/a/b.ts", "./c", "src/vs/x/c.ts"), Fit::No);
    }

    #[test]
    fn rust_modules() {
        let m = |from: &str, t: &str, d: &str| module_match(Lang::Rust, from, t, d);
        assert_eq!(m("src/query/find.rs", "crate::store", "src/store.rs"), Fit::Exact);
        assert_eq!(m("src/query/find.rs", "super::show", "src/query/show.rs"), Fit::Exact);
        assert_eq!(m("src/query/mod.rs", "self::refs", "src/query/refs.rs"), Fit::Exact);
        assert_eq!(m("src/query/find.rs", "crate::index::format", "src/index/format.rs"), Fit::Exact);
        assert_eq!(m("src/query/find.rs", "crate::index", "src/index/mod.rs"), Fit::Exact);
        assert_eq!(m("src/query/find.rs", "crate::store::Index", "src/store.rs"), Fit::Inside);
        assert_eq!(m("compiler/a/src/x.rs", "rustc_middle::ty", "compiler/rustc_middle/src/ty/mod.rs"), Fit::Exact);
        assert_eq!(m("src/query/find.rs", "crate::store", "src/other/store.rs"), Fit::No);
        assert_eq!(rust_anchor_files("a/src/x/y.rs", "crate"), vec!["a/src/lib.rs".to_string(), "a/src/main.rs".to_string()]);
        assert!(rust_anchor_files("a/src/x/y.rs", "super").contains(&"a/src/x.rs".to_string()));
        assert!(rust_anchor_files("a/src/x/mod.rs", "super").contains(&"a/src/lib.rs".to_string()));
        assert_eq!(rust_anchor_files("a/src/x/y.rs", "self"), vec!["a/src/x/y.rs".to_string()]);
        assert!(rust_anchor_files("a/src/x/y.rs", "crate::store").is_empty());
        assert_eq!(m("a/src/x.rs", "crate::store", "b/src/store.rs"), Fit::No);
        assert_eq!(m("a/src/x/y.rs", "crate::store::Index", "a/src/store/mod.rs"), Fit::Inside);
    }

    #[test]
    fn java_packages() {
        let m = |t: &str, d: &str| module_match(Lang::Java, "x/A.java", t, d);
        assert_eq!(m("org.elasticsearch.action.ActionListener", "server/src/main/java/org/elasticsearch/action/ActionListener.java"), Fit::Exact);
        assert_eq!(m("org.elasticsearch.action", "server/src/main/java/org/elasticsearch/action/ActionListener.java"), Fit::Inside);
        assert_eq!(m("org.elasticsearch.other", "server/src/main/java/org/elasticsearch/action/ActionListener.java"), Fit::No);
    }

    #[test]
    fn module_keys() {
        assert_eq!(module_key(Lang::TypeScript, "src/vs/a/widgets/index.ts"), "widgets");
        assert_eq!(module_key(Lang::TypeScript, "src/vs/a/c.d.ts"), "c");
        assert_eq!(module_key(Lang::Python, "django/http/__init__.py"), "http");
        assert_eq!(module_key(Lang::Go, "syft/format/decoder.go"), "format");
        assert_eq!(module_key(Lang::Rust, "src/index/mod.rs"), "index");
        assert_eq!(target_key(Lang::TypeScript, "src/v4/classic/compat.ts", "../core/index.js").as_deref(), Some("core"));
        assert_eq!(target_key(Lang::TypeScript, "src/a/b.ts", "./c.js").as_deref(), Some("c"));
        assert_eq!(target_key(Lang::Python, "django/db/models/query.py", "..utils").as_deref(), Some("utils"));
        assert_eq!(target_key(Lang::Python, "django/db/models/query.py", ".").as_deref(), Some("models"));
        assert_eq!(target_key(Lang::Go, "cmd/root.go", "k8s.io/api/core/v1").as_deref(), Some("v1"));
        assert_eq!(target_key(Lang::Rust, "src/query/find.rs", "crate::store").as_deref(), Some("store"));
        assert_eq!(target_key(Lang::Rust, "src/query/find.rs", "crate"), None);
        assert_eq!(target_key(Lang::Java, "x/A.java", "org.acme.io.Channel").as_deref(), Some("Channel"));
    }

    #[test]
    fn swift_modules() {
        let manifest = r#"let package = Package(name: "Alamofire",
            targets: [.target(name: "Alamofire", path: "Source", exclude: ["Info.plist"]),
                      .testTarget(name: "AlamofireTests", dependencies: [.target(name: "Alamofire")], path: "Tests"),
                      .target(name: "NIOCore", dependencies: ["X"])])"#;
        let t = swift_manifest_targets("", manifest);
        assert!(t.contains(&("Source".to_string(), "Alamofire".to_string())));
        assert!(t.contains(&("Tests".to_string(), "AlamofireTests".to_string())));
        assert!(t.contains(&("Sources/NIOCore".to_string(), "NIOCore".to_string())));
        assert_eq!(swift_manifest_targets("pkg", manifest)[0].0, "pkg/Source");
        let c = |p: &str| swift_module_by_convention(p);
        assert_eq!(c("Sources/NIOCore/Channel/Channel.swift"), ("Sources/NIOCore".to_string(), "NIOCore".to_string()));
        assert_eq!(c("Tests/NIOCoreTests/A.swift"), ("Tests/NIOCoreTests".to_string(), "NIOCoreTests".to_string()));
        assert_eq!(c("MyApp/Views/Home.swift"), ("MyApp".to_string(), "MyApp".to_string()));
        assert_eq!(c("main.swift"), (String::new(), String::new()));
        assert_eq!(target_key(Lang::Swift, "a.swift", "Darwin.C").as_deref(), Some("Darwin"));
    }

    #[test]
    fn joins() {
        assert_eq!(norm_join("a/b", "../c/./d"), "a/c/d");
        assert_eq!(last_segment("a.b.c"), "c");
        assert_eq!(last_segment("k8s.io/api/core/v1"), "v1");
        assert_eq!(last_segment("crate::store"), "store");
    }
}
