//! Syntax newer than the grammar, and plain syntax errors: the extractor parses a patched
//! copy of such files and must still report the original text.
use core::marker::Destruct;

pub const trait Compare<Rhs = Self>: [const] PartialEq<Rhs> {
    fn compare(&self, other: &Rhs) -> Ordering;
    fn is_less(&self, other: &Rhs) -> bool {
        self.compare(other).is_lt()
    }
}

const impl<T: [const] Compare + [const] Destruct> Compare for Wrapper<T> {
    fn compare(&self, other: &Self) -> Ordering {
        self.0.compare(&other.0)
    }
}

unsafe const impl Send for Wrapper<u8> {}

pub const fn pick<T: [const] Compare, F: const Fn(&T) -> bool>(a: T, b: T, keep: F) -> T {
    let chooser = const |x: &T| keep(x);
    if chooser(&a) { a } else { b }
}

unsafe extern "C" {
    pub safe fn strlen_checked(s: *const u8) -> usize;
    pub unsafe fn raw_copy(dst: *mut u8, src: *const u8);
    pub(crate) type Opaque;
    safe static VERSION: u32;
}

pub impl(crate) trait Sealed {}
pub unsafe auto trait Marker {}
pub trait Shared = Send + Sync + 'static;

/// Declarative macros 2.0.
pub macro point($x:expr, $y:expr) {
    $crate::geometry::Point::new($x, $y)
}

macro select_impl {
    ($($t:ty),*) => { $( impl Sealed for $t {} )* },
    (@count $($x:tt)*) => { 0usize $(+ ${ignore($x)} 1usize)* },
}

pub struct Options {
    pub retries: u32 = 3,
    label: &'static str = "none",
    name: String =
        String::new(),
    limits: Limits,
}

fn floats() -> f64 {
    let half = 0.5f16;
    let wide = 2f128;
    let opts = Options { limits: Limits::default(), .. };
    scale(half as f64, wide as f64, opts.retries)
}

fn before_error(x: u32) -> u32 {
    helper(x)
}

fn broken( {
    let y = ;
}

struct AfterError {
    field: Recovered,
}

fn after_error(v: AfterError) -> Recovered {
    v.field.clone()
}
