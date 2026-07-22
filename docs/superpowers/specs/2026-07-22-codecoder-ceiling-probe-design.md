# CodeCoder 上限深挖压测 — 设计文档

- **日期**: 2026-07-22
- **状态**: 待用户审阅(Pending user review)
- **作者**: Claude Code(brainstorming 产物)
- **关联**: 前序 `docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`(广度审计 + 首版上限报告)、`ARCHITECTURE.md`、`CONTEXT.md`、`docs/adr/*`、`README.md`

## 1. 背景与目标

昨天(2026-07-21~07-22)已完成一轮完整的 codecoder 能力探索并提交 master(设计→计划→《能力矩阵与上限报告》勘误版→驱动脚本)。该报告结论:**能力齐全、审计范围内无功能 gap**,并定位出若干**行为天花板**(12-tool 迭代上限、BG 失败测试固着致代码回退、~21% 上下文漂移、复合命令按首 token keying、review 可被说辞化解等);其中 **Persistent Supervisor 崩溃预算(ADR 0034)与 BG 账本/退出码(ADR 0033)仅源码核验、未 live**。

本轮**不重复广度审计**,改为**深挖上限**:针对每个已知天花板 + 报告未 live 的新特性,设计定向压测,把 codecoder 顶到失效边界,刻画失败模式、验证能否突破,并定向狩猎真实破坏(panic/死锁/数据损坏)。

**成功标准**: 产出一份可复现的《上限深挖报告》。每条结论 = 受控输入 → 结构化可测信号 → 判定(`行为边界 at X` / `破坏成立` / `live 坐实` / `safe`);诚实标出 bug 与负面结果;每条附命令 + 输出片段 + 落盘产物。

## 2. 已锁定的决策

| 维度 | 决策 |
|---|---|
| 探索方向 | **深挖上限**(不重复广度;针对报告天花板 + 未 live 新特性做定向压测) |
| 压测姿态 | **行为刻画 + bug 狩猎两者都要**(先刻画每个天花板边界,再对最脆处做定向破坏) |
| 战役结构 | **双轨混合(C)**:轨一 = 逐天花板纵切(干净归因);轨二 = 复合对抗长任务(涌现破坏);最后综合 |
| 运行边界 | **全新隔离工作区 `codecoder-probe/`**(不复用昨天的 `codecoder-lab/`,后者留作冻结证据;旧 sessions 会污染漂移/compaction 测量) |
| 操作模式 | **交互式 `cc`(drive_cc.sh)+ headless BG(bg_runner.sh)+ probe 专用探针**,三驱动并用 |
| 证据契约 | **断言只落结构化信号**(ctx% 序列 / `⚙` 计数 / mission_state↔exit code / `supervisor_state.json` / `workgraph.json` 状态迁移 / 非预期退出码+文件损坏),绝不靠匹配 LLM 文本 |

## 3. 架构

```
真实 codecoder 仓库 (保持干净)
  └─ 仅接收 docs/superpowers/{specs,plans,scripts,audits}/ + 最终报告
     (feature branch: explore/codecoder-ceiling-probe)

全新隔离工作区  /Users/rong.zhu/Code/codecoder-probe/   (CODECODER_ROOT 指向此)
  ├─ AGENTS.md / CONTEXT.md          ← 目标项目身份(codecoder 的「自我」)
  ├─ codecoder.json                  ← 每个 probe 按需覆盖 allowlist(松/紧/具体 key)
  ├─ logs/                           ← 全量 probe 日志(ts+label)
  ├─ probes/                         ← 每个 probe 的种子输入/期望/实测落盘
  ├─ skills/ capabilities/ prompts/  ← codecoder 自撰产物
  ├─ memory/ causal_tree.json workgraph.json sessions/
  ├─ bg_ledger.jsonl                 ← ADR 0033 probe 用
  ├─ supervisor_state.json           ← ADR 0034 probe 用
  └─ showcase/                       ← 复合对抗长任务(P12)种子

驱动层(复用 + 新增)
  ├─ drive_cc.sh        (piped-stdin 驱动 one-shot cc,复用昨天已验证版)
  ├─ bg_runner.sh       (CODECODER_BG_TASK headless,复用)
  ├─ probe_ctx.sh       (持续解析 stderr `[ctx N%]` → ctx% 时间序列)
  ├─ probe_concurrent.sh(并发起 N 个 BG/cc 打同一 lab,竞态探测)
  └─ daemon 生命周期脚本 (start → 灌崩溃 → 重启 → 查 skip-spawn)
```

