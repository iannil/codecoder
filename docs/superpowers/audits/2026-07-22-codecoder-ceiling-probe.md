# CodeCoder 上限深挖压测报告

- **日期**: 2026-07-22
- **范围**: 不重复 07-21 的广度审计;针对报告已定位的天花板 + 报告未 live 的新特性(ADR 0034/0033)做**定向压测 + bug 狩猎 + 复合对抗**。
- **环境**: 真实仓 `target/debug/{codecoder,cc}`;全新隔离工作区 `codecoder-probe/`(sibling,不复用 07-21 的 `codecoder-lab/`);真实 DeepSeek(`.ccd.env`,`deepseek-v4-flash`)。
- **方法**: 双轨——轨一逐天花板纵切(drive_cc/probe_ctx/bg_runner)、轨二 bug 狩猎 + 复合对抗(probe_concurrent/SIGINT)。断言 = 日志 + jq + grep 标记 + 退出码 + panic 扫描 + 源码核验(LLM 非确定性下唯一可复现依据)。脚本 `docs/superpowers/scripts/{drive_cc,bg_runner,probe_ctx,probe_concurrent}.sh`。
- **一句话结论**: 本轮在 07-21"无功能 gap"之上,**新挖出 5 条真发现/上限**(P8 BG 退出码不可达、P9 workgraph 并发丢更新、P10 无长度截断 guard、P5 复合命令 keying 安全 quirk、P11 daemon 不响应 SIGINT),**live 坐实 2 项新特性**(P7 ADR 0034 崩溃预算、P4 compaction tier-1),并纠正 1 处常见误读(P1 cap 粒度)。**未发现 panic/数据损坏类崩溃**,但若干契约与文档与 live 行为存在偏差。

---

## 1. 逐天花板结论(P1–P12)

状态图例:`live 坐实`(本轮首次实证)/ `行为边界`(到哪失效)/ `finding`(真发现/gap)/ `safe`(优雅处理)/ `observed`(行为观察)/ `正面收敛`(无破坏)。

### P1 · 12-tool 迭代上限 — 行为边界(粒度澄清)

- **实测**: 单 message 创建 15 文件 = 17 个 `⚙`/19 个 `✓` 工具调用,跨 ~4 个 LLM 轮次,**未触顶**。
- **结论(纠正误读)**: `MAX_TOOL_ITERATIONS=12`(`agent.rs:18`,const,无 env 旋钮)计的是 **LLM 轮次(round-trip),不是单次工具调用数**。每轮 LLM 可在一个响应里发**多个工具**(本轮一轮内 15 个 write_file 全派发)。要触顶需 12+ **串行依赖**轮次(每轮输出喂下一轮)。触顶 Notice 在 `agent.rs:931`。BG 单里程碑另有 `bg_milestone_tool_cap=8`(`CODECODER_BG_MILESTONE_TOOL_CAP` 可调)。
- **突破点**: 全局 cap 是硬编码 const,不可经 env 调;长任务靠跨 turn(`drive_workgraph`)/多 message 续跑。

### P2 · BG 失败测试固着 + 代码回退 — observed(未复现固着)

- **实测**: 易失败测试(2+2=5)+ 3 目标的 BG 单 shot → 10 工具均衡完成,最终 4 passed 0 failed(**无回退,反改善**)。
- **结论**: 固着**非自动,依赖测试难度**——07-21 报告的固着发生在 mdslides **硬解析**测试(反复修不好→耗 17 工具回退至 0);本轮种子太易,秒修后继续。且经 `CODECODER_BG_TASK` 走显式 task 单 turn 分支,circuit_k/gate 护栏本就不可达(见 P8),仅 `tool_cap=8` 兜底。

### P3 · 上下文漂移阈值 — observed(session 持久;onset 未复现)

- **实测**: ctx 4%→41%(读 10 文件);p4_3x 起步即 41% = **daemon 跨 cc one-shot 调用复用 session**(上下文累积)。
- **结论**: one-shot driver 无法中途注入异质小指令测漂移,07-21 报告的 ~21% onset 本轮**未复现**;但 **live 证实 session 跨 cc 调用持久**(为漂移提供了累积前提)。

### P4 · compaction tier-2 — tier-1 live 坐实;tier-2 未触发(源码核验)

