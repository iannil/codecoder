# Persistent 服务跨重启韧性 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 持久化 `Supervisor` 状态(gave_up/crash_count/manifest mtime),让 daemon 重启后不盲目重 spawn 已失败服务、崩溃预算抑制 crash-loop、manifest 变更自动重置。会话内仍守 ADR 0021。

**Architecture:** 新模块 `src/supervisor_state.rs`(纯函数 + 文件 IO);`capability.rs::Supervisor` 接入(state + budget,`start_all(root, budget)`);`config.rs` 加 budget env;`daemon/mod.rs` 传 budget。只持久化判定状态,不存 live handles。

**Tech Stack:** Rust 2024;既有 `serde`/`capability::Supervisor`;测试用既有"立即退出脚本 + marker 文件"手法 + tempdir(hermetic,不依赖 Docker/网络/真 LLM)。

## Global Constraints

- **不改 ADR 0021**:会话内崩溃仍 1 次即 give_up、不自动重启;崩溃预算只管"重启后是否再 spawn"。
- **只持久化判定状态**(gave_up/crash_count/mtime);**不**持久化 `RunningServiceTable` 的 live handles(PID 跨重启无效)。
- **manifest 变更自动重置**(mtime 变 → 清计数);无 `cc` reset 命令。
- **state 文件缺失/损坏 → 空默认,不阻塞 daemon 启动**;save 失败仅警告。
- budget=0 = 永不永久跳过(每次重启都试);`should_skip` budget=0 时仅看 gave_up。
- **hermetic 测试**:不烧 LLM、不依赖 Docker/网络。
- **commit 规范**遵 `skills/commit-conventions.md`;提交到 `feat/supervisor-persistence` 分支。
- 真实仓 key 路径:`src/capability.rs`(Supervisor)、`src/daemon/mod.rs:42`(start_all 调用)、`src/config.rs`、`src/lib.rs`(mod 注册)。

## 关键既有签名(供各 Task 引用)

```rust
// src/capability.rs
pub struct SupervisedService { pub manifest: CapabilityManifest, pub child: Option<std::process::Child>, pub gave_up: bool }
pub struct Supervisor { pub root: std::path::PathBuf, pub states: std::collections::HashMap<String, SupervisedService> }
impl Supervisor {
    pub fn start_all(root: &Path) -> anyhow::Result<Self>;   // ← 签名将改为 (root, crash_budget)
    pub fn start_one(&mut self, name: &str, root: &Path) -> anyhow::Result<()>;
    pub fn supervise(&mut self) -> Vec<String>;                // 崩溃分支将加 record_crash + save
    pub fn shutdown_all(&mut self);
}
// src/daemon/mod.rs:42  Supervisor::start_all(&self.cfg.root)
// src/capability.rs 既有测试 :232  Supervisor::start_all(&dir)   ← 改为 (&dir, 3)
```

---

## Task 1: config.rs 加 `supervisor_crash_budget` env

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Produces: `Config { supervisor_crash_budget: u32 }`(默认 3)。

- [ ] **Step 1: 写失败测试**(append 到 config.rs `#[cfg(test)] mod tests`,与既有 `bg_env_defaults_and_overrides` 并列)
```rust
    #[test]
    fn supervisor_crash_budget_default_and_override() {
        unsafe { std::env::remove_var("CODECODER_SUPERVISOR_CRASH_BUDGET"); }
        assert_eq!(Config::from_env().supervisor_crash_budget, 3);
        unsafe { std::env::set_var("CODECODER_SUPERVISOR_CRASH_BUDGET", "5"); }
        assert_eq!(Config::from_env().supervisor_crash_budget, 5);
        unsafe { std::env::remove_var("CODECODER_SUPERVISOR_CRASH_BUDGET"); }
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib config::tests::supervisor_crash_budget 2>&1 | tail -4`
Expected: 编译失败(`no field supervisor_crash_budget`)。

- [ ] **Step 3: 实现**

`Config` struct 加字段:
```rust
    pub supervisor_crash_budget: u32,
```
`from_env()` 末尾加:
```rust
            supervisor_crash_budget: env("CODECODER_SUPERVISOR_CRASH_BUDGET")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
```
同步既有 `Config { ... }` 字面量构造处(如 `src/daemon/mod.rs` 测试里的 `Config { ... }`)——补 `supervisor_crash_budget: 3`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib config::tests::supervisor_crash_budget 2>&1 | tail -3`
Expected: `1 passed`。

- [ ] **Step 5: 提交**

```bash
git add src/config.rs src/daemon/mod.rs
git commit -m "feat(config): 加 CODECODER_SUPERVISOR_CRASH_BUDGET(默认 3)