**三个关键设计点**:
- **(a) codecoder 二进制只编译一次**(真实仓 `cargo build`),运行时 `CODECODER_ROOT=codecoder-probe` 指过去——「文件系统即自我」读的是 probe lab,与真实仓彻底隔离。
- **(b) 全新 probe lab** 而非复用 lab:漂移/compaction 测量需受控基线,昨天的旧 sessions/上下文会污染 onset 阈值。
- **(c) probe lab 置于真实仓 sibling 目录**(非 gitignored 子目录),杜绝路径污染真实仓。

**LLM**: `.ccd.env` 已配真实 key(DeepSeek,`deepseek-v4-flash`,经 OpenAI 兼容 base)。Phase 0 先 `source .ccd.env` 注入 shell;真实 LLM 让 codecoder 真能推理(非 StubClient 罐头)。限流/key 失效时降级观察为 StubClient 路径并如实说明。

## 4. 两轨 × 12 目标压测卡

每张卡:`驱动 → 可测信号 → 判定`。判定四类:**行为边界**(到哪失效/能否突破)、**破坏成立**(panic/损坏坐实)、**live 坐实**(报告只源码核验、本轮首次实跑)、**safe**(优雅处理)。

### 轨一:行为刻画(纵切,目标 1–8)

**P1 · 12-tool 迭代上限**
- 驱动:cc 单 message 给需 >12 工具的任务(如"逐个总结 src 下文件" + workgraph 一次塞 5 里程碑),统计单 turn `⚙` 数。
- 信号:`⚙` 触顶 12;是否发 Notice(报告称提交 16a4876 已加,不再静默截断);TurnComplete;下一 message drive_workgraph 是否续推。
- 判定:行为边界 = 单 turn 上限 12、触顶可见、跨 turn 可续。突破点:查 `config.rs` 有无 `MAX_TOOL_ITERATIONS` 可调。

**P2 · BG 失败测试固着 + 代码回退(报告痛点,对照实验)**
- 驱动:A = 单 message 多目标(种子带故意坏的测试);B = workgraph 逐里程碑(parser→model→renderer→tests)+ BG 跑。各跑 BG 对比。
- 信号:工具计数、测试通过数、crate 是否回退(N→0)、是否触 `CIRCUIT_K`。
- 判定:行为边界 = 单 message 固着回退 / workgraph 逐里程碑不固着。**验证 ADR 0030 护栏是否真化解报告的固着。**

**P3 · 上下文漂移阈值**
- 驱动:受控 session 逐步灌真实内容(read 大文件/多轮),在递增 ctx% 给异质小指令,看是否误执行为旧模式。
- 信号:`[ctx N%]` 序列、漂移 onset 区间(报告称~21%)、`/clear`/新 session 是否消除。
- 判定:行为边界 = 漂移 onset ≈ X%、清除有效(半定量,多轮取区间)。

**P4 · compaction tier-2 真实触发**
- 驱动:在 P3 长 session 上继续灌,逼过 tier-1 仍超 → tier-2。
- 信号:日志 tier-1(丢 Reasoning/占位化 ToolResult → ctx 回落)→ tier-2(LLM 摘要合成 System);合成消息抽样;anchor/tail 存活;read/modified 文件路径是否附摘要末尾。
- 判定:**live 坐实**(报告只源码核验)。

