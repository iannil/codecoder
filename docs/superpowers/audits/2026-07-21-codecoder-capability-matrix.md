# CodeCoder 能力矩阵与上限压测报告

- **日期**: 2026-07-21 ~ 07-22
- **范围**: 广度审计(全部内置能力)+ 深度展示(mdslides 集成任务压测)
- **环境**: codecoder 真实仓 `target/debug/{codecoder,cc}`;隔离工作区 `codecoder-lab/`(sibling);真实 DeepSeek(`.ccd.env`,model=`deepseek-v4-flash`)
- **方法**: 交互式经 `cc "<msg>"` one-shot(stdin 管道喂五类 Dialog 应答);headless 经 `CODECODER_BG_TASK`;断言基于文件系统 + `jq` + 日志标记 + 退出码(LLM 非确定性 → 结构化断言)。脚本: `docs/superpowers/scripts/{drive_cc,bg_runner,fake_cc}.sh`
- **一句话结论**: codecoder **已落地且能力齐全**——26 工具、Tool/Skill/Capability 三分自我进化、env×lifecycle、workgraph/reason/review 三一等公民、client-server、headless BG + SIGINT 全部实测可用;发现 1 个真实功能 gap(`/reload` 客户端不可达)+ 若干上限信号(迭代 cap、上下文漂移、复合命令 keying、固着行为)与 2 处文档/源码注释过时。

---

## 1. 能力矩阵

状态图例:`works` = 实测可用 · `limited` = 受条件限制 · `unimplemented` = 确未实现(文档已标注)· `gap` = 文档暗示可达但实测不可达 · `quirk` = 可用但有反直觉行为 · `observed` = 行为观察(上限信号)· `source-verified` = 未 live 触发但源码+单测佐证 · `doc-stale` = 文档与实际不符。

### 1.1 编译与连通

| 能力 | 状态 | 证据 | 备注 |
|---|---|---|---|
| `cargo build` | works | Finished dev | 编译通过 |
| 测试套件 | doc-stale | **244 passed + 3 ignored**(0 failed) | CLAUDE.md 244+3 ✓;**ARCHITECTURE.md/README.md 202+3 ✗ 过时** |
| cc↔daemon↔DeepSeek | works | smoke: `list_directory ✓`×3, EXIT=0 | 真实 LLM 非罐头 |

### 1.2 文件与搜索工具

| 能力 | 状态 | 证据 | 备注 |
|---|---|---|---|
| read_file / list_directory / glob | works | file_read.log | 读 AGENTS.md、列 3 个 .md |
| grep(文本) | works | file_read.log | 命中 CONTEXT.md Session 行 |
| grep(AST,tree-sitter) | works | grep_ast.log | rust/python/js/go 全命中;C 需更细查询(指针返回 declarator 嵌套) |
| write_file / edit_file | works | notes.md hello→hi(已核实) | 精确替换 |
| diff | git-gated | file_write / run_git | **非 git 仓库时 diff 不可用**;codecoder 降级 run_command;git init 后 ✓ |

### 1.3 执行与权限

| 能力 | 状态 | 证据 | 备注 |
|---|---|---|---|
| run_command(预授权类) | works | run_git.log | `run_command:git` 直跑无弹窗 |
| run_command(未授权)+PermissionKey | works | run_perm.log | `🔐 run_command:uname` 弹窗,key **按命令名细分**;喂 y ✓ |
| run_command 复合命令 keying | quirk | wg_trans.log | **`cd X && cargo test` → key `run_command:cd`**(首 token);预授权 cargo 不覆盖 |
| Permission Scope(AlwaysThisSession) | works | perm_scope2 无弹窗 | call1 喂 `s` 授权 whoami;call2 同命令不再弹窗 |

### 1.4 自我进化(Tool/Skill/Capability)

