//! Macros: invocations, the token-level pass over their arguments, and the items that
//! `macro_rules!` bodies and item-like invocations define.
use std::cell::RefCell;

macro_rules! int_impl {
    ($SelfT:ty, $BITS:expr) => {
        /// Checked addition.
        #[inline]
        pub const fn checked_add(self, rhs: Self) -> Option<Self> {
            let (a, overflowed) = self.overflowing_add(rhs);
            if intrinsics::unlikely(overflowed) { None } else { Some(a) }
        }
        pub const MAX: Self = !Self::MIN;
        pub fn from_str_radix(src: &str, radix: u32) -> Result<Self, ParseIntError> {
            parse::<$SelfT>(src, radix).map_err(ParseIntError::from)
        }
    };
}

impl i32 {
    int_impl! { i32, 32 }
}

macro_rules! newtypes {
    ($($name:ident($inner:ty)),* $(,)?) => {
        $(
            pub struct $name($inner);
            impl From<$inner> for $name {
                fn from(v: $inner) -> Self { $name(v) }
            }
        )*
        pub enum Kind { Alpha, Beta(u32, Payload), Gamma { x: Coord, pub y: Box<Coord> } }
        pub struct Holder<T: Clone> { pub inner: Arc<T>, count: usize }
        impl<T: Clone> Holder<T> {
            pub fn get(&self, key: &Key) -> &T { key.touch(); &self.inner }
            fn fresh(seed: Seed) -> Self { Self { inner: seed.into(), count: 0 } }
        }
        impl fmt::Display for Holder<u8> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.count) }
        }
        pub trait Visit { fn visit(&mut self, n: &Node); fn done(&self) -> bool { true } type Out; }
        mod generated { pub fn deep() { super::shallow() } }
        type Alias = Holder<u8>;
        macro_rules! nested { () => { inner_call() }; }
    };
}

cfg_if::cfg_if! {
    if #[cfg(unix)] {
        mod unix;
        pub use self::unix::{Handle, raw as sys};
        pub(crate) fn platform_init(flags: Flags) -> io::Result<Handle> { Handle::open(flags.bits()) }
    } else {
        pub fn platform_init() {}
    }
}

bitflags! {
    /// Flags.
    pub struct Flags: u32 {
        const READ = 1 << 0;
        const WRITE = Self::READ.bits() << 1;
    }
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
    pub static DEPTH: Cell<usize> = const { Cell::new(0) };
}

lazy_static! {
    static ref PATTERN: Regex = Regex::new("x").unwrap();
}

impl Holder<u8> {
    fn user(&self, other: &Holder<u8>, items: Vec<Item>) -> usize {
        debug!("{:?} {}", other.get(&KEY), Holder::<u8>::fresh(make_seed()).count);
        let kinds = vec![Kind::Alpha, Kind::Beta(1, Payload::new())];
        assert_eq!(self.inner.len(), compute(kinds.len(), |a, b| a + b), "context {}", self.count);
        assert!(matches!(items.first(), Some(Item { id, .. }) if id.is_valid()), "bad");
        let total = sum!(items.iter().map(|it| it.weight()), start = BASE);
        log::warn!(target: "holder", count = self.count; "slow path {}", describe(total));
        tokio::select! {
            msg = self.inbox.recv() => { self.handle(msg) }
            _ = shutdown.changed() => {}
        }
        let tokens = quote! { fn generated_elsewhere() { #name.call() } };
        let _ = stringify!(not_a_reference);
        if cfg!(debug_assertions) { trace_state() }
        my_dsl!(@rule $crate::rules::apply(1), mem::size_of::<Item>(), 'outer: loop { break 'outer; });
        total
    }
}