**P5 · 复合命令 keying**
- 驱动:cc 跑 `cd X && cargo test`、`A;B`、`A|B` 等,看 PermissionKey 取首 token;预授权 `run_command:cargo` 是否覆盖。
- 信号:`🔐 run_command:<首token>`;codecoder 是否自适应 `--manifest-path`;预授权覆盖与否。
- 判定:行为边界 = 按首 token keying、预授权不覆盖。突破点:keying 改取最危险子命令。

**P6 · review 对抗性化解**
- 驱动:写"能言善辩的过度工程"代码 + 为之辩护的 skill;宽松/严格两框架下跑 review。
- 信号:Verdict + 四信号(foundation/over_engineering/volume/terminology);严格下是否 fail、宽松下是否被化解;哪个信号最易绕过。
- 判定:行为边界 = 框架敏感、`over_engineering` 最易被说辞化解。

**P7 · Persistent Supervisor 崩溃预算(ADR 0034)— 报告未 live,重点**
- 驱动:写一个 Persistent capability,入口 `exit 1`(必崩)。起 daemon → `run_capability` → 崩 → 标 Failed;反复到 `CRASH_BUDGET`(默认 3);重启 daemon → 验跳过 spawn(gave_up);改 manifest mtime → 验预算重置再 spawn。
- 信号:`supervisor_state.json` 的 crash_count/gave_up;重启日志 "skip spawn";manifest 变更后重置。
- 判定:**live 坐实 ADR 0034**(会话内 give_up + 跨重启达预算跳过 + manifest 变更重置)。

**P8 · BG 账本 + 退出码告警(ADR 0033)— 报告未 live,重点**
- 驱动:跑 5 类 BG 各产一种 mission_state:正常(0)、BlockedAt(2,milestone deps 指向不存在前置)、CircuitBreaker(3)、Error(4,provider 故意错)、SIGINT(0)。
- 信号:`bg_ledger.jsonl` 的 mission_state↔exit code;`cc ledger` / `--failed` / `--detail`。
- 判定:**live 坐实** 5 种映射 + 账本可查。

### 轨二:bug 狩猎 + 复合对抗(目标 9–12)

**P9 · 并发 / fan-out**
- 驱动:(a) 单 turn 内 `agent` 连派多子 agent(测**宽度**,深度已锁 1);(b) `probe_concurrent.sh` 并发起 N 个 BG 打同一 lab;(c) daemon 同时接多 client。
- 信号:子 agent 宽度;并发写 `workgraph.json`/`memory`/`sessions/` 是否损坏;多 client 是否串话。
- 判定:**破坏成立** = `jq .` 解析失败/JSON 损坏/panic;**safe** = 串行 + 文件有序无损坏。

**P10 · 病态输入**
- 驱动:(a) 畸形 manifest(坏 JSON/缺字段/未知 Environment=`BareMetal`);(b) 50MB 大文件 read(撞 length-truncation-guard);(c) 超长 tool result 回灌;(d) grep AST 空/循环;(e) causal_tree 跨 schema 旧格式。
- 信号:每类 panic/异常退出/损坏 vs 优雅 Notice。
- 判定:**破坏成立** = panic/损坏;**safe** = 优雅拒绝。

**P11 · SIGINT 边界(报告只验 run_capability)**
- 驱动:在 (a) LLM 调用中途(无可取消点)、(b) commit/git 中途、(c) 并发双 SIGINT、(d) daemon 整体 vs 单 task 各发 SIGINT。
- 信号:取消生效与否、残留子进程、BgOutcome 反映取消、daemon 整体是否优雅。
- 判定:行为边界 = 可取消点干净取消、LLM 中途延迟到下个可取消点、无残留。

**P12 · 复合对抗长任务(交互破坏)**
- 驱动:一个"自找麻烦"BG showcase:workgraph 多里程碑 + 故意失败测试 + Persistent 崩耗预算 + 灌满上下文 + 复合命令 + review 化解,全塞一个长跑。
- 信号:哪个子系统先失效;交互下是否涌现单 probe 没有的破坏;整体是否仍产出可用 crate。
- 判定:**涌现破坏** = 单 probe 没有但复合出现的 panic/死锁/损坏;或"多天花板叠加仍收敛"的正面结论。