| 能力 | 状态 | 证据 | 备注 |
|---|---|---|---|
| use_skill(既有/新生) | works | evolve_1.log | **按名直读磁盘**,无需 /reload |
| generate_skill→use_skill 闭环 | works | skills/lab-conventions.md | 生成即可激活 |
| generate_prompt→promote_prompt | works | prompts/triage.md GONE / skills/triage.md EXISTS | ADR 0025 草稿转正原子(删草稿) |
| generate_capability→run_capability(Shell) | works | linter:输出 lint-ok | Shell/OneShot |
| run_capability(Wasm,预编译 .wat) | works | mdcount:输出 count=42 | **codecoder 自撰 WASI .wat**(fd_write+iovec+_start)成功 |
| run_capability(Docker) | works | dockhello:输出 hello | 本机 Docker 在场(10+ 容器) |
| Docker 缺失不降级契约 | source-verified | builtin.rs:384-386/484-486/571-573 + test | 3 处 lifecycle 显式 "refusing to downgrade to host (ADR 0021)" |
| **/reload 客户端可达性** | **gap** | socket.rs/proto.rs | **无 `ClientRequest::Reload`**;`AgentCommand::Reload` 无 wire 路径;自进化靠 use_skill 直读磁盘绕过,catalog 仅 daemon 重启更新 |

### 1.5 委派与交互(五类 Dialog)

| 能力 | 状态 | 证据 | 备注 |
|---|---|---|---|
| agent(子 agent,只读,深度锁 1) | works | delegate.log | 子 agent 只读勘察 samples/+skills/ |
| review(结构化 Verdict + 四信号) | works | review_strict.log | 严格框架下 `over_engineering=fail`;宽松框架被说辞化解 → **判定对框架敏感** |
| ask_user(自由文本 Dialog) | works | ask_user.log | `> ` prompt;喂 Rust 后据此建文件 |
| confirm(y/n Dialog) | works | confirm.log | `[y/n]:`;链式 prompt 中第二个默认 Deny |
| Trust(交互,未信任项目) | works | smoke / mem_reason | lab 不在 trust.json;新 session 首调发 Trust 弹窗 |

### 1.6 联网与开发

| 能力 | 状态 | 证据 | 备注 |
|---|---|---|---|
| search_web | works | net.log | 抓 example.com 成功(网络可用) |
| search_github(repos:) | works | net.log | 命中 tree-sitter 26.3k⭐ |
| search_github(code:) | limited | net.log:401 | **需 GITHUB_TOKEN**(未设);repos 免 token |
| reverse_api | works(输入不当) | net.log | 工具正常;设计用于 HTTP API 端点,Vec.html 非路由文档故无端点 |
| commit(git) | works | commit_tool2:83a757c | Ask 级 key=commit;喂 y ✓ |

### 1.7 规划与推理(一等公民)

| 能力 | 状态 | 证据 | 备注 |
|---|---|---|---|
| plan(PlanApproval Dialog) | works | plan_wg.log | 喂 y→批准,写 PLAN.md |
| milestone(workgraph 状态机) | works | workgraph.json | list/add/start/done/next/needs_fix/remove 全通;done 自动解除依赖阻塞 |
| drive_workgraph 自动推进 | source-verified | agent.rs | turn 结束自动推进就绪里程碑 |
| memory(文件 KV) | works | memory/lab-fact 落盘 | set+get 往返;**跨 session/跨进程实证**(BG 读到 daemon 写的 key) |
| reason(causal tree) | works | causal_tree.json(5 节点) | add/status(hypothesis)/list |

### 1.8 横切与内核

| 能力 | 状态 | 证据 | 备注 |
|---|---|---|---|
| compaction(tier-1 + tier-2) | works+stale-comment | agent.rs:607-649 + compaction.rs:299 test | context_working_set: tier1→should_compact→summary_span→LLM 摘要→apply_tier2;**compaction.rs:22-23 注释过时**(称 tier-2 deferred,实已接线+测试) |
| session 树(parent/child) | works | `cc tree` / `cc sessions` | 渲染 parent→child 消息树(ADR 0004 Phase A);节点示 codecoder 工具 arg 出错后自恢复 |
| client-server wire 五类 Dialog | works | 累计各 Task 日志 | Permission/AskUser/Confirm/PlanApproval/Trust 均经 wire 往返 |
| 上下文污染致行为漂移 | observed | mem_reason(旧 session) | 累积上下文(~21%)致 codecoder 重做 milestone 而非 memory/reason;重启后正常 |

