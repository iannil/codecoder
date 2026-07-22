//! Persistent Capability 的跨重启监督状态(spec 2026-07-22 #3 / ADR 0034)。
//! 持久化 Supervisor 的判定状态(gave_up/crash_count/manifest mtime)到
//! `<root>/supervisor_state.json`。daemon 重启后:超预算/gave_up 的服务被跳过;
//! manifest 变更自动重置。会话内仍守 ADR 0021(崩了不自动重启)——
//! 预算只管"重启后是否再 spawn"。**不**持久化 RunningServiceTable 的 live handles。
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SupervisorState {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub services: std::collections::HashMap<String, ServiceEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ServiceEntry {
    #[serde(default)]
    pub gave_up: bool,
    #[serde(default)]
    pub crash_count: u32,
    #[serde(default)]
    pub manifest_mtime_secs: u64,
}

pub fn state_path(root: &Path) -> PathBuf {
    root.join("supervisor_state.json")
}

/// 读状态;文件缺失/损坏 → 默认空(不阻塞 daemon 启动)。
pub fn load(root: &Path) -> SupervisorState {
    let Ok(raw) = std::fs::read_to_string(state_path(root)) else {
        return SupervisorState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// 写状态(atomic:先写 tmp 再 rename)。失败返 Err(调用方记警告)。
pub fn save(root: &Path, state: &SupervisorState) -> anyhow::Result<()> {
    let path = state_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(state)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &raw)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// manifest.json 的 mtime(epoch 秒);不可用时 0。
pub fn mtime_of(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 若记录的 mtime ≠ cur_mtime → 清 gave_up/crash_count 并刷新 mtime;返回是否 reset。
/// 服务无记录时:视为首次(写入 mtime,返回 false)。
pub fn reset_if_manifest_changed(state: &mut SupervisorState, name: &str, cur_mtime: u64) -> bool {
    match state.services.get_mut(name) {
        Some(e) if e.manifest_mtime_secs != cur_mtime => {
            e.gave_up = false;
            e.crash_count = 0;
            e.manifest_mtime_secs = cur_mtime;
            true
        }
        Some(e) => {
            e.manifest_mtime_secs = cur_mtime; // 确保刷新(即便未变)
            false
        }
        None => {
            state.services.insert(
                name.to_string(),
                ServiceEntry { gave_up: false, crash_count: 0, manifest_mtime_secs: cur_mtime },
            );
            false
        }
    }
}

/// 该服务是否应被 start_all 跳过。budget=0 时仅看 gave_up(永不因 crash_count 跳过)。
pub fn should_skip(state: &SupervisorState, name: &str, budget: u32) -> bool {
    match state.services.get(name) {
        Some(e) if e.gave_up => true,
        Some(e) if budget > 0 && e.crash_count >= budget => true,
        _ => false,
    }
}

/// 记录一次崩溃:crash_count++;budget>0 且达预算 → gave_up=true。
pub fn record_crash(state: &mut SupervisorState, name: &str, budget: u32) {
    let e = state.services.entry(name.to_string()).or_default();
    e.crash_count = e.crash_count.saturating_add(1);
    if budget > 0 && e.crash_count >= budget {
        e.gave_up = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_missing_returns_default() {
        let dir = tempdir().unwrap();
        let s = load(dir.path());
        assert!(s.services.is_empty());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tempdir().unwrap();
        let mut s = SupervisorState::default();
        s.services.insert(
            "flaky".into(),
            ServiceEntry { gave_up: true, crash_count: 3, manifest_mtime_secs: 123 },
        );
        save(dir.path(), &s).unwrap();
        let back = load(dir.path());
        assert_eq!(back.services.get("flaky").unwrap().crash_count, 3);
        assert!(back.services.get("flaky").unwrap().gave_up);
    }

    #[test]
    fn load_corrupt_returns_default() {
        let dir = tempdir().unwrap();
        std::fs::write(state_path(dir.path()), "{not json").unwrap();
        assert!(load(dir.path()).services.is_empty(), "损坏文件应回退默认");
    }

    #[test]
    fn record_crash_increments_and_trips_budget() {
        let mut s = SupervisorState::default();
        record_crash(&mut s, "x", 3);
        assert_eq!(s.services["x"].crash_count, 1);
        assert!(!s.services["x"].gave_up, "未达预算不该 give_up");
        record_crash(&mut s, "x", 3);
        record_crash(&mut s, "x", 3);
        assert_eq!(s.services["x"].crash_count, 3);
        assert!(s.services["x"].gave_up, "达预算应 give_up");
    }

    #[test]
    fn reset_if_manifest_changed_clears_when_mtime_differs() {
        let mut s = SupervisorState::default();
        s.services.insert(
            "x".into(),
            ServiceEntry { gave_up: true, crash_count: 3, manifest_mtime_secs: 100 },
        );
        assert!(reset_if_manifest_changed(&mut s, "x", 999), "mtime 不同应 reset");
        assert_eq!(s.services["x"].crash_count, 0);
        assert!(!s.services["x"].gave_up);
        assert_eq!(s.services["x"].manifest_mtime_secs, 999);
        assert!(!reset_if_manifest_changed(&mut s, "x", 999), "mtime 相同不应 reset");
    }

    #[test]
    fn should_skip_respects_gave_up_and_budget() {
        let mut s = SupervisorState::default();
        s.services.insert("g".into(), ServiceEntry { gave_up: true, crash_count: 0, manifest_mtime_secs: 0 });
        s.services.insert("c".into(), ServiceEntry { gave_up: false, crash_count: 3, manifest_mtime_secs: 0 });
        s.services.insert("ok".into(), ServiceEntry { gave_up: false, crash_count: 1, manifest_mtime_secs: 0 });
        assert!(should_skip(&s, "g", 3));
        assert!(should_skip(&s, "c", 3), "crash_count≥budget 应 skip");
        assert!(!should_skip(&s, "ok", 3));
        assert!(!should_skip(&s, "c", 0), "budget=0 时即使 crash_count 高也不 skip");
        assert!(should_skip(&s, "g", 0), "budget=0 时 gave_up 仍 skip");
    }
}
