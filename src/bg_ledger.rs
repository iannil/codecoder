//! BG 任务账本(spec 2026-07-22 #2 / ADR 0033):每次 BG 调用追加一条 JSONL 记录到
//! `<root>/bg_ledger.jsonl`;`mission_exit_code` 把 mission_state 映射成进程退出码,
//! 供外部调度器(systemd OnFailure / cron)告警。纯函数 + 文件 IO,不经 daemon。
//! 写账本失败仅记 stderr,绝不拖垮主流程。
//!
//! Task 4: MissionState 简化为 Running/Completed/EmptyGraph/Error(String),
//! 不再需要 blocked_at / StuckNeedsFix / CircuitBreaker / BlockedAt 等 gate 专用状态。
use crate::background::{BgOutcome, SubgoalOutcome};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// BG 任务终态（去掉 gate 专用状态）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MissionState {
    Running,
    Completed,
    EmptyGraph,
    Error(String),
}

/// 一条账本记录(JSONL 一行)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerRecord {
    /// Unix epoch 秒(UTC)。
    pub ts: u64,
    /// "workgraph" | "<explicit task>" | "no task"。
    pub task: String,
    pub mission_state: MissionState,
    pub subgoals: Vec<crate::background::SubgoalOutcome>,
    pub counts: LedgerCounts,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LedgerCounts {
    pub tools: usize,
    pub denied: usize,
    pub milestones: usize,
    pub passed: usize,
    pub failed: usize,
}

pub fn ledger_path(root: &Path) -> PathBuf {
    root.join("bg_ledger.jsonl")
}

/// mission_state → 进程退出码。保守:未知 → 0(不误报)。
pub fn mission_exit_code(state: &MissionState) -> i32 {
    match state {
        MissionState::Completed | MissionState::Running => 0,
        MissionState::EmptyGraph => 5,
        MissionState::Error(_) => 4,
    }
}

/// 从 BgOutcome 聚合 counts。
pub fn counts_of(outcome: &BgOutcome) -> LedgerCounts {
    let passed = outcome.subgoals.len();
    let failed = 0;
    LedgerCounts {
        tools: outcome.tool_calls.len(),
        denied: outcome.denied.len(),
        milestones: outcome.subgoals.len(),
        passed,
        failed,
    }
}

/// 从 BgOutcome 构造一条记录(ts 取当前 epoch 秒)。
pub fn record_of(outcome: &BgOutcome, task: &str) -> LedgerRecord {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    LedgerRecord {
        ts,
        task: task.to_string(),
        mission_state: outcome.mission_state.clone(),
        subgoals: outcome.subgoals.clone(),
        counts: counts_of(outcome),
    }
}

/// 追加一条记录到 `<root>/bg_ledger.jsonl`(每行一个 JSON)。IO 失败返 Err。
pub fn append(root: &Path, outcome: &BgOutcome, task: &str) -> anyhow::Result<()> {
    let rec = record_of(outcome, task);
    let line = serde_json::to_string(&rec)?;
    let path = ledger_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// 读最近 n 条(文件顺序的最后 n,旧→新)。only_failed=true 只回
/// mission_state≠Completed 的。损坏行跳过(JSONL 容错)。
pub fn read_recent(root: &Path, n: usize, only_failed: bool) -> Vec<LedgerRecord> {
    let path = ledger_path(root);
    let f = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };
    let all: Vec<LedgerRecord> = BufReader::new(f)
        .lines()
        .filter_map(|ln| {
            ln.ok()
                .and_then(|s| serde_json::from_str::<LedgerRecord>(&s).ok())
        })
        .collect();
    let filtered: Vec<LedgerRecord> = if only_failed {
        all.into_iter()
            .filter(|r| !matches!(r.mission_state, MissionState::Completed))
            .collect()
    } else {
        all
    };
    let start = filtered.len().saturating_sub(n);
    filtered[start..].to_vec()
}

/// epoch 秒 → "YYYY-MM-DDTHH:MM:SSZ"(UTC,无 chrono 依赖;civil-from-days 算法)。
pub fn format_utc(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86400) as i64;
    let secs_of_day = (epoch_secs % 86400) as u64;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Truncate the ledger to keep only the most recent `max_lines` lines.
/// Returns the number of lines removed, or 0 if no truncation needed.
/// When `max_lines` is 0, returns 0 (no-op).
pub fn truncate(root: &Path, max_lines: u32) -> anyhow::Result<usize> {
    if max_lines == 0 {
        return Ok(0);
    }
    let path = ledger_path(root);
    let all = std::fs::read_to_string(&path)?;
    let lines: Vec<&str> = all.lines().collect();
    if lines.len() <= max_lines as usize {
        return Ok(0);
    }
    let keep = lines[lines.len() - max_lines as usize..].join("\n");
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, &keep)?;
    std::fs::rename(&tmp, &path)?;
    Ok(lines.len() - max_lines as usize)
}

