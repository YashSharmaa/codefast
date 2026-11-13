//! Language registry: detection by path, parser cache, dispatch to extractors.

use crate::extract::Scratch;
use crate::facts::FileFacts;
use tree_sitter::Parser;

#[cfg(feature = "lang-c")]
pub mod c;
#[cfg(feature = "lang-cpp")]
pub mod cpp;
#[cfg(feature = "lang-go")]
pub mod go;
#[cfg(feature = "lang-java")]
pub mod java;
#[cfg(feature = "lang-js")]
pub mod js;
#[cfg(feature = "lang-python")]
pub mod python;
#[cfg(feature = "lang-rust")]
pub mod rust;

/// Languages known to the index. Values are stored on disk: only append.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Lang {
    Other = 0,
    C = 1,
    Cpp = 2,
    Python = 3,
    JavaScript = 4,
    TypeScript = 5,
    Tsx = 6,
    Go = 7,
    Rust = 8,
    Java = 9,
    Ruby = 10,
    Php = 11,
    CSharp = 12,
    Kotlin = 13,
    Swift = 14,
    Scala = 15,
    Lua = 16,
    Bash = 17,
    Zig = 18,
    ObjC = 19,
}

pub const LANG_COUNT: usize = 20;

impl Lang {
    pub fn from_u8(v: u8) -> Lang {
        match v {
            1 => Lang::C,
            2 => Lang::Cpp,
            3 => Lang::Python,
            4 => Lang::JavaScript,
            5 => Lang::TypeScript,
            6 => Lang::Tsx,
            7 => Lang::Go,
            8 => Lang::Rust,
            9 => Lang::Java,
            10 => Lang::Ruby,
            11 => Lang::Php,
            12 => Lang::CSharp,
            13 => Lang::Kotlin,
            14 => Lang::Swift,
            15 => Lang::Scala,
            16 => Lang::Lua,
            17 => Lang::Bash,
            18 => Lang::Zig,
            19 => Lang::ObjC,
            _ => Lang::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Other => "other",
            Lang::C => "c",
            Lang::Cpp => "cpp",
            Lang::Python => "python",
            Lang::JavaScript => "javascript",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::Go => "go",
            Lang::Rust => "rust",
            Lang::Java => "java",
            Lang::Ruby => "ruby",
            Lang::Php => "php",
            Lang::CSharp => "csharp",
            Lang::Kotlin => "kotlin",
            Lang::Swift => "swift",
            Lang::Scala => "scala",
            Lang::Lua => "lua",
            Lang::Bash => "bash",
            Lang::Zig => "zig",
            Lang::ObjC => "objc",
        }
    }

    /// Detect the language from a repo-relative path.
    pub fn from_path(path: &str) -> Lang {
        let file = path.rsplit('/').next().unwrap_or(path);
        let ext = match file.rfind('.') {
            Some(i) if i > 0 => &file[i + 1..],
            _ => "",
        };
        match ext {
            "c" | "h" => Lang::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "inl" | "ipp" | "tcc" | "C" | "H" => Lang::Cpp,
            "py" | "pyi" | "pyx" => Lang::Python,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "tsx" => Lang::Tsx,
            "go" => Lang::Go,
            "rs" => Lang::Rust,
            "java" => Lang::Java,
            "rb" | "rake" | "gemspec" => Lang::Ruby,
            "php" => Lang::Php,
            "cs" => Lang::CSharp,
            "kt" | "kts" => Lang::Kotlin,
            "swift" => Lang::Swift,
            "scala" | "sc" => Lang::Scala,
            "lua" => Lang::Lua,
            "sh" | "bash" | "zsh" => Lang::Bash,
            "zig" => Lang::Zig,
            "m" | "mm" => Lang::ObjC,
            _ => Lang::Other,
        }
    }

    /// True when the path is a header / declaration file for its language.
    pub fn is_header(path: &str) -> bool {
        let file = path.rsplit('/').next().unwrap_or(path);
        match file.rfind('.') {
            Some(i) => matches!(&file[i + 1..], "h" | "hpp" | "hh" | "hxx" | "H" | "inl" | "pyi") || file.ends_with(".d.ts"),
            None => false,
        }
    }

