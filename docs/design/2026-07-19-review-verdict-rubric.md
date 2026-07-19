# Spec: review 结构化裁决 + 架构偏移 rubric(一等公民 #4)

> 来源:`docs/audit/0002-first-class-citizen-analysis-2026-07-19.md` 的候选 **#4**。
> tree-sessions spec 已把它列为「独立快胜」(:176)。本 spec 把现有 `review` 工具从
> 「自由文本 review」升级为**带结构化裁决(Verdict)+ 架构偏移 rubric** 的验收闸门。
> **零架构改动**:不加工具、不改 schema、不改会话格式、不加 AgentEvent 变体。
> 依赖 [[0019-sub-agent-capability-boundary]](只读子 agent)。

## 目标

- `review` 的产出从自由散文 → **确定性结构化头 + 散文**:
  ```
  REVIEW VERDICT: needs_fix
  signals: foundation=ok  over_engineering=warn  volume=fail  terminology=ok
  —
  <子 agent 的完整 review 正文>
  ```
- 引入 **Verdict ∈ {pass, needs_fix, rebuild}** 与 **四个架构偏移信号**(移植 engineer-inspector,
  按 codecoder 校准)。
- 裁决可被父 agent 确定性地读取,并作为**未来 Plan(#2)节点验收结果**的接入点(本 spec 只产出
  裁决值,不建 Plan 耦合——Plan 尚不存在)。

## 现状(必须兼容)

- `Review` 工具(`src/tool/builtin.rs:756`):schema 仅 `{ target?: string }`,`Permission::None`,
  `run` 返回错误占位(实际被拦截)。
- 拦截(`src/agent.rs:778`):把 `target` 拼成 task,`spawn_sub_agent` 跑一个只读子 agent,
  返回其 `last_assistant_text()` 作为 `ToolResult`(`is_error: false`)。
- 子 agent 工具集(0019):`read_file`/`list_directory`/`glob`/`grep`/`diff` 等 `Permission::None`
  只读集——**足以**读 `CONTEXT.md`、`docs/adr/`、跑 `diff` 做 rubric 比对。无需新增能力。

## 设计

### 1. 四个架构偏移信号(rubric,按 codecoder 校准)

移植 engineer-inspector 的「架构偏移」信号,阈值/口径按 codecoder(小文件、强术语表)调整:

| 信号 | 含义 | codecoder 校准 |
|---|---|---|
| **foundation**(篡改地基) | 悄改已固化的地基:公共类型/trait 签名、消息模型、权限键、会话格式、ADR 已定契约 | 对 `CONTEXT.md` + `docs/adr/` 的红线比对;命中即最严重 |
| **over_engineering**(过度设计) | 为边缘情形引入不必要的依赖/抽象/间接层 | codecoder 无 async 运行时、单二进制;新依赖/新抽象需强理由 |
| **volume**(体积失控) | 文件/函数不成比例地膨胀、复制粘贴 | 定性为主(codecoder 均 ~256 LOC/文件):超长函数、重复块、文件职责发散即 warn/fail |
| **terminology**(术语漂移) | 新命名撞 `CONTEXT.md` 术语表的 `_Avoid_` 列表,或引入近义词 | **codecoder 尤其适用**——`CONTEXT.md` 每个术语都带显式 `_Avoid_` |

每信号取状态 ∈ **{ok, warn, fail}**。

### 2. Verdict 聚合规则(内核护栏)

子 agent **既报**自己的 `VERDICT`,**也报**四信号状态。内核**不盲信** `VERDICT`,而是取
两者的**较重者**(护栏,防宽松 reviewer 放水):

```
severity: pass < needs_fix < rebuild

从信号派生 derived:
  foundation == fail            → rebuild
  else 任一信号 == fail          → needs_fix
  else (全 ok/warn)             → pass

最终 verdict = max_severity(子agent自报 VERDICT, derived)
```

- 严格 reviewer 的 `rebuild` 即使信号平和也被尊重(reviewer 可有定性理由)。
- 宽松 reviewer 报 `pass` 但 `foundation=fail` → 内核上调为 `rebuild`。
- **解析失败**(既无 `VERDICT:` 也无 `SIGNALS:` 行)→ `needs_fix`,并在头部标 `(unparsed)`
  ——安全默认,强制注意,绝不静默 pass。

### 3. 子 agent 输出契约(prompt 强约束)

`review` 拦截处的 task 模板改为:注入 rubric 说明 + 要求正文**以下述两行结尾**:

```
VERDICT: <pass|needs_fix|rebuild>
SIGNALS: foundation=<ok|warn|fail> over_engineering=<...> volume=<...> terminology=<...>
```

模板要点:读 `CONTEXT.md` 与 `docs/adr/`,用 `diff` 看变更(默认 target = 当前 diff),
按四信号逐项判定并给证据;裁决就低不就高时说明理由。

### 4. Verdict 解析(健壮、末位优先)