/// 单行摘要(供 `cc ledger` 默认输出)。
pub fn summarize_line(r: &LedgerRecord) -> String {
    let st = match &r.mission_state {
        MissionState::Error(msg) => format!("Error({msg})"),
        other => format!("{other:?}"),
    };
    format!(
        "{}  {}  {}m({}✓ {}✗)  {}t  {}d",
        format_utc(r.ts),
        st,
        r.counts.milestones,
        r.counts.passed,
        r.counts.failed,
        r.counts.tools,
        r.counts.denied
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::{BgOutcome, SubgoalOutcome};
    use tempfile::tempdir;

    fn outcome(state: MissionState) -> BgOutcome {
        let mut o = BgOutcome::default();
        o.mission_state = state;
        o.tool_calls = vec!["read_file".into(), "edit_file".into()];
        o.denied = vec!["run_command: x".into()];
        o.subgoals = vec![SubgoalOutcome {
            milestone_id: 1,
            tool_cap_hit: false,
            touched_files: vec!["a.rs".into()],
        }];
        o
    }

    #[test]
    fn exit_code_mapping() {
        assert_eq!(mission_exit_code(&MissionState::Completed), 0);
        assert_eq!(mission_exit_code(&MissionState::Running), 0);
        assert_eq!(mission_exit_code(&MissionState::Error("e".into())), 4);
    }

    #[test]
    fn empty_graph_exit_code_is_5() {
        assert_eq!(mission_exit_code(&MissionState::EmptyGraph), 5);
    }

    #[test]
    fn format_utc_is_human_readable() {
        // epoch 1784678507 ≈ 2026-07-22T08:41:47Z。
        let s = format_utc(1784678507);
        assert!(s.starts_with("2026-07-22"), "{s}");
        assert!(s.contains('T') && s.ends_with('Z'), "{s}");
    }

    #[test]
    fn append_then_read_roundtrip() {
        let dir = tempdir().unwrap();
        append(dir.path(), &outcome(MissionState::Error("x".into())), "workgraph").unwrap();
        let recs = read_recent(dir.path(), 10, false);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.task, "workgraph");
        assert!(matches!(r.mission_state, MissionState::Error(_)));
        assert_eq!(r.subgoals.len(), 1);
        assert_eq!(
            r.counts,
            LedgerCounts { tools: 2, denied: 1, milestones: 1, passed: 1, failed: 0 }
        );
    }

    #[test]
    fn read_recent_returns_last_n_in_order() {
        let dir = tempdir().unwrap();
        append(dir.path(), &outcome(MissionState::Running), "a").unwrap();
        append(dir.path(), &outcome(MissionState::Running), "b").unwrap();
        append(dir.path(), &outcome(MissionState::Running), "c").unwrap();
        let recs = read_recent(dir.path(), 2, false);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].task, "b"); // 旧→新
        assert_eq!(recs[1].task, "c");
    }

    #[test]
    fn read_recent_only_failed() {
        let dir = tempdir().unwrap();
        append(dir.path(), &outcome(MissionState::Completed), "ok").unwrap();
        append(dir.path(), &outcome(MissionState::Error("x".into())), "bad").unwrap();
        let recs = read_recent(dir.path(), 10, true);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].task, "bad");
    }

    #[test]
    fn read_recent_skips_malformed_lines() {
        let dir = tempdir().unwrap();
        std::fs::write(ledger_path(dir.path()), "{not json\n").unwrap();
        append(dir.path(), &outcome(MissionState::Running), "good").unwrap();
        let recs = read_recent(dir.path(), 10, false);
        assert_eq!(recs.len(), 1, "坏行应被跳过");
        assert_eq!(recs[0].task, "good");
    }

    #[test]
    fn truncate_noop_when_under_limit() {
        let dir = tempdir().unwrap();
        for i in 0..3 {
            let mut o = BgOutcome::default();
            o.mission_state = MissionState::Running;
            append(dir.path(), &o, &format!("t{i}")).unwrap();
        }
        let removed = truncate(dir.path(), 10).unwrap();
        assert_eq!(removed, 0);
        // 文件仍含 3 行
        let content = std::fs::read_to_string(ledger_path(dir.path())).unwrap();
        assert_eq!(content.lines().count(), 3);
    }

    #[test]
    fn truncate_keeps_last_n() {
        let dir = tempdir().unwrap();
        for i in 0..10 {
            let mut o = BgOutcome::default();
            o.mission_state = MissionState::Running;
            append(dir.path(), &o, &format!("t{i}")).unwrap();
        }
        let removed = truncate(dir.path(), 4).unwrap();
        assert_eq!(removed, 6);
        let content = std::fs::read_to_string(ledger_path(dir.path())).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 4);
        // 保留最后 4 条 (t6, t7, t8, t9)
        for (i, line) in lines.iter().enumerate() {
            let rec: LedgerRecord = serde_json::from_str(line).unwrap();
            assert_eq!(rec.task, format!("t{}", i + 6));
        }
    }

    #[test]
    fn truncate_zero_max_is_noop() {
        let dir = tempdir().unwrap();
        for i in 0..3 {
            let mut o = BgOutcome::default();
            o.mission_state = MissionState::Running;
            append(dir.path(), &o, &format!("t{i}")).unwrap();
        }
        let removed = truncate(dir.path(), 0).unwrap();
        assert_eq!(removed, 0);
        let content = std::fs::read_to_string(ledger_path(dir.path())).unwrap();
        assert_eq!(content.lines().count(), 3);
    }

    #[test]
    fn truncate_empty_file() {
        let dir = tempdir().unwrap();
        let path = ledger_path(dir.path());
        std::fs::write(&path, "").unwrap();
        let removed = truncate(dir.path(), 5).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn summarize_line_format() {
        let r = LedgerRecord {
            ts: 1784678507,
            task: "wg".into(),
            mission_state: MissionState::Error("x".into()),
            subgoals: vec![],
            counts: LedgerCounts { tools: 15, denied: 2, milestones: 3, passed: 1, failed: 2 },
        };
        let s = summarize_line(&r);
        assert!(
            s.contains("2026-07-22")
                && s.contains("Error(x)")
                && s.contains("3m(1✓ 2✗)")
                && s.contains("15t")
                && s.contains("2d"),
            "{s}"
        );
    }
}