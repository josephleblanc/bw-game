//! Compile-and-run tests for `#[alloc_probe]`. The proc-macro API panics
//! outside macro expansion, so the attribute is exercised the real way —
//! applied to items in this consumer crate. This crate declares no
//! `perf-alloc` feature, so the emitted cfg strips the guard here: these
//! tests cover expansion validity and behavior preservation; the live
//! guard path is covered by `cargo xtask perf check`'s alloc pass on
//! bw-demo.

// The guard's cfg names the *using* crate's feature; undeclared here on
// purpose (see module docs).
#![allow(unexpected_cfgs)]

use alloc_probe::alloc_probe;

#[alloc_probe]
fn plain(x: u32) -> u32 {
    x + 1
}

#[alloc_probe]
pub fn decorated(items: &[u32]) -> u32 {
    // Braces nested inside the body (closures, blocks) must not confuse
    // the body search: the first brace group after `fn` is the body.
    let double = |v: u32| { v * 2 };
    items.iter().map(|&v| { double(v) }).sum()
}

struct Fixture;

impl Fixture {
    #[alloc_probe]
    fn method(&self, x: u32) -> u32 {
        x * 10
    }
}

#[test]
fn probed_functions_keep_their_behavior() {
    assert_eq!(plain(1), 2);
    assert_eq!(decorated(&[1, 2, 3]), 12);
    assert_eq!(Fixture.method(4), 40);
}
