// Cases found by the 2026-11-13 audit.
use std::ffi;
use crate::store;
use crate::kinds::Kind;

// `type Target = module::Type` in a Deref impl: Extends carries the module as qualifier.
impl std::ops::Deref for SmallCStr {
    type Target = ffi::CStr;
    fn deref(&self) -> &ffi::CStr {
        todo!()
    }
}

// Zero-width nodes (`Ty<>`, a unit type as bound subject) name nothing.
struct Unit<T>(T)
where
    (): Bound<T>;
type Empty = Ty<>;

// `Item: Bound` inside generic arguments is an associated type, not a type reference.
fn assoc_bound(it: impl IntoIterator<Item: Clone>) -> Vec<Item> {
    let _x: Box<dyn Iterator<Item = Kind, IntoIter: Send>>;
    Vec::new()
}

// Fields of a struct-like variant are reached through the enum.
fn variants(k: Kind, t: store::Thing) -> u32 {
    let made = Kind::Pair { left: 1, right: helper };
    match t {
        store::Thing::Wide { width } => width.get(),
        store::Thing::Tall(h) => h.get(),
    }
    if let Kind::Pair { left, .. } = k {
        left.get();
    }
    let store::Plain { root } = todo!();
    root.get()
}

// A closure after a block in a macro's own syntax: its parameters are locals.
fn dsl(sock: Socket) {
    callback!(
        @capture<'tcx> { sock: Socket }
        |ecx, unblock: UnblockKind| {
            assert_eq!(unblock, UnblockKind::Ready);
            ecx.write(sock, other)
        }
    );
    table!(flags[0] | MASK, extra);
}

// A `use` whose path contains a metavariable is known only at expansion.
macro_rules! each_fn {
    ($($f:ident),*) => {
        $(fn $f() { use crate::imp::$f; use $crate::store::Index; })*
    };
}
