# pi 对照分析与借鉴路线图

`archived/pi`(earendil-works/pi)是 codecoder 的 **TypeScript 孪生**:同一个「自扩展编码 agent」命题,反向实现。因为命题一致,它是高信噪比的参照物。compaction batch-1 已从 pi 借过一轮(见 [[0023-context-compaction]] 增强说明)。本 ADR 记录一次系统对照:量化两者规模、各自结构性优劣,并给出**有优先级的借鉴路线图**,以及**刻意不借**的清单与理由。

> 本文是**分析与取舍记录**,不是实现契约。每个 Wave 若落地,应各自开 spec / ADR。

## 量化对照

| 维度 | codecoder (Rust) | pi (TypeScript) | 倍率 |
|---|---:|---:|---:|
| 源码 LOC | 6,660 | 111,737 | 16.8× |
| 源文件数 | 24 | 378 | 15.8× |
| 内置工具(原生) | 25 | 7(默认仅 4) | 0.28× |
| LLM provider | 1(OpenAI 规范)+stub | ~38 注册 / 35 厂商模型目录 / 17 协议适配 | 38× |
| 决策记录 | 18 ADR(≈1 条/370 LOC) | 外部 RFC + docs/ | — |
| 测试 | 112 `#[test]`(≈1 个/59 LOC) | 分离、按 API key 门控,未枚举 | — |

关键事实:pi 的 `ai` 包一个(36,702 LOC)就是整个 codecoder 的 5.5×。pi 的体量主要堆在两处——多 provider 广度(`ai`)与自扩展宿主+会话树+trust+telemetry 等产品化外围(`coding-agent` 52,660 LOC)。这不是「pi 更聪明」,而是 **pi 把大量真实 provider 怪癖与产品功能沉淀成了代码**。

## 各自的结构性优劣

**codecoder 的优势**(pi 主动放弃的):真隔离(原生 Wasm+Docker capability 执行,见 [[0021-capability-environments-and-lifecycle]])+ 真权限系统([[0005-permission-scope-and-session-allowlist]]);manifest 化 capability 比 pi 的解释执行 TS 扩展更有隔离原则;18 ADR + `CONTEXT.md` 术语表带来高架构自洽度;OS 线程 + channel 的内存安全真并发([[0016-channel-topology-and-event-model]]);单静态二进制交付。**pi README 明说不做沙箱、无权限系统,推给 OS/VM**——这正是 codecoder 相对 pi 的核心竞争力。

**pi 的优势**(codecoder 目前为空的):provider 广度(38 vs 1),含会话中途换模型、跨 provider 历史转换、prompt 缓存、compat-flags;**健壮性语料是「打出来的」**——overflow 检测 ~25 条正则、retry 白/黑名单、`length` 截断处理,codecoder 目前是 0;会话是树(fork/clone + 分支导航 + 废弃分支摘要);loop 工效(steering 三级队列 + 工具并行);多实例编排守护进程。

**一句话结论:codecoder 用 pi 6% 的代码量,守住了 pi 放弃的隔离与权限;代价是放弃了 pi 用 94% 代码换来的 provider 广度与健壮性语料。** 取舍逻辑随之确定:**借 pi 的「经验沉淀」,不借它的「体量与无边界哲学」。**

## 已在 codecoder 核实的三处缺口

- **`agent.rs` 无任何 stop-reason/`length` 处理** → pi 那个「截断后仍执行工具参数」的 bug 同样存在。
- **工具调用是串行 `for` 循环**(`agent.rs:479`)→ 并行执行是真正的新能力。
- **`registry.rs`/`permission.rs`/`capability.rs` 无 trust/签名门禁**,且 `AGENTS.md` 被无门禁自动注入 system prompt(`agent.rs:793`)→ clone 一个仓库,其 `AGENTS.md`/skills 就悄然注入进 agent 身份。这是「文件系统即自我」的锋利边缘,pi 恰好守此处。

## 借鉴路线图(按成本/依赖分波次)

评分维度:正确性 / 新能力 / 架构契合 / 低成本。**编排(原 Wave 5)明确标为范围外**(见末节)。

**Wave 0 — 正确性、健壮性与安全门禁**(先做;小、防御性)

| # | 借鉴项 | 主收益 | codecoder 现状 |
|---|---|---|---|
| 1 | `length` 截断时让整批工具调用失败(`failToolCallsFromTruncatedMessage`) | 正确性 | 无 stop-reason 处理,会执行截断参数 |
| 2 | overflow 正则库 + usage 锚定 token 估算,喂给 compaction 触发器(`overflow.ts`/`estimate.ts`) | 正确性/健壮 | 有 `tokenizer.rs` 估算;不捕捉 provider overflow |
| 3 | retry 分类器与策略解耦(`is_retryable(&msg)`) | 健壮 | 临时/不明确 |
| 4 | 错误即内联消息(`stopReason: error/aborted`) | 健壮/契合 | 统一 channel 内核错误管道 |
| 5 | **对磁盘加载的「自我」做 trust 门禁**(`trust.json`、就近祖先决策、`TRUST_REQUIRING_RESOURCES`),覆盖 `AGENTS.md`/`CONTEXT.md`/`skills/`/`capabilities/`/`prompts/` | 安全/契合 | 活的安全缺口——**故与 Wave 0 同波,不延后** |

