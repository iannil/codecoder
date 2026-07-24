# 设计 · 迭代 4：no-op 探索兜底 + footgun 清零

- **日期**: 2026-07-24
- **类型**: 迭代实现设计（spec）
- **上游**: `docs/superpowers/specs/2026-07-23-codecoder-maturity-to-90-roadmap-design.md`（路线图 · 迭代 4）
- **关联 ADR**: 0029（turn steering）、0028（项目 trust 加载门）、0035（workgraph 并发写保护）、0026（headless runner）

---

## 1. 背景与范围

评价报告 §4.2「整轮只探索不动手」是自主执行的最后短板；§6 footgun 里 `.ccd.env` 不自动加载（6.7）、allowlist 强依赖 trust（6.1）、并发写竞争（6.8）是健壮性缺口。

本迭代**收敛范围**（YAGNI，经确认）：
- **A. no-op 探索兜底**（核心自主性杠杆）——写内核代码。
- **B. `.ccd.env` 自动加载**——写代码，改动小。
- **C. allowlist 未加载引导**——**仅改引导，不放松 trust 门**（放松门本身是新 footgun）。
- **D. 并发写护栏**——**归文档/跨出**（评估报告自定性为编排者操作失误、非内核 bug；ADR 0035 已护 workgraph）。仅补编排纪律注记，无代码。

---

## 2. 决策（已确认）

- A 触发用 **turn 内连续 K 次纯探索 → 注入一次 nudge**（每 turn 幂等），而非事后告警（事后已由迭代 1 自恢复覆盖）。
- C 不动 trust 门，只在 headless+未 trusted+存在 codecoder.json 时 stderr 引导。
- D 无代码，仅文档注记。

---

## 3. 架构与改动点

### A — no-op 探索兜底（`src/agent.rs` process_turn tool 迭代循环）

- **纯探索工具集**常量：`const EXPLORATION_TOOLS: &[&str] = &["read_file", "glob", "grep", "diff"];`（保守只含纯读工具；`write_file`/`edit_file`/`run_command`/`commit`/`reason`/`milestone`/`memory`/`plan` 等都算「动了」）。
- 循环内维护两个局部：`consecutive_explore_iters: usize`、`nudged_this_turn: bool`（turn 起点重置，与迭代 2 `effective_max_tokens` 同一位置）。
- 每个 tool 迭代收集完 tool_calls 后判定：若本迭代 tool_calls **非空且全部** ∈ EXPLORATION_TOOLS → `consecutive_explore_iters += 1`；否则清零。
- 当 `threshold > 0 && consecutive_explore_iters >= threshold && !nudged_this_turn`：
  - 追加一条 `Role::User` steering 文本（模型下轮可见）：`You have only explored (read/glob/grep/diff) for N tool steps without making a change. Make a concrete edit or run a command now, or explicitly state that you are blocked and why.`（N = threshold）
  - 发 `AgentEvent::Notice("no-op backstop: nudged to act after N exploration-only steps")`。
  - 置 `nudged_this_turn = true`（每 turn 至多一次，防刷屏）。
- **配置**：`CODECODER_NOOP_NUDGE_THRESHOLD`（默认 3，`0` = 禁用）。`Config.noop_nudge_threshold: usize`；`AgentLoop.noop_nudge_threshold`（`build` 内由 `Config::from_env()` 注入，零构造点 fanout）+ `set_noop_nudge_threshold` setter 供确定性测试。
- 作用域：交互 + headless 都生效；headless 弱模型价值最大，与迭代 1 自恢复叠加。

放置点：nudge 注入必须在下一次 provider 调用之前，且不干扰既有 `drain_steer`（用户 steering）——两者都追加 User 消息，语义一致；nudge 是 agent 自发的一种 steering。

### B — `.ccd.env` 自动加载（`src/config.rs` + `src/main.rs` + `src/bin/cc.rs`）