为跨重启崩溃预算铺垫(ADR 0034 草案);2024 edition set/remove_var 包 unsafe。"
```

---

## Task 2: `supervisor_state.rs` — 类型 + load/save + 纯函数(T1-T4)

**Files:**
- Create: `src/supervisor_state.rs`
- Modify: `src/lib.rs`(加 `pub mod supervisor_state;`)

**Interfaces:**
- Produces: `SupervisorState`、`ServiceEntry`、`state_path`、`load`、`save`、`mtime_of`、`reset_if_manifest_changed`、`should_skip`、`record_crash`。

- [ ] **Step 1: 写失败测试**(supervisor_state.rs 内 `#[cfg(test)]`)
```rust
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
        s.services.insert("flaky".into(), ServiceEntry { gave_up: true, crash_count: 3, manifest_mtime_secs: 123 });
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
        s.services.insert("x".into(), ServiceEntry { gave_up: true, crash_count: 3, manifest_mtime_secs: 100 });
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
        // budget=0:永不永久跳过(crash_count≥0 不参与),仅看 gave_up
        assert!(!should_skip(&s, "c", 0), "budget=0 时即使 crash_count 高也不 skip");
        assert!(should_skip(&s, "g", 0), "budget=0 时 gave_up 仍 skip");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib supervisor_state 2>&1 | tail -5`
Expected: 编译失败(模块/类型未定义)。

- [ ] **Step 3: 实现**

创建 `src/supervisor_state.rs`:
```rust
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

/// 写状态(atomic: 先写 tmp 再 rename)。失败返 Err(调用方记警告)。
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
/// 服务无记录时:视为首次(写入 mtime,返回 false,不清零——本就为空)。
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
```
`src/lib.rs` 加 `pub mod supervisor_state;`(紧随 `pub mod supervisor_state` 无——放在 `pub mod registry;` 附近)。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib supervisor_state 2>&1 | tail -4`
Expected: 全部 `passed`(6 个)。

- [ ] **Step 5: 提交**

```bash
git add src/supervisor_state.rs src/lib.rs
git commit -m "feat(supervisor_state): 持久化监督状态模块(纯函数)

SupervisorState/ServiceEntry + load(缺失/损坏→默认)/save(atomic rename)
+ mtime_of + reset_if_manifest_changed + should_skip(budget=0 仅看 gave_up)
+ record_crash。6 hermetic 单测。会话内 ADR 0021 不变。"
```

---

## Task 3: `capability.rs::Supervisor` 接入 state + budget(T5/T6/T8)

**Files:**
- Modify: `src/capability.rs`(`Supervisor` struct + `start_all` + `supervise` + 既有测试签名)

**Interfaces:**
- Consumes: `supervisor_state::{load, save, mtime_of, reset_if_manifest_changed, should_skip, record_crash, SupervisorState}`。
- Produces: `Supervisor::start_all(root, crash_budget) -> Result<Self>`(新签名);`Supervisor` 增 `state`/`crash_budget` 字段。

- [ ] **Step 1: 改 `Supervisor` struct + `start_all` 签名**

`src/capability.rs` 的 `Supervisor` 改为:
```rust
pub struct Supervisor {
    pub root: std::path::PathBuf,
    pub states: std::collections::HashMap<String, SupervisedService>,
    pub state: crate::supervisor_state::SupervisorState,
    pub crash_budget: u32,
}
```
`start_all` 改为(读 state → 逐服务 reset/skip/spawn → save):
```rust
pub fn start_all(root: &std::path::Path, crash_budget: u32) -> anyhow::Result<Self> {
    use crate::supervisor_state::{self, SupervisorState};
    let mut sup = Self {
        root: root.to_path_buf(),
        states: Default::default(),
        state: supervisor_state::load(root),
        crash_budget,
    };
    let caps = root.join("capabilities");
    let Ok(entries) = std::fs::read_dir(&caps) else { return Ok(sup); };
    for e in entries.flatten() {
        let man = e.path().join("manifest.json");
        let Ok(raw) = std::fs::read_to_string(&man) else { continue; };
        let Ok(m) = serde_json::from_str::<CapabilityManifest>(&raw) else { continue; };
        if !(m.lifecycle == Lifecycle::Persistent && m.environment == Environment::Shell) {
            continue;
        }
        let cur_mtime = supervisor_state::mtime_of(&man);
        supervisor_state::reset_if_manifest_changed(&mut sup.state, &m.name, cur_mtime);
        if supervisor_state::should_skip(&sup.state, &m.name, crash_budget) {
            let cnt = sup.state.services.get(&m.name).map(|e| e.crash_count).unwrap_or(0);
            eprintln!(
                "capability '{}' skipped: previously Failed (crash_count={}, budget={})",
                m.name, cnt, crash_budget
            );
            continue;
        }
        let _ = sup.start_one(&m.name, root);
    }
    let _ = supervisor_state::save(root, &sup.state);
    Ok(sup)
}
```
注:既有 `start_one`/`supervise`/`shutdown_all` 不改签名;`supervise` 在崩溃分支加 record_crash + save(Step 2)。

- [ ] **Step 2: `supervise` 记崩溃 + 持久化**

`supervise` 的崩溃分支(原 `s.gave_up = true; s.child = None;` 处)改为:
```rust
            if !exited { continue; }
            // ADR 0021:会话内崩溃即 give_up、不自动重启。
            s.gave_up = true;
            s.child = None;
            crate::supervisor_state::record_crash(&mut self.state, name, self.crash_budget);
            let _ = crate::supervisor_state::save(&self.root, &self.state);
            let cnt = self.state.services.get(name).map(|e| e.crash_count).unwrap_or(0);
            events.push(format!(
                "capability '{name}' exited; marked Failed (crash_count={cnt}, budget={}, not auto-restarted, ADR 0021)",
                self.crash_budget
            ));
