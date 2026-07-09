// tests/testkit/mod.rs — shared black-box test harness (compiles against the
// public codecoder API only).
pub mod scripted_provider;
// TODO(Task 3): re-enable driver/workspace
// pub mod driver;
// pub mod workspace;

pub use scripted_provider::*;
// TODO(Task 3): re-enable driver/workspace
// pub use driver::*;
// pub use workspace::*;
