//! Trace observability system (spec 2026-07-29).
//! Enable with `CODECODER_TRACE=1` env var.
//! Writes to `<root>/.ccd.trace.ndjson`.
pub mod types;
pub mod emitter;
pub mod writer;

pub use types::*;
pub use emitter::TraceEmitter;
pub use writer::TraceWriter;

pub fn init_trace(root: &std::path::Path) -> Option<TraceEmitter> {
    let enabled = std::env::var("CODECODER_TRACE")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    if !enabled { return None; }
    let tx = TraceWriter::spawn(root);
    let session_id = root.file_stem().and_then(|s| s.to_str()).unwrap_or("agent").to_string();
    Some(TraceEmitter::new(tx, &session_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 串行化 env 操作测试（与其它 env 测试共享）。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn init_trace_returns_none_when_env_not_set() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var("CODECODER_TRACE"); }
        assert!(init_trace(&tempdir().unwrap().path()).is_none());
    }

    #[test]
    fn init_trace_returns_some_when_env_is_1() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var("CODECODER_TRACE", "1"); }
        assert!(init_trace(&tempdir().unwrap().path()).is_some());
        unsafe { std::env::remove_var("CODECODER_TRACE"); }
    }

    #[test]
    fn init_trace_returns_some_when_env_is_true() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::set_var("CODECODER_TRACE", "true"); }
        assert!(init_trace(&tempdir().unwrap().path()).is_some());
        unsafe { std::env::remove_var("CODECODER_TRACE"); }
    }
}