- **实测**: 3 读 agent.rs → ctx **60%→41%→25%→7%→4% 暴跌** = **tier-1 静默运行**(elide Reasoning + 旧 ToolResult);log **无 "compacting context…" Notice**(`agent.rs:658/673`,仅 tier-2 发)→ tier-2 未触发。末触 12-tool-iteration cap Notice。
- **结论**: tier-1 已把 ctx 压到 4% 远低 75% 阈值(`COMPACTION_THRESHOLD=0.75 × 128K`),tier-2 LLM 摘要回退**未需介入**。tier-1 **live 坐实**;tier-2 仍源码核验 + 单测(`compaction.rs:299`)。

### P5 · 复合命令 keying — 行为边界 + 安全 quirk ⚠️

- **实测**: `cd showcase/mini && cargo test` → key = `run_command:cd`(首 token)→ 弹窗;预授权 `run_command:cargo` **不覆盖**;批 `cd` 后整条复合跑通(`cargo test` ok)。
- **结论**: key = `builtin.rs:48` `split_whitespace().next()` = **首个空白 token**(运算符无关;`ls; uname`→`ls;`、`ls | head`→`ls`)。
- **⚠️ 安全 quirk**: 预授权**良性前缀**(`cd`/`ls`/`echo`)等于**隐式授权后缀任意命令**(`cd X && rm -rf …` 只被 `run_command:cd` 门控)。
- **突破点**: key 取**最危险子命令** / 拒复合命令 / 逐子命令授权。

### P6 · review 对抗化解 — 行为边界(signal 稳,verdict 可化解)

- **实测**: 同一份过度工程代码,宽松框架(+ defend skill)与严格框架下,**4 信号均检 `over_engineering=fail`(一致)**;但 **Verdict 分裂**——lenient→`pass`(defend 修辞把检出 fail 说成"正当前瞻投资")、strict→`needs_fix`("必须重构为简单函数")。另观察 review tool 对 `samples/` 目录给 `warn`(上下文感知),严格框架下 LLM 升 `warn→fail`。
- **结论**: **4 信号稳定不可化解;Verdict 可被说辞/框架左右**。这是 LLM 判断的特性,也是可被公关式 skill 化解的风险。

### P7 · Persistent Supervisor 崩溃预算(ADR 0034)— live 坐实 ✅

- **实测**(daemon 重启循环 5 cycle,`CRASH_BUDGET=3`):
  - cycle1(fresh)→ crash_count=1;cycle2→2;cycle3→ **3, gave_up=true**(达预算)。
  - cycle4 → daemon stderr **"capability 'crasher' skipped: previously Failed (crash_count=3, budget=3)"**,**不 respawn**。
  - cycle5(touch manifest,mtime 1784702183→1784702195)→ reset → crash_count=1 → respawn。
- **结论**: ADR 0034 **live 坐实,与源码契约零偏差**(auto-spawn-on-start + 1s supervise tick + `reset_if_manifest_changed`)。07-21 报告只源码核验,本轮首次实证。纯进程管理,无 LLM。

### P8 · BG 账本 + 退出码(ADR 0033)— **finding(gap)** ⚠️⚠️

- **实测**: 正常 task → exit 0 / mission_state `Running`;**坏 API base(provider 错)→ 输出 "error: OpenAI request failed" 但仍 exit 0 / mission_state `Running`(非 4!)**。`cc ledger` 可读,但 `--failed`(= 非 CompletedAllReady)把**两行 Running 全标"需关注"**。
- **结论(代码 + 实测双重坐实)**: 经 `CODECODER_BG_TASK` 接口,**退出码 2/3/4 不可达**:
  - **2/3(BlockedAt/CircuitBreaker)** 仅由 `bg_gate::next_action` 产,只在 `run_background_cfg` 的 **workgraph 分支**(background.rs:131)被调;该分支要求 **task 为空**;但 `main.rs:7` 要求 CODECODER_BG_TASK 非空(空则走 daemon)。→ workgraph 分支对 BG 入口**不可达**。
  - **4(Error)** 全代码库唯一构造点 `background.rs:453` 是 `#[test]`;生产代码从不构造;provider 错误只冒泡成 `error:` 事件,mission_state 留 `Running`。
  - **CompletedAllReady 亦不可达** → `--failed` 过滤器(= 非 CompletedAllReady)**误报全部** BG run。
- **判定**: 映射代码正确且单测覆盖(bg_ledger.rs:191-195),但 **live 仅退出码 0 可观测**。README/ADR 0033 的 0/2/3/4 表与 live 行为存在**契约偏差**。

