//! Lexical word extraction: the text side of the index.
//!
//! Every text file, parsed or not, contributes the identifier-like words it contains. That gives
//! an exact answer to "which files mention X at all" (comments, strings, assembly, build files,
//! docs), which is what lets structural answers state what they do not cover, and it lets text
//! searches visit only the files that can match.

use crate::facts::LocalNames;

pub const MIN_WORD: usize = 2;
pub const MAX_WORD: usize = 64;

const fn build_class() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let b = i as u8;
        let alpha = (b >= b'a' && b <= b'z') || (b >= b'A' && b <= b'Z') || b == b'_';
        let digit = b >= b'0' && b <= b'9';
        t[i] = if alpha {
            1
        } else if digit {
            2
        } else {
            0
        };
        i += 1;
    }
    t
}

/// 1 = may start a word, 2 = may continue one, 0 = separator.
static CLASS: [u8; 256] = build_class();

#[inline]
pub fn is_word_byte(b: u8) -> bool {
    CLASS[b as usize] != 0
}

/// Count the words of `src` into `counts`, indexed by the id each word gets in `names`.
pub fn count_words(src: &[u8], names: &mut LocalNames, counts: &mut Vec<u32>) {
    let n = src.len();
    let mut i = 0usize;
    while i < n {
        let c = CLASS[src[i] as usize];
        if c == 0 {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < n && CLASS[src[i] as usize] != 0 {
            i += 1;
        }
        // Tokens starting with a digit are numbers, not words.
        if c != 1 {
            continue;
        }
        let len = i - start;
        if !(MIN_WORD..=MAX_WORD).contains(&len) {
            continue;
        }
        let id = names.intern(&src[start..i]) as usize;
        if id >= counts.len() {
            counts.resize(id + 1, 0);
        }
        counts[id] += 1;
    }
}

/// File types that are never text worth indexing, or whose "words" would only pollute the lexicon.
pub fn skip_text(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    if matches!(file, "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "Cargo.lock" | "go.sum" | "poetry.lock" | "composer.lock" | "Gemfile.lock") {
        return true;
    }
    let ext = match file.rfind('.') {
        Some(i) => &file[i + 1..],
        None => return false,
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "ico"
            | "webp"
            | "svg"
            | "pdf"
            | "zip"
            | "gz"
            | "tgz"
            | "bz2"
            | "xz"
            | "zst"
            | "7z"
            | "tar"
            | "jar"
            | "war"
            | "class"
            | "o"
            | "a"
            | "so"
            | "dylib"
            | "dll"
            | "exe"
            | "bin"
            | "dat"
            | "db"
            | "sqlite"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
            | "eot"
            | "mp3"
            | "mp4"
            | "mov"
            | "avi"
            | "wav"
            | "ogg"
            | "flac"
            | "pyc"
            | "pyo"
            | "mo"
            | "po"
            | "wasm"
            | "node"
            | "map"
            | "snap"
            | "pb"
            | "onnx"
            | "npy"
            | "parquet"
            | "icns"
            | "psd"
            | "keystore"
            | "p12"
            | "der"
            | "crt"
            | "pem"
    ) || file.ends_with(".min.js")
        || file.ends_with(".min.css")
}