```

- [ ] **Step 3: 同步既有测试签名(T8)+ 新增集成测试(T5/T6)**

既有测试 `supervisor_marks_crashed_persistent_failed_without_restart`(capability.rs tests):把 `Supervisor::start_all(&dir)` 改为 `Supervisor::start_all(&dir, 3)`;事件断言 `events.iter().any(|e| e.contains("marked Failed") && e.contains("not auto-restarted"))` 仍成立(新事件文案保留两关键词)。

新增(capability.rs tests,沿用同 tempdir + marker 文件手法):
```rust
    #[test]
    fn start_all_skips_persistently_failed_service() {
        use crate::supervisor_state::{save, ServiceEntry, SupervisorState};
        let dir = std::env::temp_dir().join(format!("cc_sup_skip_{}_{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
        let capdir = dir.join("capabilities/flaky");
        std::fs::create_dir_all(&capdir).unwrap();
        let marker = dir.join("skip_marker.txt");
        std::fs::write(dir.join("capabilities/flaky/entry.sh"),
            format!("#!/bin/sh\necho ran >> \"{}\"\nexit 1\n", marker.display())).unwrap();
        std::fs::write(capdir.join("manifest.json"),
            r#"{"name":"flaky","description":"d","environment":"shell","lifecycle":"persistent","entry":"sh entry.sh"}"#).unwrap();
        // 预写:gave_up=true(跨重启永久跳过)。
        let mut st = SupervisorState::default();
        st.services.insert("flaky".into(), ServiceEntry { gave_up: true, crash_count: 3, manifest_mtime_secs: 0 });
        save(&dir, &st).unwrap();
        let sup = Supervisor::start_all(&dir, 3).unwrap();
        // should NOT have spawned → marker 不存在(无 "ran" 行)。
        assert!(!marker.exists(), "gave_up 服务不应被 spawn");
        assert!(sup.states.get("flaky").is_none(), "states 不应含被跳过的服务");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_all_respawns_when_manifest_changed() {
        use crate::supervisor_state::{save, ServiceEntry, SupervisorState};
        let dir = std::env::temp_dir().join(format!("cc_sup_reset_{}_{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
        let capdir = dir.join("capabilities/flaky");
        std::fs::create_dir_all(&capdir).unwrap();
        let marker = dir.join("reset_marker.txt");
        std::fs::write(dir.join("capabilities/flaky/entry.sh"),
            format!("#!/bin/sh\necho ran >> \"{}\"\nexit 0\n", marker.display())).unwrap();
        let man = capdir.join("manifest.json");
        std::fs::write(&man,
            r#"{"name":"flaky","description":"d","environment":"shell","lifecycle":"persistent","entry":"sh entry.sh"}"#).unwrap();
        // 预写:gave_up=true + 旧 mtime(0);真实 manifest 的 mtime ≠ 0 → 触发 reset → 重 spawn。
        let mut st = SupervisorState::default();
        st.services.insert("flaky".into(), ServiceEntry { gave_up: true, crash_count: 3, manifest_mtime_secs: 0 });
        save(&dir, &st).unwrap();
        let _sup = Supervisor::start_all(&dir, 3).unwrap();
        // 应已重 spawn → marker 存在。
        assert!(marker.exists(), "manifest 变更后应重置并 spawn");
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 4: 跑测试确认通过 + 不回归**

Run: `cargo test --lib capability:: 2>&1 | tail -6`
Expected: 全 `passed`(既有 1 + 新增 2;新事件文案保留 "marked Failed"/"not auto-restarted")。

- [ ] **Step 5: 提交**

```bash
git add src/capability.rs
git commit -m "feat(capability): Supervisor 接入持久化状态 + 崩溃预算

start_all(root, budget):load → manifest 变更 reset → 超预算/gave_up 跳过
(不 spawn)→ save;supervise 崩溃分支 record_crash + save + 事件附
crash_count/budget。会话内仍守 ADR 0021(崩了不自动重启)。既有测试签名
同步;+2 集成测试(跳过 gave_up / manifest 变更重 spawn)。"
```

---

## Task 4: daemon 传 budget + 验证全链路

**Files:**
- Modify: `src/daemon/mod.rs:42`(传 `cfg.supervisor_crash_budget`)

- [ ] **Step 1: 改 daemon 调用**

`src/daemon/mod.rs:42` 附近:
```rust
        let mut supervisor = crate::capability::Supervisor::start_all(
            &self.cfg.root, self.cfg.supervisor_crash_budget,
        )
```

- [ ] **Step 2: 编译 + 全量不回归**

Run: `cargo build 2>&1 | tail -2 && cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4;f+=$6;ig+=$8} END{print "TOTAL: "p" passed, "f" failed, "ig" ignored"}'`
Expected: `Finished` + 全通过(原 273 + 新增 config 1 + supervisor_state 6 + capability 2 = +9 → 282)。

- [ ] **Step 3: 提交**

```bash
git add src/daemon/mod.rs
git commit -m "feat(daemon): start_all 传 supervisor_crash_budget

daemon 启动把 Config.supervisor_crash_budget 传给 Supervisor::start_all,
接通跨重启崩溃预算。"
```

---

## Task 5: 文档同步 + CLAUDE.md 已知清单更新

**Files:**
- Create: `docs/adr/0034-persistent-supervisor-cross-restart.md`
- Modify: `ARCHITECTURE.md`、`README.md`、`CLAUDE.md`

- [ ] **Step 1: 写 ADR 0034**

Context=审计核验 Persistent 无跨重启注册表属实 + 既有 Supervisor 盲目重 spawn 风险;Decision=持久化 supervisor_state.json(gave_up/crash_count/mtime)+ 崩溃预算跨重启抑制 crash-loop + manifest 变更自动重置;**补 ADR 0021 的跨重启缺口,不推翻之(会话内仍不自动重启)**;Status=Accepted;Consequences=仅持久化判定状态不存 live handles、state 损坏回退默认不阻塞、budget=0 永不永久跳过。

- [ ] **Step 2: 同步 ARCHITECTURE/README/CLAUDE.md**

ARCHITECTURE 模块图加 `supervisor_state.rs` 行、capability.rs 行补"跨重启持久化"、文件系统图加 `supervisor_state.json`、ADR 索引加 `0034`;README env 表加 `CODECODER_SUPERVISOR_CRASH_BUDGET`;**CLAUDE.md「已知未实现」清单把 Persistent 跨重启一项从「未实现」改为「已实现(ADR 0034),会话内仍守 ADR 0021」**,并刷新相关计数/描述。

- [ ] **Step 3: 提交**

```bash
git add docs/adr/0034-persistent-supervisor-cross-restart.md ARCHITECTURE.md README.md CLAUDE.md
git commit -m "docs: ADR 0034 Persistent 跨重启韧性 + 同步文档 + CLAUDE.md 已知清单更新

CLAUDE.md「已知未实现」中 Persistent 跨重启一项标记为已实现(ADR 0034);
会话内仍守 ADR 0021。"
```

---

## Self-Review(plan vs spec)

**1. Spec coverage:**
- 持久化 gave_up/crash_count/mtime → Task 2(supervisor_state)+ Task 3(接入)。
- start_all 跳过超预算/gave_up → Task 3 Step 1(should_skip 分支)。
- 崩溃预算(N=3,env)→ Task 1(config)+ Task 3(record_crash)。
- manifest 变更自动重置 → Task 2(reset_if_manifest_changed)+ Task 3(start_all 调用)+ Task 3 T6 集成。
- 会话内仍守 ADR 0021 → Task 3 Step 2(supervise 不自动重启,仅 record_crash)。
- budget=0 语义 → Task 2 should_skip 测试 + 实现。
- state 损坏回退默认 → Task 2 load + T1。
- save 失败仅警告 → Task 3 `let _ = save(...)`。
- daemon 接通 → Task 4。
- 文档 + CLAUDE.md 已知清单更新 → Task 5。

**2. Placeholder scan:** 无 TBD/适当错误处理;Task 3 Step 1/Step 2 给出完整 start_all/supervise 改动代码;T5/T6 给出完整测试(marker 文件手法照搬既有测试)。无占位。

**3. Type consistency:** `SupervisorState`/`ServiceEntry`/`state_path`/`load`/`save`/`mtime_of`/`reset_if_manifest_changed`/`should_skip`/`record_crash` 在 Task 2 定义、Task 3 调用一致;`start_all(root, crash_budget)` 签名 Task 3 定义、Task 4 调用一致;`Config.supervisor_crash_budget` Task 1 定义、Task 4 使用一致。
