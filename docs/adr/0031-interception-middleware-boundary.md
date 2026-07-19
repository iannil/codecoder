# ADR 0031: 拦截中间件——论证与边界

> **关于 #7 拦截中间件的专门 ADR,论证它与自撰安全([[0022-self-authoring-safety-loop]])
> 的边界、可行方案、以及 codecoder 在此处的取舍。**
>
> 结论:在 codecoder 的当前架构中,**不引入通用的 `block`/`mutate` 拦截管道**。
> 理由见 §3。改为**精准 hook 替代方案**,见 §5。

## 1. 什么是「拦截中间件」

pi 的 Extension Event Pipeline(详见 `docs/audit/0002`)是一个**双向事件管道**:
- `tool_call` 事件可**`block`**(拒绝执行该工具调用)
- `tool_result` 事件可**`mutate`**(修改工具返回结果再回灌给 LLM)
- `context` 事件可**修改发给 LLM 的 message 数组**
- `before_provider_request` 事件可**修改 HTTP 请求头/体**
- `input` 事件可**吞掉或改写用户输入**

这本质上是一个**中间件栈**:每次 tool call / tool result / provider request 都经过可组合的
拦截器链,每个拦截器可以读/写/终止。

codecoder 目前:**AgentEvent 仅出站**。事件是 agent → TUI 的单向通知。没有任何站点允许
外部队列(注册的扩展、磁盘上的 capability、skill)插入一个 handler 来修改内核行为。

## 2. 张力:与 codecoder 自撰安全回路的核心冲突

[[0022-self-authoring-safety-loop]] 定义了 codecoder 的自撰安全三道防线:

1. **`@shell` Capability 上限 `AlwaysThisSession`,永不 `AlwaysThisProject`** —— 自撰物
   不能越过人的明确边界。
2. **`generate_*` 仅 `write_file` 级** —— 自撰物只能写文件,不能注册自己为内核扩展。
3. **Capability 进程拿不到 agent 工具集,不能递归自撰** —— 运行中的自撰物无法调用 `generate_*`
   或 `agent`。

**拦截中间件与此三点都冲突:**

- 它给了运行时 Capability 一个**改写内核行为的口子**:Capability 在 shell 中运行→写一个
  `~/.codecoder/extensions/intercept.ts`→重启后此扩展注册 `before_tool_call` handler→
  拦截 `run_capability` 调用→绕过 permission gate。这本质上是一个**在 `generate_*` 之外、
  不需要 `/reload`、不需要信任门禁**的自撰安全绕行通道。

- 即使拦截器是 skill(注入的 prompt 文本,不可执行),`tool_result` 的 mutate 能力也会让 agent
  学会**自欺**:agent 要求 review → review 子 agent 返回 `VERDICT: rebuild` → 一个「审查回避」
  skill 拦截 tool_result,把 `rebuild` 改为 `pass`。这不是恶意,是 LLM 的「省力」倾向——而
  codecoder 的护栏设计正是为了防止这种自欺。