- 纯函数（`src/config.rs`）：
  ```rust
  /// 解析 dotenv 风格文本为 (key, value) 列表:跳过空行/`#` 注释/无 `=` 行,trim,去成对引号。
  pub fn parse_dotenv(text: &str) -> Vec<(String, String)>
  /// 从 <root>/.ccd.env 读取并对每个 key 仅在未设置时 set_var(显式 env 优先,文件兜底)。
  /// 文件不存在静默跳过。返回实际注入的 key 数。
  pub fn autoload_ccd_env(root: &Path) -> usize
  ```
- 入口在 `Config::from_env()` **之前**调用 `autoload_ccd_env`：root 取 `CODECODER_ROOT` 或 CWD。`src/main.rs`(ccd) 与 `src/bin/cc.rs`(client) 各在 main 起始调用。
- 语义：仅当 env 未设置时注入（`std::env::var(key).is_err()` 才 `set_var`）。
- 测试：`parse_dotenv` 覆盖多行/注释/引号/无等号/空行；`autoload_ccd_env` 在临时 root 写 `.ccd.env` → 断言未设置的 key 被注入、已设置的 key 不被覆盖（用一个测试专用 key 前缀避免污染，测后清理）。

### C — allowlist 未加载引导（`src/lib.rs`，仅引导）

- 纯判定函数（`src/trust.rs` 或 `src/lib.rs`）：
  ```rust
  /// headless 且未 trusted 且磁盘存在 codecoder.json(有 allowlist 资源)→ 应提示。
  pub fn should_warn_untrusted_allowlist(root: &Path, trusted: bool, headless: bool) -> bool
  ```
  = `headless && !trusted && codecoder.json 存在`（复用 `trust::has_config_resources` 或直接查 `codecoder.json`）。
- 在 `run_background`（lib.rs，headless 入口）解析 trust 后调用；为真则 `eprintln!` 一条引导：`codecoder.json found but project is untrusted → allowlist not loaded; every pre-authorized Ask tool will be auto-denied. Set CODECODER_DEFAULT_TRUST=always or add ~/.codecoder/trust.json to load it.`
- **不改** trust 门 / 权限语义。测试：`should_warn_untrusted_allowlist` 各组合真值表。

### D — 并发写护栏（文档，无代码）

- `CLAUDE.md` / 评估报告补编排纪律注记：切忌向同一常驻 daemon 并发发消息（共享 session 历史 + 异步写 → 版本竞争）；并发工作用独立 root/daemon 或串行化。ADR 0035 已护 workgraph 并发写。

---

## 4. 测试策略（TDD，全 hermetic L1）

- **A**：有状态测试 Provider（前 K 轮只发 `glob`/`grep` 的 tool_call、其后发纯文本结束）→ 断言会话历史在第 K 轮后出现 nudge 文案且只一条；混入一次 `write_file` tool_call 重置计数、不触发；`threshold=0` 不触发；`set_noop_nudge_threshold` 覆盖默认。
- **B**：`parse_dotenv` 各格式；`autoload_ccd_env` 注入未设置 key、不覆盖已设置 key。
- **C**：`should_warn_untrusted_allowlist` 真值表（trusted/untrusted × headless/交互 × 有/无 codecoder.json）。

---

## 5. 文档同步

- README env 表：加 `CODECODER_NOOP_NUDGE_THRESHOLD`（默认 3）；`.ccd.env` 自动加载说明。
- ARCHITECTURE：补 no-op steering 兜底 + `.ccd.env` 自动加载。
- ADR：no-op 兜底修订 **ADR 0029**（追加 agent 自发 nudge）或新立；`.ccd.env`/allowlist 引导可在 0028 附近提一句。
- 评估报告：§4.2 标注 no-op 兜底已治；§6.1/6.7/6.8 对应 footgun 标注已治/已引导/已文档化。

---

## 6. 依赖与风险

- **A**：EXPLORATION_TOOLS 为启发式；保守只含 4 个纯读工具，planning/write 都算「动了」→ 误判低；nudge 每 turn 至多一条；`threshold=0` 可关。作用于交互+headless（无害）。
- **B**：`.ccd.env` 自动 `set_var` 改进程环境——只在启动早期、不覆盖已设值、文件缺失静默；风险低。
- **C**：纯 stderr 引导，零权限语义变更。
- **D**：无代码。
- 无并发/迁移风险。

---

## 7. 收尾定义（DoD）

- A/B/C 各 L1 测试全绿；既有测试不回退；全仓 `cargo test` 绿；文档一致。
- 维度预期：自主执行 →~90（no-op 兜底补自主性最后一环）、健壮性 →~90（B/C footgun 清零/引导）。

---

## 8. 下一步
本 spec 经复核后进入 writing-plans 细化为 TDD 分解、文件级改动的实现计划。
