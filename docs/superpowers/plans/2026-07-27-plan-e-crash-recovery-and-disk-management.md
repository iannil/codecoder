# Plan E: Daemon Crash Auto-Recovery & Disk Space Management

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable CodeCoder daemon to survive crashes and run indefinitely without disk overflow. Two independent sub-systems:
1. **Crash auto-recovery**: daemon crash → auto-restart → load last session → resume workgraph → continue
2. **Disk space management**: session count limit, ledger rotation, memory TTL

**Architecture:** 
- **Recovery**: Add a `CODECODER_DAEMON_AUTO_RESTART` env var (default false). When true, the daemon writes a "heartbeat" stamp file on startup and on each workgraph tick. A new `main.rs` entry mode wraps `run_daemon` in a crash loop: on non-zero exit, re-read the stamp file, `resume` the latest session, and restart the daemon.
- **Disk management**: Add a daemon thread that runs every N minutes, checks session count, deletes oldest sessions beyond a configurable limit, and truncates `bg_ledger.jsonl`. Add `CODECODER_MAX_SESSIONS` (default 100), `CODECODER_MAX_LEDGER_LINES` (default 10000).

**Tech Stack:** Rust (existing), no new dependencies

## Global Constraints

- Recovery is opt-in (`CODECODER_DAEMON_AUTO_RESTART=1`) — never change default behavior
- Session deletion is soft: only delete sessions older than 7 days when over limit
- `CODECODER_ALERT_WEBHOOK` is used to notify on crash+recovery events
- All new env vars documented in README.md
- No changes to session format or workgraph format — only management around them

---

### Task 1: Add auto-restart env var and config

**Files:**
- Modify: `src/config.rs` (add `daemon_auto_restart: bool`, `max_sessions: u32`, `max_ledger_lines: u32`)
- Test: `src/config.rs` inline tests

**Interfaces:**
- `Config.daemon_auto_restart: bool` (default false, env `CODECODER_DAEMON_AUTO_RESTART`)
- `Config.max_sessions: u32` (default 100, env `CODECODER_MAX_SESSIONS`)
- `Config.max_ledger_lines: u32` (default 10000, env `CODECODER_MAX_LEDGER_LINES`)

- [ ] **Step 1: Add fields to Config struct**

```rust
/// 是否在 daemon 崩溃后自动重启并恢复 session。env CODECODER_DAEMON_AUTO_RESTART, 默认 false。
pub daemon_auto_restart: bool,
/// sessions/ 目录最大文件数，超限时删除最旧的。0 = 不限制。env CODECODER_MAX_SESSIONS, 默认 100。
pub max_sessions: u32,
/// bg_ledger.jsonl 最大行数，超限时截断。0 = 不限制。env CODECODER_MAX_LEDGER_LINES, 默认 10000。
pub max_ledger_lines: u32,
```

- [ ] **Step 2: Add env parsing in `from_env()`**

```rust
daemon_auto_restart: env("CODECODER_DAEMON_AUTO_RESTART")
    .map(|v| v == "1" || v == "true")
    .unwrap_or(false),
max_sessions: env("CODECODER_MAX_SESSIONS")
    .and_then(|v| v.parse().ok())
    .unwrap_or(100),
max_ledger_lines: env("CODECODER_MAX_LEDGER_LINES")
    .and_then(|v| v.parse().ok())
    .unwrap_or(10000),
```

- [ ] **Step 3: Add to DOTENV_ALLOWED_KEYS**

```rust
"CODECODER_DAEMON_AUTO_RESTART",
"CODECODER_MAX_SESSIONS",
"CODECODER_MAX_LEDGER_LINES",
```

- [ ] **Step 4: Write tests**

- [ ] **Step 5: Update daemon/mod.rs Config literal in tests**

- [ ] **Step 6: Commit**

---

### Task 2: Implement crash auto-recovery loop

**Files:**
- Create: `src/recovery.rs` (stamp file + recovery logic)
- Modify: `src/main.rs` (add crash loop wrapper)
- Modify: `src/lib.rs` (add `pub mod recovery;`)

**Interfaces:**
- `recovery::write_stamp(root: &Path) -> Result<()>` — writes a stamp file with current timestamp
- `recovery::read_stamp(root: &Path) -> Result<Option<(u64, String)>>` — reads stamp (timestamp, session_id)
- `recovery::run_with_recovery(cfg: Config) -> Result<()>` — loop: run daemon → crash → resume → retry
- Stamp file at `CODECODER_ROOT/.ccd_stamp.json`:
  ```json
  {"last_tick": 1234567890, "session_id": "s0000", "workgraph_mtime": 1234567890}
  ```

- [ ] **Step 1: Create `src/recovery.rs`**

```rust
// src/recovery.rs
// Daemon crash auto-recovery: stamp file + restart loop.

use std::path::{Path, PathBuf};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStamp {
    pub last_tick: u64,
    pub session_id: Option<String>,
    pub workgraph_mtime: Option<u64>,
}

pub fn stamp_path(root: &Path) -> PathBuf {
    root.join(".ccd_stamp.json")
}

pub fn write_stamp(root: &Path, stamp: &DaemonStamp) -> anyhow::Result<()> {
    let path = stamp_path(root);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(stamp)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read_stamp(root: &Path) -> Option<DaemonStamp> {
    let path = stamp_path(root);
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}
```

