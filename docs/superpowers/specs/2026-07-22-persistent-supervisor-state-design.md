# Persistent 服务跨重启韧性 — 设计文档

- **日期**: 2026-07-22
- **状态**: 已批准(Approved)
- **分支**: `feat/supervisor-persistence`
- **作者**: Claude Code(brainstorming 产物)
- **关联**: roadmap #3、ADR 0021(Capability 环境与生命周期——不自动重启)、ADR 0022(自撰安全回路)、`docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`、`src/capability.rs`、`src/daemon/mod.rs`

## 1. 目标

让 Persistent Capability 服务**跨 daemon 重启有记忆**:重启后**不盲目重 spawn 已失败的服务**;用**崩溃预算**跨重启抑制 crash-loop;**manifest 变更自动重置**某服务的失败计数(贴合自我进化)。会话内仍遵守 ADR 0021(崩溃即 give_up、不自动重启)。

**成功标准**:
- `Supervisor` 状态(gave_up + crash_count + manifest mtime)持久化到 `supervisor_state.json`,daemon 重启后恢复。
- 超过崩溃预算(N,默认 3)的服务在 start_all 被**跳过**(不 spawn),事件可见。
- manifest.json mtime 变化 → 该服务 gave_up/crash_count **自动重置**(agent 重新生成 capability → 重试)。
- 全部逻辑 hermetic 可测(沿用既有 supervisor 测试手法)。
- 不改 ADR 0021(会话内仍不自动重启)。

## 2. 背景(审计实证)

`2026-07-21` 审计核验「Persistent 无跨重启注册表」属实:`Supervisor.states`(gave_up/Failed)与 `RunningServiceTable`(live handles)均**进程内内存**,daemon 重启即丢。`Supervisor::start_all`(capability.rs:132)重启时**重新扫描 capabilities/ 并盲目重 spawn 全部声明服务**——包括上次已 `gave_up` 的 → 若该服务仍崩溃,每次重启都 crash-loop;且"Failed, agent 决定"的信号丢失。

`RunningServiceTable` 的 live handles(PID/容器)跨重启无意义(PID 失效),**不持久化**;需持久化的是 `Supervisor` 的**判定状态**。

## 3. 已锁定决策(brainstorming)

| 维度 | 决策 |
|---|---|
| 重启策略 | **持久记忆 + 崩溃预算**(不改 ADR 0021;不加会话内自动重启/退避) |
| reset 触发 | **manifest 变更自动重置**(无 `cc` reset 命令;YAGNI) |
| 持久化范围 | 只存 Supervisor 状态(gave_up/crash_count/mtime);不存 live handles |

## 4. 两层 "gave_up" 的精确区分(关键不变量)

| 层 | 语义 | 行为 |
|---|---|---|
| **会话内 gave_up**(既有,ADR 0021) | 本次 daemon 生命周期内崩过 → 不再重启 | 不变:1 次崩溃 → gave_up=true(本会话) |
| **跨重启永久跳过**(新) | 历史崩溃累计 ≥ 预算 → 重启后不再 spawn | start_all 跳过,直到 manifest 变更重置 |

崩溃预算管的是**「重启后要不要再 spawn」**;**不改**会话内「崩了不重启」(ADR 0021)。两者不冲突。

## 5. 架构

```
daemon 启动 → Supervisor::start_all(root)
  → supervisor_state::load(root)                         [NEW]
  → 逐声明 Persistent+Shell 服务:
      reset_if_manifest_changed(name, cur_mtime)          [NEW:mtime 变→清零]
      若 should_skip(name)(gave_up 或 crash_count≥budget) [NEW]
        → 跳过 spawn + emit "skipped: previously Failed (N crashes, budget B)"
      否则 spawn + 记录 cur_mtime
daemon 运行 → 周期 supervise()
  → 检测崩溃 → record_crash(name)(crash_count++; ≥budget→gave_up=true) [NEW]
  → supervisor_state::save(root)                          [NEW]
  → emit 既有 "marked Failed"(附 crash_count)
daemon 重启 → load → 超预算跳过 / manifest 变更重置后重 spawn
agent generate_capability 改 manifest.json → mtime 变 → 下次 start_all 重置重试
```

新模块 `src/supervisor_state.rs`(纯函数 + 文件 IO,与 `bg_ledger.rs` 同风格);`capability.rs::Supervisor` 接入;`config.rs` 加 budget env。

## 6. 组件

### 6.1 `src/supervisor_state.rs`

```rust
#[derive(Default, Serialize, Deserialize)]
pub struct SupervisorState {
    pub schema_version: u32,
    pub services: std::collections::HashMap<String, ServiceEntry>,
}

#[derive(Default, Serialize, Deserialize, Clone)]
pub struct ServiceEntry {
    pub gave_up: bool,
    pub crash_count: u32,
    /// 上次记录的 manifest.json mtime(epoch 秒;不可用时 0)。
    pub manifest_mtime_secs: u64,
}

pub fn state_path(root: &Path) -> PathBuf { root.join("supervisor_state.json") }
pub fn load(root: &Path) -> SupervisorState;                      // 缺失/损坏→默认
pub fn save(root: &Path, &SupervisorState) -> anyhow::Result<()>;

/// manifest mtime 变化 → 清 gave_up/crash_count;并刷新 mtime。返回是否发生了 reset。
pub fn reset_if_manifest_changed(state: &mut SupervisorState, name: &str, cur_mtime: u64) -> bool;

/// 该服务是否应被 start_all 跳过(gave_up 或 crash_count ≥ budget)。
pub fn should_skip(state: &SupervisorState, name: &str, budget: u32) -> bool;

/// 记录一次崩溃:crash_count++;≥budget → gave_up=true。
pub fn record_crash(state: &mut SupervisorState, name: &str, budget: u32);
```