### P9 · 并发 / fan-out — **finding(workgraph 并发丢更新)** ⚠️

- **实测**: 4 并发 BG 各 `milestone add` → workgraph.json **0 milestone 存活**(`jq` 合法 = **未损坏**,但**数据丢失**);bg_ledger 4 行全合法(append-only 安全);sessions 6 个全合法(分文件安全)。子 agent 只读(list/read)、串行、深度锁 1。
- **结论**: workgraph.json 是**非原子 read-modify-write** → 并发写**静默 lost-update**(非 corruption,是 **data loss**)。ledger append-only 与 sessions 分文件均安全。
- **风险外延**: daemon 的 30s workgraph 推进线程(`daemon/mod.rs:79`)+ 并发 BG 也会互踩丢里程碑。

### P10 · 病态输入 — safe(无 panic)+ 1 代码发现

- **实测**: 畸形 manifest(坏 JSON)/未知 env(`BareMetal`)/缺字段 → daemon 启动**优雅跳过不崩**(start_all `continue`);grep AST 空文件 → `✓` 优雅空;reason 旧 schema causal_tree → `✓` 当空树(不迁移、不崩)。**5 类病态输入全无 panic**。
- **代码发现**: `read_file`(`builtin.rs:246` `read_to_string`)+ `run_command` 输出(`:98/:103` `read_to_end`)**master 上无长度截断 guard**(guard 仅在未合并分支 `feat/length-truncation-guard`)。大文件/冗长命令输出 → 无界内存/上下文膨胀(未 live 测以避 OOM)。

### P11 · SIGINT 边界 — 行为边界 + 1 发现

- **实测**: cc 中途 SIGINT(单 + 双)→ EXIT=0 干净退、**无残留**、daemon 仍活(只取消 turn)。**daemon 整体 SIGINT → 5s 仍活、socket 仍占、无 panic(不响应)**。
- **结论**: cc/BG 接 SIGINT→CancelToken(协作式取消,run_command/run_capability 轮询 kill 子进程);**daemon 未装 SIGINT handler**(只 SIGTERM 可停)。
- **关联发现**: `cc shutdown` 打印 "shutting down" 但**进程不退**(本轮多次复现),须 SIGTERM 才干净 → daemon 生命周期管理偏脆弱(无 SIGINT、shutdown 不可靠)。

### P12 · 复合对抗长任务 — 正面收敛(无涌现破坏)✅

- **实测**: 多天花板叠加(失败测试 + `run_command:cd` 拒绝 + git not-a-repo 错误 + review)的 BG 长跑 → 中途 SIGINT EXIT=0 无残留;重跑后 **10 tests passed 0 failed**;**panic/死锁/损坏扫描全净**;sessions 0 corrupt。
- **结论**: 叠加 stress 下 codecoder 仍产出可用 crate,**自适应绕过**复合命令拒绝(改 cargo 直跑)与 git 错误。**无单 probe 没有的涌现破坏**——多天花板不会"组合崩塌"。

---

## 2. 新特性 live 坐实汇总

| 特性 | ADR | 07-21 状态 | 本轮 | 证据 |
|---|---|---|---|---|
| Persistent Supervisor 崩溃预算 | 0034 | 源码核验 | **✅ live 坐实** | supervisor_state.json cc 1→2→3→gave_up→skip→reset |
| BG 账本 + 退出码 | 0033 | 源码核验 | **⚠️ live 部分不可达** | 仅 exit 0 可观测;2/3/4 不可达(P8) |
| compaction tier-1 | 0023 | 源码核验 | **✅ live 坐实** | ctx 60%→4% 暴跌 |
| compaction tier-2 | 0023 | 源码核验 | ⏳ 未触发(仍源码核验) | 无 "compacting context…" Notice |

---

## 3. 发现与上限清单(按影响排序)

### 3.1 真发现(gap / 契约偏差)

