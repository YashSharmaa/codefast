//! The 2026-11-13 contract: module-qualified names, receiver type paths, import records.
use crate::query as q;
use crate::store::{self, Index};
use crate::{cfg, net::Conn as Link};
use std::collections::hash_map::{Entry, HashMap};
use std::fmt;
pub use crate::a::B;
pub use crate::wild::*;
use super::sibling::*;
extern crate alloc as al;

mod inline {
    pub fn local_fn() {}
}
mod child;

pub struct Engine {
    store: store::Index,
    by_name: Index,
    conn: Option<Link>,
    table: HashMap<u32, cfg::Entry>,
    err: crate::a::b::Failure,
    out: Box<dyn fmt::Write>,
    plain: Plain,
}

pub fn open(path: &str) -> store::Index {
    let s: store::Index = store::open(path);
    s.flush();
    let t = store::Index::new();
    t.flush();
    let u = store::Index::load(path);
    u.flush();
    let v = crate::a::b::Type::new();
    v.touch();
    let w = store::open(path);
    w.flush();
    s
}

impl Engine {
    pub fn new() -> Self {
        Self::build(q::run(), q::Plan::default())
    }

    fn build(n: u32, plan: q::Plan) -> Self {
        let made = Self::new();
        made.step();
        let other = Self::build(1, plan);
        other.step();
        let from = Engine::from_parts(1);
        from.step();
        let with = Engine::with_store(2);
        with.step();
        let conf = Engine::configure(3);
        conf.step();
        made
    }

    fn step(&self) -> &Plain {
        self.store.get(1).unwrap().name();
        self.plain.render().finish();
        self.step().render();
        self.by_name.lookup("x").len();
        self.conn.as_ref().unwrap().send();
        cfg::DEFAULT.load();
        cfg::current().load();
        Mode::Fast.describe();
        Self::LIMIT.check();
        inline::local_fn();
        child::remote_fn();
        al::vec::Vec::new();
        <Plain as fmt::Display>::fmt(&self.plain, f);
        <Self as Runner>::run(self);
        <Engine>::build(1, p);
        crate::a::b::Type::CONST;
        std::mem::swap(a, b);
        ::std::mem::drop(a);
        fmt::format(args);
        u32::from_str_radix("1", 10).unwrap();
        let lit = store::Index { root: 1 };
        lit.flush();
        let e = cfg::Entry::Vacant(1);
        e.insert();
        let p = store::Pair(1, 2);
        p.swap();
        &self.plain
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

impl q::Runner for Engine {}

pub trait Deep: q::Runner + fmt::Debug {
    fn dig(&self) -> store::Index;
}

fn generic<T: q::Runner>(x: T, y: Box<store::Index>, z: Option<store::Index>) -> cfg::Entry {
    x.run();
    y.flush();
    z.unwrap().flush();
    let k = match z {
        Some(i) => i.flush(),
        None => store::Index::default().flush(),
    };
    make!(store::Index::new(), q::run(), Self::helper(), cfg::Entry { a: 1 });
    todo!()
}

macro_rules! tuple_like {
    ($($T:ident)+) => {
        #[stable(feature = "x")]
        impl<$($T: PartialEq),+> PartialEq for ($($T,)+) {
            fn eq(&self, other: &($($T,)+)) -> bool { true }
        }
        #[inline]
        impl<T> From<[T; 2]> for Wrapper {
            fn from(a: [T; 2], b: Vec<(u8, u8)>) -> q::Plan { store::open(a).flush() }
        }
        impl<T> Marker for [T] {}
        impl Marker for () {}
    };
}

pub enum Shape {
    Rect { w: f64 },
}