    /// Whether a structural extractor exists for this language in this build.
    pub fn has_extractor(self) -> bool {
        self.grammar().is_some()
    }

    /// The tree-sitter grammar for this language, when its extractor is compiled in.
    pub fn grammar(self) -> Option<tree_sitter::Language> {
        match self {
            #[cfg(feature = "lang-c")]
            Lang::C => Some(tree_sitter_c::LANGUAGE.into()),
            #[cfg(feature = "lang-cpp")]
            Lang::Cpp => Some(tree_sitter_cpp::LANGUAGE.into()),
            #[cfg(feature = "lang-python")]
            Lang::Python => Some(tree_sitter_python::LANGUAGE.into()),
            #[cfg(feature = "lang-js")]
            Lang::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
            #[cfg(feature = "lang-js")]
            Lang::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            #[cfg(feature = "lang-js")]
            Lang::Tsx => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
            #[cfg(feature = "lang-go")]
            Lang::Go => Some(tree_sitter_go::LANGUAGE.into()),
            #[cfg(feature = "lang-rust")]
            Lang::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
            #[cfg(feature = "lang-java")]
            Lang::Java => Some(tree_sitter_java::LANGUAGE.into()),
            _ => None,
        }
    }
}

/// Language of a file: the path-based guess, refined by a look at the content. A `.h` file is
/// C++ when it uses C++-only syntax.
pub fn detect(path: &str, src: &[u8]) -> Lang {
    let lang = Lang::from_path(path);
    if lang == Lang::C && path.ends_with(".h") && looks_like_cpp(src) {
        return Lang::Cpp;
    }
    lang
}

fn looks_like_cpp(src: &[u8]) -> bool {
    let head = &src[..src.len().min(64 * 1024)];
    const MARKERS: [&[u8]; 7] = [b"\nnamespace ", b"\ntemplate <", b"\ntemplate<", b"\nclass ", b"std::", b"\nusing namespace ", b"constexpr "];
    MARKERS.iter().any(|m| memchr::memmem::find(head, m).is_some())
}

/// Per-thread parser cache: creating and configuring a parser is not free.
#[derive(Default)]
pub struct Parsers {
    slots: [Option<Parser>; LANG_COUNT],
}

impl Parsers {
    pub fn get(&mut self, lang: Lang) -> Option<&mut Parser> {
        let slot = &mut self.slots[lang as usize];
        if slot.is_none() {
            let ts_lang = lang.grammar()?;
            let mut p = Parser::new();
            p.set_language(&ts_lang).ok()?;
            *slot = Some(p);
        }
        slot.as_mut()
    }
}

/// Parse `src` and extract its facts. Returns false when the language has no extractor or the
/// parse failed; `facts` is then left empty.
pub fn extract(lang: Lang, src: &[u8], path: &str, parsers: &mut Parsers, facts: &mut FileFacts, scratch: &mut Scratch) -> bool {
    facts.clear();
    facts.nlines = count_lines(src);
    if !lang.has_extractor() {
        return false;
    }
    let Some(parser) = parsers.get(lang) else { return false };
    let Some(tree) = parser.parse(src, None) else { return false };
    match lang {
        #[cfg(feature = "lang-c")]
        Lang::C => c::extract(src, &tree, path, facts, scratch),
        #[cfg(feature = "lang-cpp")]
        Lang::Cpp => cpp::extract(lang, src, &tree, path, facts, scratch),
        #[cfg(feature = "lang-python")]
        Lang::Python => python::extract(lang, src, &tree, path, facts, scratch),
        #[cfg(feature = "lang-js")]
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => js::extract(lang, src, &tree, path, facts, scratch),
        #[cfg(feature = "lang-go")]
        Lang::Go => go::extract(lang, src, &tree, path, facts, scratch),
        #[cfg(feature = "lang-rust")]
        Lang::Rust => rust::extract(lang, src, &tree, path, facts, scratch),
        #[cfg(feature = "lang-java")]
        Lang::Java => java::extract(lang, src, &tree, path, facts, scratch),
        _ => return false,
    }
    true
}

pub fn count_lines(src: &[u8]) -> u32 {
    let n = memchr::memchr_iter(b'\n', src).count() as u32;
    if src.last().is_some_and(|&b| b != b'\n') { n + 1 } else { n }
}
