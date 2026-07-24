# 设计 · 迭代 3：acceptance 契约化（结构化 command + gate_kind 可观测）

- **日期**: 2026-07-24
- **类型**: 迭代实现设计（spec）
- **上游**: `docs/superpowers/specs/2026-07-23-codecoder-maturity-to-90-roadmap-design.md`（路线图 · 迭代 3）
- **关联 ADR**: 0030（BG 客观验收门）、0033（账本/退出码）

---

## 1. 背景与定位

评价报告 §5 Issue A：agent 用 `milestone` 工具写的自然语言 acceptance（尤其 CJK）曾被原样交 `sh -c` 执行 → 假 needs_fix / 假 pass。**该执行 bug 已在先前提交修复**：`bg_gate::extract_gate_command` 现在仅当匹配行是**纯 ASCII 命令**时才作命令门，prose 行跳过 → 交 review 门。

迭代 3 处理剩余的**契约化与可观测**缺口：

1. acceptance 仍是自由文本，agent 无从知道「写成可运行命令能拿到强客观门、写 prose 只能拿弱 review 门」——缺少写入引导。
2. 命令提取靠行扫描启发式（猜测），没有显式的 command 通道。
3. 「某里程碑是强命令门还是弱 review 门通过的」不可观测——编排者/账本无法过滤弱信号验收。

---

## 2. 决策（已确认）

- **两者都做**：结构化 `command` 通道 + 写入引导 + 弱信号结构化可观测。
- 保留 `acceptance: String` 作 prose/人读契约；新增可选 `command` 通道。
- **保留** `extract_gate_command` 启发式作旧数据兜底（不破坏已有裸命令 acceptance）。
- 弱信号用**结构化 `gate_kind` 枚举**（非字符串约定）记录，随账本序列化。
- 写入引导只**提示**，绝不因缺 command 拒绝 add（保 agent 灵活性）。

---

## 3. 架构与改动点

### 改动点 1 — Milestone 结构（`src/workgraph.rs`）
新增字段：
```rust
    /// 可选的客观验收命令(迭代 3):独占一行的裸命令(如 `cargo test`)。存在时 bg_gate
    /// 走命令门;缺失时回退到 extract_gate_command(acceptance) 启发式,再回退 review 门。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
```
`acceptance: String` 不变。`#[serde(default)]` → 旧 workgraph.json 兼容，无 schema bump。`WorkGraph::add` 的 `Milestone { … }` 字面量补 `command: None`。

### 改动点 2 — 门路由单一事实源（`src/bg_gate.rs`）
```rust
/// 本里程碑将走哪种验收门(迭代 3 可观测)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GateKind { Command, Review, None }

/// 客观命令门要跑的命令:显式 command 优先,旧数据裸命令启发式兜底。
pub fn gate_command(m: &Milestone) -> Option<String> {
    m.command.clone().or_else(|| extract_gate_command(&m.acceptance))
}

/// 路由决策(单一事实源):有命令→Command;否则 acceptance 空→None;否则→Review。
pub fn gate_kind(m: &Milestone) -> GateKind {
    if gate_command(m).is_some() {
        GateKind::Command
    } else if m.acceptance.trim().is_empty() {
        GateKind::None
    } else {
        GateKind::Review
    }
}
```
`evaluate` 改为 `match gate_kind(m)`：
- `Command` → `run_command_gate(&gate_command(m).unwrap(), root, cancel)`
- `Review` → `review_runner()`
- `None` → `GateVerdict::Inconclusive("no acceptance criterion (weak signal)".into())`

对既有用例行为不变：`command == None` 时 `gate_command` 完全退回 `extract_gate_command`，与当前逻辑等价。