### 1.9 Background Agent(ADR 0026)

| 能力 | 状态 | 证据 | 备注 |
|---|---|---|---|
| headless one-shot(CODECODER_BG_TASK) | works | bg-mdslides_run.log | 一轮跑完:读 memory+造 crate+跑测试,15 工具,BgOutcome 汇总 |
| memory 跨 session/跨进程 | works | BG 进程读到 daemon 写的 lab-fact | 文件级 KV 跨进程实证 |
| headless 拒绝路径(无人授权) | works | "denied: no user present" | 未授权 Ask 工具自动拒,记入 denied(ADR 0026/0028) |
| **SIGINT 优雅取消** | works | sigint_test: `run_capability: cancelled`;sleep 子进程被杀 | signal-hook→CancelToken→run_shell_cancellable kill 子进程;EXIT=0 优雅退 |
| BG turn 对失败测试的固着 | observed | mdslides_features:17 工具全花在修 parser 测试 | 单任务 one-shot turn 遇失败测试**固着**耗尽预算,未及其余特性;crate 回退至 0 测试 |

---

## 2. 「已知未实现」核验(CLAUDE.md 清单)

| 项 | 文档声称 | 核验结论 | 证据 |
|---|---|---|---|
| Wasm 源码→wasm 编译 | 未实现(ADR 0021,只接预编译 .wasm/.wat) | **属实(unimplemented)** | `run_capability rawrs@wasm` → `wasm run failed: expected '('`(把 .rs 当 WAT 解析,不编译) |
| Persistent 跨重启注册表 | 未实现(绑进程生命周期;崩溃标 Failed 不重启) | **属实** | capability.rs:60/93 `RunningServiceTable`(OnceLock 进程级)+ :114/124/157 Supervisor 标 Failed 不自动重启 |
| 内置调度器/多 runner 资源上限 | 未实现(调度外置,ADR 0026) | **属实** | background/daemon/agent 无任何 scheduler/Semaphore/concurrency-limit |
| margin/leverage/terminal | 仅字符串元数据,排序未内核化 | **属实** | reason.rs:41-43 三字段均 `"type":"string"`;:91-94 仅作 presence-check 建议转 milestone,无"余量×杠杆"内核排序 |

**结论**: CLAUDE.md 的「已知未实现」清单**全部属实**,无遗漏、无虚报。

---

## 3. 文档计数核对

| 计数 | 文档声称 | 实际 | 判定 |
|---|---|---|---|
| 内置工具 | 26 | **26**(builtin 15 + net 3 + dev 5 + reason 1 + search 2) | ✓ 一致 |
| ADR | 23 | **23**(`ls docs/adr/*.md`) | ✓ 一致 |
| 测试 | CLAUDE.md 244+3;ARCH/README 202+3 | **244 passed + 3 ignored** | CLAUDE.md ✓;**ARCHITECTURE.md/README.md 过时(202)** |

**建议**: 把 ARCHITECTURE.md(146 行 "202 个")与 README.md(141 行 "202 个")的测试计数更新为 244+3。

---

## 4. 上限在哪 / 可突破点

按影响排序的发现(均为本次压测实证):

### 4.1 真实功能 gap
- **`/reload` 客户端不可达**(高影响): `AgentCommand::Reload` 在 agent.rs 存在且被处理,但 client-server 协议(ADR 0032)**未定义 `ClientRequest::Reload`**,cc 也无 `/reload` slash。后果: agent 自撰生成的新 Skill **不会进入 system prompt 的 skill catalog**,除非重启 daemon。当前靠 `use_skill` 直读磁盘(按名激活)绕过——但这要求外部知道 skill 名,agent 自己"发现"不了新 skill。**可突破**: 在 `ClientRequest` 增 `Reload` 变体 + socket.rs 映射 + cc 加 `/reload` slash。

