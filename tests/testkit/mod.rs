// tests/testkit/mod.rs — shared black-box test harness (compiles against the
// public codecoder API only).
//
// Each tests/*.rs integration file is its own crate that `mod testkit;`, so any
// binary only exercises a subset of this shared surface. The unused remainder is
// expected, not dead — silence the per-binary dead_code/unused warnings here.
#![allow(dead_code, unused_imports)]

pub mod driver;
pub mod scripted_provider;
pub mod workspace;

pub use driver::*;
pub use scripted_provider::*;
pub use workspace::*;