1. **P8 · BG 退出码 2/3/4 经 `CODECODER_BG_TASK` 不可达**(最高影响):workgraph 分支需空 task,main.rs 禁空 task;Error 从不构造。README/ADR 0033 的 0/2/3/4 表 live 仅 0 可观测;`cc ledger --failed` 因此误报全部。**建议**:BG 入口允许空 task 走 workgraph 模式;或显式 task 也接入 gate/circuit_k;或在显式 task 失败时置 `Error`。
2. **P9 · workgraph.json 并发 lost-update**:非原子 read-modify-write → 并发 BG(或 daemon 推进线程 + BG)静默丢里程碑。**建议**:写时文件锁 / 原子 replace / 串行化 workgraph 写。
3. **P5 · 复合命令首-token keying 安全 quirk**:预授权良性前缀隐式授权后缀任意命令。**建议**:key 取最危险子命令 / 拒复合 / 逐子命令授权。
4. **P10 · master 无长度截断 guard**:read_file/run_command 输出无界 → 大文件/冗长命令致内存/上下文膨胀。**建议**:合并 `feat/length-truncation-guard`。
5. **P11 · daemon 不响应 SIGINT + `cc shutdown` 不可靠**:daemon 无 SIGINT handler,`cc shutdown` 不杀进程 → 生命周期管理脆弱。**建议**:daemon 装 SIGINT→优雅 shutdown;修 `cc shutdown`。

### 3.2 行为上限(非 bug,约束自主性)

- **P1**: 全局 tool cap=12 是 **LLM 轮次**(非工具调用),const 不可调;每轮可批量多工具。长任务跨 turn 续跑。
- **P2**: BG 固着**依赖测试难度**,非自动;circuit_k/gate 护栏对显式 task 不可达(同 P8)。
- **P3**: session 跨 cc 调用持久(可累积上下文);漂移 onset 本轮未复现。
- **P4**: compaction 阈值=0.75×128K;tier-1 通常足够,tier-2 难触发。
- **P6**: review 4 信号稳,Verdict 可被说辞/框架化解。

### 3.3 文档/一致性

- **P8**: README ADR 0033 退出码表 vs live(仅 0 可观测)——契约偏差,建议更新文档或补可达路径。
- 其余文档计数(26 工具/23 ADR/244 测试)07-21 已核验,本轮未复测。

---

## 4. 上限在哪 / 可突破点(总览)

| 维度 | 上限 | 突破点 |
|---|---|---|
| 单 turn 工具量 | 12 LLM 轮次(每轮可批量多工具) | 跨 turn / env 调 cap |
| BG 任务终态 | 仅 `Running`/exit 0 可达 | 接入 gate/circuit_k 或允许空 task 走 workgraph(P8) |
| 并发 workgraph 写 | lost-update 丢里程碑 | 文件锁/原子 replace(P9) |
| 命令授权粒度 | 首 token(可被良性前缀绕过) | 最危险子命令 keying(P5) |
| 输出体积 | 无界 | 合并 length-truncation-guard(P10) |
| daemon 停止 | 仅 SIGTERM;SIGINT/shutdown 不可靠 | 装 SIGINT handler + 修 shutdown(P11) |
| review 判断 | Verdict 可化解 | 信号已稳;可加"signal fail→强制 needs_fix"硬规则(P6) |

**结论**: codecoder 在**单进程、单 turn、显式 task** 场景下稳健(P12 复合对抗无涌现破坏);上限与缺口集中在**(a) BG workgraph 模式不可达(P8)→ 固着护栏/退出码失效**、**(b) 并发写无保护(P9)**、**(c) 授权/输出/生命周期三处粒度偏粗(P5/P10/P11)**。这些都不是崩溃级 bug,而是**契约/鲁棒性**层面的可突破点。

---

## 5. 方法论与可复现性

- **隔离**: 真实仓仅编译二进制 + 接收本报告;所有 codecoder 运行在全新 `codecoder-probe/`(sibling);真实仓 `src/`/`skills/`/git 未被触碰(仅新增 `docs/superpowers/{scripts,audits}/` + 本报告)。
- **驱动**: `drive_cc.sh`/`bg_runner.sh`(复用 07-21)+ `probe_ctx.sh`/`probe_concurrent.sh`(本轮新增);日志全在 `codecoder-probe/logs/`。
- **断言**: 文件存在 + `jq` 结构 + grep 标记(`⚙`/`✓`/`✗`/`🔐`/`[ctx N%]`)+ 退出码 + panic 扫描 + 源码锚点——LLM 非确定性下唯一可复现证据。
- **诚实标注**: P2/P3 漂移未复现、P4 tier-2 未触发、P10 大文件未 live 测(避 OOM)均如实记为 observed/源码核验/limited,不冒充。
- **trust 预处理**: probe lab 预授 `~/.codecoder/trust.json` trusted,消除每-session Trust 弹窗对 stdin 应答的污染(本计划无 trust-阻断探测,不影响结论)。
