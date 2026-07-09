// tests/testkit/mod.rs — shared black-box test harness (compiles against the
// public codecoder API only).
pub mod driver;
pub mod scripted_provider;
pub mod workspace;

pub use driver::*;
pub use scripted_provider::*;
pub use workspace::*;
