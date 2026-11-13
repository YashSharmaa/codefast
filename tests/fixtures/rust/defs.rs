//! Definitions: every item kind, with visibility, test and async flags, and imports.
#![allow(dead_code)]

extern crate alloc as al;
pub extern crate core;

use std::collections::{self, hash_map::Entry, HashMap as Map};
use std::fmt;
use super::*;
pub use crate::net::{Client, Server as Srv};
pub(crate) use self::inner::deep::*;
use ::serde::Serialize;

pub const LIMIT: usize = 16;
static mut COUNTER: Option<Counter> = None;
pub static HOOK: fn(&Config, u32) -> bool = default_hook;
pub type Result<T> = std::result::Result<T, Error>;
type Shared = Arc<Config>;

/// A stack.
#[derive(Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Stack<T: Clone> {
    pub items: Vec<T>,
    cfg: Arc<Config>,
    parent: Option<Box<Stack<T>>>,
    pub(crate) on_push: Box<dyn Fn(&T, usize) + Send>,
    cb: Option<fn(u32) -> u32>,
    name: &'static str,
}

pub struct Pair(pub u32, Config);
struct Unit;

pub union Bits {
    int: u32,
    float: f32,
}

pub enum Shape {
    Empty,
    Circle(f64),
    #[deprecated]
    Rect { w: f64, h: Size },
    Tagged = 7,
}

pub trait Area: fmt::Debug + Send {
    const SIDES: u32;
    type Unit: Default;
    fn area(&self) -> f64;
    fn describe(&self, verbose: bool) -> String {
        String::new()
    }
}

impl<T: Clone> Stack<T> {
    pub const DEPTH: usize = 8;

    pub fn new(cfg: Arc<Config>) -> Self {
        todo!()
    }

    pub async fn drain(&mut self, n: usize, mut sink: impl Sink) -> Result<Vec<T>> {
        fn helper(x: u32) -> u32 {
            x
        }
        todo!()
    }

    fn peek(&self) -> Option<&T> {
        None
    }
}

impl Area for Shape {
    const SIDES: u32 = 4;
    type Unit = Meters;
    fn area(&self) -> f64 {
        0.0
    }
}

impl<T: fmt::Display> Area for T {}
impl<'a, T> fmt::Debug for &'a Stack<T> {}
impl<T> [T] {
    fn first_or(&self, d: T) -> T {
        d
    }
}
impl dyn Area {}
unsafe impl Send for Bits {}

pub mod net;
mod inner {
    pub(super) mod deep {
        pub fn dig(depth: u8, ...) -> ! {
            loop {}
        }
    }
    const HIDDEN: u8 = 1;
}

extern "C" {
    pub fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    static errno: i32;
    type Opaque;
}

#[macro_export]
macro_rules! square {
    ($x:expr) => {
        $x * $x
    };
}

#[tokio::main]
async fn main() {}

#[inline]
#[instrument(skip(cfg))]
pub(crate) unsafe fn tuple_ret(cfg: &Config) -> (u8, u8) {
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Stack<u8> {
        todo!()
    }

    #[test]
    fn pushes() {}

    #[tokio::test]
    async fn drains() {}
}

#[cfg(all(test, feature = "slow"))]
fn only_in_tests() {}

#[cfg(not(test))]
fn never_in_tests() {}
