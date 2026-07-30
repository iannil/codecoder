//! Observers — concrete Observer implementations for the trace system.
//!
//! Each submodule implements `crate::trace::observer_set::Observer` and consumes
//! trace events from one specific backend (file, network, buffer, etc.).

pub mod trace_writer;
pub use trace_writer::TraceWriterObserver;

pub mod replay_buffer;
pub use replay_buffer::ReplayBufferObserver;

pub mod sse_observer;
pub use sse_observer::SseObserver;