### 改动点 3 — 可观测（`src/background.rs`）
`SubgoalOutcome` 新增：
```rust
    /// 本里程碑实际走的验收门类型(迭代 3)。旧账本记录缺此字段 → 默认 None。
    #[serde(default = "gate_kind_default")]
    pub gate_kind: crate::bg_gate::GateKind,
```
（`fn gate_kind_default() -> GateKind { GateKind::None }`，或对 `GateKind` 实现 `Default = None` 并用 `#[serde(default)]`。plan 阶段择一。）
`run_milestone_and_gate` 用 `crate::bg_gate::gate_kind(&m)` 填入。随 `bg_ledger.jsonl` 序列化 → 编排者可 grep 弱信号（Review/None）通过的里程碑。

### 改动点 4 — milestone 工具 command 参数 + 写入引导（`src/tool/dev.rs`）
- schema `properties` 加：
  `"command": { "type": "string", "description": "Bare runnable acceptance command (e.g. `cargo test`) — objective gate; prefer over prose acceptance." }`
- `add` 分支：读 `command` arg；`g.add(title, acceptance, deps)` 成功后，在同一 `with_lock` 内把新节点 `command` 设为该值（不改 `WorkGraph::add` 签名 → 零 caller fanout，`reason.rs::to_milestone` 不受影响）。
- **写入引导**：add 成功后，若未传 `command` 且 `bg_gate::gate_command(new_node)` 为 None 且 acceptance 非空 → 工具输出追加一行：
  `note: no runnable command; this milestone will use the weaker review gate. Pass `command` (e.g. "cargo test") for an objective gate.`
  始终 add 成功。

### 改动点 5 — render 可见（`src/workgraph.rs` 及工具 list/next 输出）
`render`/`next` 对有 `command` 的节点显示一行 `cmd: <command>`，与既有 `accept:` 并列。

---

## 4. 测试策略（TDD，全 hermetic）

- **bg_gate**：`gate_command` 显式 command 优先于 extract；`gate_kind` 返回 Command（显式 command）/ Command（裸命令 acceptance 兜底）/ Review（prose）/ None（空）；`evaluate` 在各 kind 下走对应门（显式 command 优先于 extract 生效）。
- **workgraph**：新 milestone `command` 默认 None；旧 JSON（无 command 字段）反序列化 → None；render 显示 `cmd:`。
- **dev.rs milestone 工具**：`add` 带 command → 节点 command 设置且 render 显示；`add` 仅 prose acceptance（无 command、无可提取命令）→ 输出含引导提示；`add` 裸命令 acceptance（"cargo test"）→ **不**提示。
- **可观测**：`SubgoalOutcome` 序列化含 `gate_kind`；旧账本记录（无该字段）反序列化 → `GateKind::None`。

---

## 5. 文档同步

- README / CONTEXT 若引用 milestone 工具 schema，补 `command`。
- ADR 0030（BG 客观验收门）修订：追加「结构化 command 通道 + gate_kind 可观测 + 写入引导」。
- 评估报告 §5 Issue A 补注「（迭代 3 已契约化：结构化 command + 写入引导 + gate_kind）」。
- ARCHITECTURE 若述验收门，补一句结构化 acceptance。

---

## 6. 依赖与风险

- **账本向后兼容**：`SubgoalOutcome` 加字段后旧 `bg_ledger.jsonl` 记录缺 `gate_kind` → `#[serde(default)]` 落 `GateKind::None`（最保守，旧记录无门类信息）。加旧-JSON 反序列化测试锁住。
- 无并发/迁移风险（纯 append 字段 + 工具文案 + 门路由重构；`evaluate` 行为对无 command 的既有用例等价）。

---

## 7. 收尾定义（DoD）

- §4 测试全绿；既有 bg_gate/milestone/ledger 测试不回退；全仓 `cargo test` 绿；文档一致。
- 维度预期：健壮性 →~82；护栏保持 95（结构化 command 让更多里程碑走强门，gate_kind 让弱信号可见）。

---

## 8. 下一步
本 spec 经复核后进入 writing-plans 细化为 TDD 分解、文件级改动的实现计划。