- [ ] **Step 2: Wire stamp writes into daemon workgraph thread**

In `src/daemon/mod.rs`, after each workgraph tick, update the stamp file with current timestamp, session_id, and workgraph mtime.

- [ ] **Step 3: Add crash loop in `main.rs`**

```rust
// In main.rs, when daemon_auto_restart is true, wrap run_daemon in a loop:
fn main() -> anyhow::Result<()> {
    config::autoload_ccd_env();
    let cfg = Config::from_env();
    
    match bg_mode_from_env() {
        Some(BgMode::Explicit(task)) => run_background(cfg, task),
        Some(BgMode::Workgraph) => run_background(cfg, String::new()),
        None => {
            if cfg.daemon_auto_restart {
                recovery::run_with_recovery(cfg)
            } else {
                run_daemon(cfg)
            }
        }
    }
}
```

- [ ] **Step 4: Implement `run_with_recovery`**

```rust
pub fn run_with_recovery(cfg: Config) -> anyhow::Result<()> {
    let max_restarts = 5; // prevent infinite crash loop
    for attempt in 0..max_restarts {
        let result = crate::run_daemon(cfg.clone());
        match result {
            Ok(()) => return Ok(()), // graceful shutdown
            Err(e) => {
                eprintln!("[recovery] daemon crashed (attempt {}/{}): {e}", attempt + 1, max_restarts);
                // Send alert if configured
                if let Some(ref webhook) = cfg.alert_webhook {
                    let msg = format!("🔄 CodeCoder daemon recovered (crash #{})\nError: {e}", attempt + 1);
                    let _ = crate::alert::send_alert(webhook, &msg);
                }
                // Small delay before restart
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        }
    }
    anyhow::bail!("daemon crashed {} times, giving up", max_restarts);
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test
```

- [ ] **Step 6: Commit**

---

### Task 3: Implement disk space management thread

**Files:**
- Modify: `src/daemon/mod.rs` (add cleanup thread)
- Modify: `src/bg_ledger.rs` (add truncation function)
- Test: inline tests

**Interfaces:**
- `bg_ledger::truncate(root: &Path, max_lines: u32) -> Result<usize>` — keeps only the most recent `max_lines` from ledger
- `session::cleanup_old_sessions(root: &Path, max_sessions: u32) -> Result<usize>` — deletes oldest sessions beyond limit
- Daemon thread runs every 5 minutes, calls both cleanup functions

- [ ] **Step 1: Add `truncate` to `bg_ledger.rs`**

```rust
/// Truncate the ledger to keep only the most recent `max_lines` lines.
/// Returns the number of lines removed, or 0 if no truncation needed.
/// When `max_lines` is 0, returns 0 (no-op).
pub fn truncate(root: &Path, max_lines: u32) -> anyhow::Result<usize> {
    if max_lines == 0 { return Ok(0); }
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
```

- [ ] **Step 2: Add `cleanup_old_sessions` to `session.rs`**

```rust
/// Delete oldest session files when the count exceeds `max_sessions`.
/// Only deletes sessions older than 7 days. Returns number deleted.
pub fn cleanup_old_sessions(root: &Path, max_sessions: u32) -> anyhow::Result<usize> {
    if max_sessions == 0 { return Ok(0); }
    let mgr = SessionManager::new(root);
    let mut all = mgr.list();
    if all.len() <= max_sessions as usize {
        return Ok(0);
    }
    let seven_days_ago = std::time::SystemTime::now()
        - std::time::Duration::from_secs(7 * 24 * 3600);
    all.sort_by(|a, b| a.mtime.cmp(&b.mtime)); // oldest first
    let to_delete: Vec<_> = all.iter()
        .filter(|m| m.mtime < seven_days_ago)
        .take(all.len() - max_sessions as usize)
        .collect();
    for meta in &to_delete {
        let path = sessions_dir(root).join(format!("{}.json", meta.id));
        let _ = std::fs::remove_file(&path);
    }
    Ok(to_delete.len())
}
```

- [ ] **Step 3: Add cleanup thread to daemon**

In `src/daemon/mod.rs`, add a new thread that runs every 300 seconds (5 min), calls `bg_ledger::truncate()` and `session::cleanup_old_sessions()`.

- [ ] **Step 4: Run tests**

```bash
cargo test
```

- [ ] **Step 5: Commit**

---

### Task 4: Update docs

**Files:**
- Modify: `README.md` (add new env vars)
- Modify: `ARCHITECTURE.md` (update module map)

- [ ] **Step 1: Update README.md env table**

Add `CODECODER_DAEMON_AUTO_RESTART`, `CODECODER_MAX_SESSIONS`, `CODECODER_MAX_LEDGER_LINES`.

- [ ] **Step 2: Commit**

```bash
git add README.md ARCHITECTURE.md
git commit -m "docs: document daemon auto-recovery and disk management"
```