### 4.2 行为上限(非 bug,但约束自主性)
- **12-tool-iteration cap**: 一个 turn 内工具调用上限。压测中两次触顶(grep AST 的 C 查询精修、workgraph+测试循环)。长任务必须跨多个 turn/message,或调高 cap。drive_workgraph 的自动推进正是为跨 turn 续跑而设。
- **BG one-shot turn 对失败测试固着**: headless 单任务遇失败测试会反复 edit/run 耗尽 17 工具预算,未及其余请求事项,**且会回归代码**(crate 从 8/9 测试回退至 0)。显式约束"不改代码"后正常。**可突破**: BG runner 增加每子目标预算/失败熔断,或 workgraph 显式驱动逐里程碑而非单 message 塞多目标。
- **上下文污染致漂移**: 累积上下文(~21%)时 codecoder 把新指令(mem_reason)误执行为旧模式(重做 milestone)。长 session 需 `/clear` 或新 session 隔离异质任务。

### 4.3 反直觉行为(quirk)
- **复合命令按首 token keying**: `cd X && cargo test` → key `run_command:cd`,故预授权 `run_command:cargo` 不覆盖它 → headless 下被拒。codecoder 会自适应改用 `cargo test --manifest-path ...`。**可突破**: keying 取首 token 或改为校验"最危险子命令";或在预授权语义里允许复合命令覆盖。
- **diff 依赖 git 仓库**: 非 git 目录 diff 不可用(降级 run_command)。文档可点明。
- **空 stdin 默认行为**: `prompt_user` 读 `io::stdin()`,EOF 时 `read_line` 返 `Ok(0)` → 各 Dialog 默认(Trust→Once / Permission→Deny / Confirm/Plan→false)。管道驱动时需显式喂答。
- **review 判定对框架敏感**: 四信号由审查子 agent 的 LLM 判断产出,同一份过度工程代码在"测试样本"框架下判 pass,在"生产主干"框架下判 over_engineering=fail。**这是特性(LLM 判断)也是风险(可被说辞化解)**。

### 4.4 文档/注释一致性
- **测试计数过时**: ARCHITECTURE.md/README.md 202 vs 实际 244。
- **compaction.rs:22-23 源码注释过时**: 称 "Tier 2 is still deferred",但 tier-2 已在 agent.rs:607-649 接线 + compaction.rs:299 测试。CLAUDE.md(说已实现)正确。

### 4.5 能力上确认到位的上限(无 gap,仅记录边界)
- **网络可用**(非沙箱阻断): search_web/search_github(repos)/reverse_api 实测通;唯 `search_github code:` 需 GITHUB_TOKEN。
- **Docker 可用**: 本机 Docker 在场,docker capability 实跑;缺失时不降级契约已源码核验。
- **SIGINT 优雅取消**真实生效(ADR 0026 headline 特性): kill 子进程、记 cancelled、EXIT=0。

---

## 5. 方法论与可复现性

- **隔离**: 真实仓仅编译二进制 + 接收本报告;所有 codecoder 运行在 `codecoder-lab/`(sibling),真实仓 `src/`/`skills/`/git 未被触碰。
- **驱动**: `drive_cc.sh <label> <msg> [answers]`(stdin 重定向喂 Dialog 应答,tee 日志);`bg_runner.sh <label> <task>`(headless)。日志全在 `codecoder-lab/logs/`。
- **断言**: 文件存在 + `jq` 结构 + 日志标记(`⚙`/`✓`/`✗`/`🔐`)+ 退出码——LLM 非确定性下唯一的可复现证据。
- **产物**: `codecoder-lab/` 下 skills/(4)、capabilities/(5 manifest)、memory/lab-fact、causal_tree.json(5 节点)、workgraph.json(7 里程碑)、sessions/、showcase/mdslides/(crate)。
- **可信度**: 每条矩阵结论附可复现命令 + 证据文件名。负面结论(limited/unimplemented/gap)均附触发条件与源码/ADR 锚点。