### 排序与依赖

P7/P8 是报告未 live 的新特性,价值最高、独立可跑,**最先**做(同时验证驱动脚本基线)。P4 依赖 P3 的长 session;P2 需 mdslides 种子;P12 依赖 P2/P4/P5/P6/P7 探针就绪,放**最后**。

## 5. 安全边界(不可破)

- **真实仓保持干净**:除 `docs/superpowers/{specs,plans,scripts,audits}/` 与本计划产出的报告外,绝不改动 `src/`、`skills/`、`capabilities/`、git master。
- **SIGINT 只对 probe-lab 进程**(P11/P12),绝不波及真实仓 daemon 或当前 Claude Code 会话。
- **bug 狩猎(P9/P10)只在 probe lab 内**喂病态/并发,绝不打真实仓 daemon。
- **Persistent 崩溃服务(P7)**:限 probe lab;靠 daemon 重启测预算,不无上限 spawn。
- **发现真 bug → 记带复现的 finding → 不在本轮修**(只读分析,沿用昨天 YAGNI)。
- **漂移/compaction 探针(P3/P4)每次用新 session**,杜绝旧上下文污染测量。

## 6. 成本预算(尽可能但有界)

- 每 probe 软上限:每个断言点 ≤ 3 次真实 LLM 尝试;全轮 BG 调用总量 ≤ ~20 次。
- **最烧 token 的是 P3/P4**(要灌满上下文):各 ≤ 2 次完整跑。
- DeepSeek 限流/key 失效 → 降级观察为 StubClient 路径并如实说明,不冒充成功。

## 7. 交付物

1. `docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md` — 上限深挖报告:逐天花板(刻画行为/破坏/突破点)+ 新特性 live 坐实(P7/P8)+ 涌现发现(P12)+ 诚实 bug 清单或"无 gap"。
2. `docs/superpowers/scripts/probe_ctx.sh`、`probe_concurrent.sh`、daemon 生命周期脚本。
3. 真 bug(若有)→ `docs/audit/2026-07-22-ceiling-probe-findings.md`(带复现)或报告内联。
4. `codecoder-probe/` lab(scratch,不进真实仓 git,留全部 probe 产物)。

## 8. 分阶段

| 阶段 | 内容 | 出口条件 |
|---|---|---|
| **Phase 0** 搭台 | 编译核验二进制(报告后新提交仍能 build);建全新 `codecoder-probe/`;移植/改造驱动脚本;真实 DeepSeek smoke | 二进制编译通过、probe lab smoke 一轮通 |
| **Phase 1** 轨一纵切 | **P7→P8**(新特性、独立、验基线)→ P1·P5·P6(便宜、dialog 驱动)→ P3 → **P4**(续 P3 长 session)→ **P2**(mdslides 种子对照) | 8 张卡各落结论 + 证据 |
| **Phase 2** 轨二破坏 | P9(并发)→ P10(病态)→ P11(SIGINT 边界),各隔离 | 破坏/安全判定各坐实 |
| **Phase 3** 复合 | **P12**(依赖 P2/P4/P5/P6/P7 就绪) | 涌现破坏 or 正面收敛结论 |
| **Phase 4** 综合 | 写报告 + 自检 + 提交 `explore/codecoder-ceiling-probe` 分支 | 报告无占位、提交成功 |

## 9. 不在本范围内(YAGNI)

- **不修改 codecoder 源码**(只读分析 + 运行现成二进制)。发现 bug 记入报告,不顺手修。
- **不做性能基准**(吞吐/延迟)——目标是"能力/行为上限"非"性能上限"。
- **不实现报告已标注的已知未实现项**(Wasm 源码编译、内置调度器)——只验证其确未实现,不在本轮复测(昨天已核验属实)。
- **不重复昨天的广度审计**——昨天已 works 的常规能力(read/write/grep/commit 等)不在本轮重测,只在需要时作为压测的脚手架。