### 6.2 `capability.rs::Supervisor` 接入

- `Supervisor` 增字段 `state: SupervisorState` 与 `crash_budget: u32`。
- `start_all(root, crash_budget)`:
  - `let mut state = supervisor_state::load(root);`
  - 逐声明 Persistent+Shell 服务:`cur = mtime_of(manifest)`;`reset_if_manifest_changed(&mut state, name, cur)`;若 `should_skip(&state, name, budget)` → emit skip 事件、**不 spawn**;否则 `start_one` + 在 state 里记 `manifest_mtime_secs=cur`。
  - 末尾 `save(root, &state)`。
- `supervise()`:崩溃分支 → `record_crash(&mut self.state, name, self.crash_budget)`;`supervisor_state::save(&self.root, &self.state)`;事件附 `crash_count=N budget=B`。
- `mtime_of(path) -> u64`:`std::fs::metadata(path).modified().ok()` → epoch 秒;失败→0。

### 6.3 `config.rs`

加 `pub supervisor_crash_budget: u32`,env `CODECODER_SUPERVISOR_CRASH_BUDGET`(默认 3)。`daemon/mod.rs:42` `Supervisor::start_all(&root, cfg.supervisor_crash_budget)`。

## 7. 错误处理

- state 文件缺失/损坏 → `load` 返回默认空状态(**不阻塞 daemon 启动**)。
- `save` 失败 → stderr 警告,继续(内存状态有效,跨重启记忆丢一次)。
- `manifest.modified()` 不可用(某些 FS)→ mtime=0(首次记录即视为"已记录",不触发误 reset)。
- budget=0 → `should_skip` 中 `crash_count ≥ 0` 恒真 → 永远跳过?**否**:budget=0 语义为"永不永久跳过",实现上 `should_skip` 仅看 `gave_up`(budget=0 时 `crash_count ≥ budget` 不参与判定)。文档明示 budget=0 = 每次重启都试。
- 跨重启 PID 失效:**不持久化 live handles**(RunningServiceTable 不变);只持久化判定状态。

## 8. 测试(hermetic)

沿用 `capability.rs` 既有测试的"立即退出脚本 + marker 文件"手法。

- **T1 load/save 往返**:save 一条 → load 回,字段匹配;缺失文件 → 默认空。
- **T2 record_crash + budget**:crash_count 累计;达 budget → gave_up=true;未达 → gave_up=false。
- **T3 reset_if_manifest_changed**:mtime 变 → 清零(返回 true);不变 → 保留(返回 false)。
- **T4 should_skip**:gave_up=true → skip;crash_count≥budget(且 budget>0)→ skip;否则不 skip;budget=0 → 仅看 gave_up。
- **T5 集成 跳过**:`start_all` 在预写 state(gave_up=true)下 **不 spawn** 该服务(marker 文件无新行);emit skip 事件。
- **T6 集成 manifest 重置**:预写 gave_up=true + 旧 mtime;更新 manifest mtime → `start_all` 重置并 **重新 spawn**(marker 有新行)。
- **T7 budget env**:`CODECODER_SUPERVISOR_CRASH_BUDGET` 读取 + 默认 3(与既有 config 测试同手法;注意 2024 edition set_var 包 unsafe)。
- **T8 不回归 + 签名同步**:既有 `supervisor_marks_crashed_persistent_failed_without_restart` 的 `Supervisor::start_all(&dir)` 调用改为 `start_all(&dir, 3)`(新签名),断言不变(会话内 1 崩 → gave_up;新逻辑不改此)。
- 纯函数 + tempdir;不依赖 Docker/网络/真 LLM。

## 9. 不做(YAGNI)

- ❌ 会话内自动重启 / 退避(bounded auto-restart)——改 ADR 0021 路径,未选。
- ❌ `cc services --reset` 命令——manifest 变更已覆盖 reset。
- ❌ 持久化 RunningServiceTable 的 live handles(PID 跨重启无效)。
- ❌ 跨 root / 跨主机协调。
- ❌ TTL 过期重置——manifest 变更即重置。
- ❌ 崩溃原因/日志持久化(crash 原因在 reason/causal 层,非本项)。

## 10. 交付物

1. `src/supervisor_state.rs`:`SupervisorState`/`ServiceEntry` + load/save/reset_if_manifest_changed/should_skip/record_crash/state_path/mtime_of。
2. `src/capability.rs`:`Supervisor` 接入 state + budget;`start_all` 签名加 `crash_budget`;`supervise` 记崩溃 + save。
3. `src/config.rs`:`supervisor_crash_budget` env。
4. `src/daemon/mod.rs:42`:`start_all(&root, cfg.supervisor_crash_budget)`。
5. 测试 T1-T8。
6. 文档:ADR 0034 + ARCHITECTURE(capability.rs 行 + supervisor_state.json + 索引)+ README(env 表)+ CLAUDE.md「已知未实现」更新(Persistent 跨重启 → 已实现 ADR 0034)。