- **审计溯源的断裂**:拦截器可以抹掉自己的操作痕迹(`block` 掉某个 `commit` 并返回「已提交」)。
  即使有 SourceInfo(#5),源到拦截器链的路径也不透明。

## 3. 核心取舍:通用拦截管道 vs. 精准 hook

**通用拦截管道**的优势:
- 最大扩展性:skill/capability 可以插入任何行为
- pi 证明了它的可行性(~35 事件,1666 行类型定义)
- 可以实现「revert 前自动 review」「commit 前跑测试」「禁止 `git push` 到 main」等政策

**通用拦截管道的代价(对 codecoder 而言):**
- **安全模型翻转**:从「agent 运行在沙箱中,Capability 隔离,工具自报权限」→「任何已加载物
  可以改写内核」。这是 codecoder 与 pi 的根本分歧——pi 故意不做沙箱(codecoder 做了),
  因此 pi 的拦截管道是「弥补缺失的安全感」,codecoder 的拦截管道是「拆掉已有的安全墙」。
- **复杂度膨胀**:需要拦截器注册表、优先级排序、错误隔离(一个坏 handler 不应杀死 agent)、
  热重载(handler 变更何时生效)、并发安全。这至少增加 ~1000 LOC 和 3–5 个新概念。
- **调试困难**:「为什么这个 tool 没被调用?」→「被拦截器 block 了,但那个拦截器是哪个
  skill 注入的?」——违反 codecoder 的「文件系统即自我」透明性原则。

**结论:不引入通用拦截管道。** 改为**精准 hook**。

## 4. 精准 hook 替代方案

与其开放一个通用的 block/mutate 管道,不如在 agent loop 中**少数固定站点**提供可选的、
**skill 可注入的检查点**,且每个检查点有严格的行为契约(只读/不可绕行/可审计)。

| hook 站点 | 契约 | 安全边界 |
|---|---|---|
| `before_review` | 注入额外的 rubric 指令到 review 子 agent 的 prompt | 只读(追加 prompt),不改变 review 的返回路径 |
| `after_review` | 审查 `Review Verdict`,若为 `rebuild` 可追加确认 | 只读,不可修改 verdict(护栏仍在) |
| `before_commit` | 检查工作区是否有未跟踪的敏感文件 | 只读(仅检查,不阻止) |
| `before_milestone_done` | 检查验收标准是否满足 | 只读,但可追加 `Notice` |

这些 hook 的实现方式:**不是中间件,而是 `skills/` 中的一个约定。**
- agent loop 在特定站点**检查是否有同名 skill 已激活**(通过 Registry 或 `use_skill` 注入)。
- 若有,把该 skill 的 prompt 注入到该站点的上下文中(追加指令,不改变内核逻辑)。
- **不走拦截管道**,不走 handler 注册表,不走事件 block/mutate。

### 具体实现示例:`before_commit` hook

```rust
// 在 commit 工具执行前,agent loop 检查是否有 `skills/hooks-before-commit.md`
// 激活。若有,其内容被追加到 tool_call 的上下文(以 System 消息注入)。
// agent 看到这条指令后自行决定是否执行 commit。
// 内核不做强制拦截——信任 agent 的自觉,但 hook 的存在让 skill 作者能
// 在 prompt 层面引导行为。
```

**这本质上是 codecoder 已有的 `use_skill` 机制**,只是在固定的 loop 站点自动激活而非
手动调用。不引入新原语,不改变安全模型,不增加攻击面。

## 5. 推荐的替代方案——"skills hook 公约"

**不做拦截中间件,改为在 agent loop 中增加 3–4 个 `hook_` 站点**,每个站点:
1. 检查 Registry 中是否有 `hooks/<name>.md` 已激活
2. 若有,把该文件内容(作为 `System` 消息)注入到当前的 `CompletionRequest.messages` 头部
3. **不改变任何工具的执行路径**,不引入 block/mutate 能力

**站点清单(v1,最小):**

| 站点 | 触发时机 | skill 文件约定 | 用途示例 |
|---|---|---|---|
| `hook_tool_pre` | 每个工具调用前(已通过 permission gate) | `hooks/tool-pre-<tool-name>.md` | 「commit 前检查敏感文件」「review 前追加检查项」 |
| `hook_review_rubric` | review 子 agent 启动前 | `hooks/review-rubric.md` | 覆盖默认四信号 rubric(已作为 #4 的延后扩展点记录) |
| `hook_turn_end` | turn 结束后、下一个 turn 开始前 | `hooks/turn-end.md` | 「每轮对话后自动检查工作图进度」 |

**刻意不做(与通用管道的区别):**
- 不拦截 `tool_call` 的执行——hook 只追加 prompt,不阻止工具运行
- 不修改 `tool_result`——hook 不能篡改返回数据
- 不覆盖 `Provider` 请求——hook 不触及通信层
- 不提供 handler 注册 API——hook 即文件,活在 `skills/hooks/` 目录

## 6. 与现有一等公民的关系

- **#5 溯源统一**:hook skill 的来源由 `SourceInfo` 记录,「从哪来的 hook」透明。
- **#4 Review Verdict**:hook 可以追加 rubric,但不能修改 verdict——护栏在内核。
- **#2 Work Graph**:hook 可以检查里程碑状态,但不能绕过验收闸门。
- **Skill 系统**:hook 是 skill 的一种特殊激活模式(自动激活 vs 手动 `use_skill`),不新增
  文件格式或类型。

## 7. 实现成本估算

- 新增 `agent.rs` 中 3–4 个 `hook_` 站点,每个 ~5 行(检查 Registry + 追加 prompt)。
- 新增 `Registry::has_hook(name)` 查询方法。
- 新增 `CONTEXT.md` 术语 **Hook**(与 Skill 同层,自动激活)。
- 文档同步。
- 单测:验证 hook skill 被注入到 request 的 system prompt 中。
- 总 LOC:~150–200,零架构改动。

## 8. 延期理由(为什么现在不做)

1. **没有紧迫需求**:当前 codecoder 没有「在工具执行前注入 policy」的真实用例。
   所有验证需求(#4 review / #2 milestone acceptance)已由结构化裁决覆盖。
2. **`use_skill` 已能手动达到相同效果**:agent 可以在 review 前 `use_skill review-rubric`。
   hook 只是让这件事自动发生——便利性改进,不是新能力。
3. **需要先在 `skills/` 中积累 hook 用例**:至少有一个经过实战验证的 hook skill
   (如 `hooks/tool-pre-commit.md`),才能确认 hook 站点的位置和契约是合理的。
   在没有用例时猜测 hook 站点,极易过度设计。

**建议:在 `skills/` 中先落地一个 hook skill(如 review-rubric 覆盖),用 `use_skill` 手动激活
验证设计,迭代稳定后再内核化到 agent loop 的 hook 站点。** 届时开 spec 并实现。

## 9. 结论

| 方案 | 成本 | 安全风险 | 推荐 |
|---|---|---|---|
| **通用拦截管道**(pi 风格,block/mutate) | ~1000 LOC + 3–5 新概念 | 高——与 0022 自撰安全冲突 | ❌ 不做 |
| **精准 hook 站点**(prompt 注入,只读) | ~150 LOC | 无——只追加 prompt,不改变执行 | ✅ 延后,先积累用例 |
| **什么都不做** | 0 | — | 当前状态,合理 |

> **codecoder 不引入通用拦截中间件。** 它与 codecoder 的隔离立场和自撰安全回路正面冲突。
> 若未来需要,应走「先积累 skill hook 用例,再内核化精准 hook 站点」的渐进路线,而非
> 一步到位引入 pi 风格的 block/mutate 管道。