**Wave 1 — Agent loop 交互工效**(新增、用户可感)

| # | 借鉴项 | 主收益 |
|---|---|---|
| 6 | steering / follow-up / next-turn 三级队列(turn 运行中改向、不打断) | 新能力 |
| 7 | 工具并行执行:串行权限预检 + 按源顺序回结果 + 单工具 `sequential` 兜底 | 新能力;权限仍确定性 |

**Wave 2 — 会话变成树**(较大)

| # | 借鉴项 | 主收益 |
|---|---|---|
| 8 | 树状会话:fork/clone、`/tree` 导航、离开分支时摘要废弃分支、配置变更即 transcript 条目;与 [[0023-context-compaction]] tier-2 摘要天然配对,扩展 [[0004-session-persistence-and-migration]] | 新能力 |

**Wave 3 — 多 provider 深度**(机会性)

| # | 借鉴项 | 主收益 |
|---|---|---|
| 9 | compat-flags 结构 + 跨 provider 历史转换 + `CacheRetention` 旋钮(会话中途换模型、prompt 缓存);建立在 [[0017-provider-neutral-message-model]] 之上 | 新能力/健壮 |

**TUI 波次 — 机会性打磨**(已有 [[0024-tui-viewport-and-render-loop]] + Overlay):overlay 先合成再 diff、光标标记 APC 解决 IME、`StdinBuffer` 完整性分类、行宽溢出崩溃日志。

## 刻意分道扬镳 — 不借(记下理由)

- **用渐进披露干掉 `use_skill`/`run_capability`**:pi 只给模型 `read`+`bash`、让它自己改自己的目录;codecoder **刻意**把这些做成显式工具([[0020-skills-and-capabilities-registry]]),且 Wasm/Docker manifest 隔离(pi 根本没有)需要它们。**替代**:可选地**增加** Agent-Skills 标准互通(读 `~/.claude/skills`),但不砍 codecoder 工具。
- **Operations 接口缝**(`BashOperations` 替换):是 pi 对 codecoder 已在 Capability-manifest 层做的事的替代品,优先级低。
- **pi packages / `appendEntry` 自定义会话条目**:分发故事过早;后者若 Wave 2 树状会话落地即免费得到。

## 明确范围外:多实例编排

pi 的 orchestrator 守护进程(N 个常驻 headless 实例、socket JSONL RPC、重启恢复,+ `extension_ui_request` 反向 UI 桥)是 [[0026-background-agent-headless-runner]] 之后的自然一步,但**不在本路线图范围内**:它与 CLAUDE.md / [[0026-background-agent-headless-runner]] 已列为延后项的「内置调度器 / 多 runner 资源上限」同类,应留待其成为实际需求时另开单独 ADR。此处仅记录 pi 的实现只需 ~1,987 LOC,设计小巧,届时可直接参照。

## Wave 0 实现记录

- **#1 已落地**:`length` 截断守卫(见 `docs/design/2026-07-18-length-truncation-tool-guard.md`)。`Provider::complete` 改返回 `Completion { message, stop_reason }`;截断且带 tool_call 时整批失败并重试。
- **#5 已落地**:trust 加载期门禁([[0028-project-trust-load-gate]])。
- **#3 已落地**:`src/retry.rs::is_retryable`(移植 pi `retry.ts` 语料,改用无依赖小写子串匹配)+ `AgentLoop::complete_retrying` 有界退避重试(分类器与策略分离)。
- **#2 部分落地**:`src/retry.rs::is_context_overflow`(移植 pi `overflow.ts` 语料)。overflow 被排除出可重试集,并在错误路径给出可操作提示(`/clear`)。**延后**:「强制压缩后重试一次」的反应式补偿——codecoder 每轮已按阈值主动压缩([[0023-context-compaction]]),反应式仅作安全网,留待需要时补。
- **#4 重新定性,不按 pi 原样移植**:pi 的「provider 永不抛、失败编码为 `stopReason: error/aborted` 流事件」是为 JS 流式生成器无法干净传播错误而设。codecoder 是**非流式 + 单 provider**,`anyhow::Result<Completion>` + `crate::retry` 分类器已是同一目标的**惯用等价物**(`?` 传播干净、错误已分类)。把 `complete` 改成永不 `Err`(返回带 `StopReason::Error` 的 `Completion`)会再次冲击所有 provider/测试且无功能收益,故**不移植该签名改动**。

## Wave 1 实现记录

- **#6 已落地**:turn steering + follow-up([[0029-turn-steering-and-follow-up]])。`SteerQueue` 共享句柄 + `process_turn` 双点 drain。
- **#7 暂缓(deferred)**:工具并行执行。技术可行(`ToolCtx` 轻量、`Tool: Send+Sync`、每并发工具各建 ctx),但性价比是本轮最低:需把 `dispatch_tool` 拆成「串行权限预检 → 并行执行 → 源序回结果」、拦截类工具须留串行、`ToolStarted/ToolFinished` 事件交错需 TUI 适配、并改动「turn 内工具串行」不变量;而收益仅限多工具 turn 的读取提速(进程内 Rust 读取本快)。留待确有多读取瓶颈时再做,届时按上述两阶段设计另开 ADR。