- 从**末尾向上**扫行,取**最后一个** `VERDICT:` 行(大小写不敏感)解析 token;同理最后一个
  `SIGNALS:` 行解析 `k=v` 对。对多余散文鲁棒。
- 未知信号键/值 → 该信号记 `warn` 并不阻断解析。

### 5. 输出契约(返回给父 agent 的 ToolResult)

确定性头(见"目标")+ `—` 分隔 + 子 agent 完整正文。`is_error` **保持 `false`**
(裁决是数据,不是工具失败;`needs_fix`/`rebuild` 不应伪装成 tool error)。

## 内核改动点

- **新增 `src/review.rs`**(纯函数 + 类型,便于单测):
  - `enum Verdict { Pass, NeedsFix, Rebuild }`(带 `severity()` 与 `as_str()`)。
  - `enum SignalStatus { Ok, Warn, Fail }`;`struct Signals { foundation, over_engineering, volume, terminology }`。
  - `fn parse_review(text: &str) -> ReviewOutcome`(末位优先解析 + 派生 + 护栏聚合 + unparsed 兜底)。
  - `fn format_result(outcome: &ReviewOutcome, body: &str) -> String`(确定性头 + 正文)。
  - `const RUBRIC_PROMPT: &str`(四信号说明 + 两行结尾契约;**内核内置默认 rubric**)。
- **`src/agent.rs` 拦截处(:778)**:改用 `RUBRIC_PROMPT` 拼 task;把 `spawn_sub_agent` 拆出
  一个返回**原始文本**的私有 helper(`spawn_sub_agent_text(task, event_tx) -> String`,
  `agent` 与 `review` 共用),`review` 分支拿到文本后 `parse_review` + `format_result` 再包成
  `ToolResult`。`agent` 分支行为不变。
- **`src/tool/builtin.rs`**:更新 `Review::description`(说明现在返回结构化裁决 + rubric)。
- **`src/lib.rs`**:挂 `mod review;`。

## 与在途工作的接口(不在本 spec 落地)

- **Plan(#2)**:`ReviewOutcome.verdict` 就是未来挂到里程碑节点上的验收结果。本 spec 只把它
  作为**可解析的返回值**产出,留作接缝,不建耦合。
- **on-disk rubric 覆盖**:遵循「机制在内核、方法在磁盘」,理想是 rubric 可被 `skills/review-rubric.md`
  覆盖。**本 spec 刻意先内置**(四信号稳定、直接镜像 engineer-inspector,是验收*契约*而非编排),
  把"子 agent 若检测到该 skill 则 `use_skill` 覆盖默认"列为**已记录的一行式扩展点,延后**。
  【唯一开放决策——若你更想一步到位走磁盘覆盖,评审时说,我改设计。】

## 测试(TDD)

- **`src/review.rs` 单测**:
  - 解析 `pass`/`needs_fix`/`rebuild`;大小写混合;多 `VERDICT:` 行取末位。
  - 缺两行 → `needs_fix` 且标 `unparsed`。
  - 派生:`foundation=fail`→rebuild;仅 `volume=fail`→needs_fix;全 ok→pass。
  - 护栏:自报 `VERDICT: pass` + `foundation=fail` → 上调 rebuild;自报 `rebuild` + 全 ok → 保持 rebuild。
  - `format_result` 头部格式稳定(可字符串断言)。
- **行为验证(`tests/`,黑盒,见 `docs/testing/behavioral-validation.md`)**:
  用 `ScriptedProvider` 依次驱动:父 turn 发起 `review` → 子 agent turn 产出含 `VERDICT`/`SIGNALS`
  的正文 →(子 agent 与父共享 `provider` Arc,脚本按顺序供给两段)→ 断言父侧 `ToolResult`
  文本以 `REVIEW VERDICT: <期望值>` 开头。

## 风险

- **reviewer 不遵守两行契约** → 解析兜底为 `needs_fix(unparsed)`;prompt 给明确示例降低概率。
- **volume 阈值主观** → 定性化 + 按 codecoder 小文件校准,宁可 warn 不 fail。
- **`diff` 工具覆盖范围** → 依赖现有 `diff` 工具看到的变更;若需暂存/未暂存全覆盖,沿用其现状,不在本 spec 扩展。

## 刻意不做

- 不加 `AgentEvent::ReviewVerdict` / TUI 徽章(避免 TUI 改动;裁决已在渲染的 ToolResult 头部可见)——
  待 Plan(#2)落地需要节点级展示时另议。
- 不建 Plan 耦合、不做 on-disk rubric 覆盖(见上,均为延后接缝)。
- 不改 `review` 的 `target` schema。

## 文档同步(落地后)

- `CONTEXT.md`:新增术语 **Review Verdict**(`_Avoid_`: grade/score/rating)与 **Drift Signal**。
- `ARCHITECTURE.md` / `README.md`:更新 `review` 一行描述与测试计数;工具总数不变(仍 25)。
