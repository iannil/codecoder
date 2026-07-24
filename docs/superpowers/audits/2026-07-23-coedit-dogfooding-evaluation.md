# Dogfooding 评价报告 — 用 codecoder 从零建成 coedit(RGA CRDT 协作编辑器)

- **日期**: 2026-07-23
- **类型**: dogfooding / 完成度评价(端到端实测,非纸面审计)
- **方法**: 用 codecoder 的二进制(`cc`/`ccd`/`CODECODER_BG_WORKGRAPH`)从零建一个**与 codecoder 无关**的外部 Rust 项目 `~/Code/coedit`——RGA CRDT 实时协作文本编辑器(核心 + WebSocket 服务 + 浏览器 demo)——由 Claude Code 充当编排/监督者,codecoder 充当实际建造者。
- **关联**: ADR 0026(Background Agent)、ADR 0033(退出码/账本,本次修订)、`docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`、`2026-07-22-codecoder-ceiling-probe.md`

---

## TL;DR

codecoder **确实用自己的能力建成了一个可运行、可收敛、双浏览器窗口真协作的 CRDT 编辑器**——远超 PoC。但把人抽走它跑不完:卡点需要外部"解卡"(重置 needs_fix、提 max_tokens、给精确指令)。**综合完成度 ≈ 75%**,缺口集中在**弱模型下的自主韧性**与**headless 路径的真实打磨**。当前准确定位是**"监督式自主(supervised-autonomous)",而非"无人值守"**。

本次实测还**直接发现并修复了 codecoder 内核的 2 个真实 bug**(见 §5),证明真实 dogfooding 的性价比极高。

---

## 1. 交付物与验收结果

目标项目 `~/Code/coedit`,采用 **RGA(Replicated Growable Array)** 字符级序列 CRDT。三级验收**全部达成**:

| 验收标准 | 结果 | 证据 |
|---|---|---|
| ① 算法正确 + 单测全绿 | ✅ | `cargo test` 5/5:并发同 origin 定序、乱序 integrate 收敛、幂等、并发删除、兄弟带子树收敛 |
| ② 服务端多客户端实时同步 | ✅ | 多线程 WS 服务(welcome+site_id 分配 / history 回放 / broadcast),site2+site3 实时互传 Op |
| ③ 浏览器两窗口真实协作可见 | ✅ | 双窗并发编辑收敛到同一 `HAAello WorldBB`,截图存 `~/Code/coedit/docs/verify/` |

**codecoder 编写了全部项目代码**:RGA 核心(`opid.rs`/`ops.rs`/`rga_char.rs`/`document.rs` 模块化拆分)、integrate 算法、5 个收敛测试、161 行多线程 WS 服务、440 行原生 JS 浏览器 demo(前端自带乱序 pending 缓冲,比 Rust 核心还多一层)。

---

## 2. 用到的 codecoder 能力(覆盖面)

`milestone`(建 7 节点依赖有序工作图)、`search_web` + `reason`(建 4 条收敛/因果不变量推理树)、`generate_skill`(产出 `skills/rga-invariants.md`)、`use_skill`(写核心前注入不变量)、`write_file`/`edit_file`/`read_file`/`glob`/`run_command`/`commit`、以及 headless `CODECODER_BG_WORKGRAPH=1` 逐里程碑自主推进 + `bg_gate` 客观验收门 + `mission_state`/退出码。交互驱动经 `cc "<msg>"` 单次消息;权限经 `codecoder.json` 预授权 + trust 门。

---

## 3. 按维度评价(基于实测)

| 维度 | 完成度 | 依据 |
|------|--------|------|
| **架构/设计** | 90% | "文件系统即自我"真生效:AGENTS.md 注入身份、CONTEXT.md 术语被代码精确遵守(Op/Char/OpId/SiteId/Origin/Tombstone);三分模型、工作图、trust/权限、账本/退出码均实际工作 |
| **单能力正确性** | 85% | `generate_skill` 产出的不变量 skill 质量惊人(完整 integrate 算法 + OpId 全序 tie-break + 边界条件);reason/use_skill/milestone/commit/edit_file 全按预期 |
| **客观验收门(护栏)** | 95% | **最大亮点**:模型谎报"19 tests pass"时,`bg_gate` 客观跑 `cargo test` 失败、把 M3 翻回 needs_fix——设计成功拦住幻觉 |
| **自主执行(headless)** | 60% | 能自主连做 M2→M5;但**无法从失败自恢复**:gate 失败→needs_fix 后 runner 放弃、需人手动重置 pending 才重试(且撞上"假绿 exit 0" bug,见 §5) |
| **健壮性/打磨** | 55% | 2 个真 kernel bug + 多个 footgun(max_tokens 默认 4096 致截断、单 daemon 并发写竞争、.ccd.env 不自动加载、acceptance prose 当命令) |

**综合 ≈ 75%**:声称的能力大多真实存在且可用,缺口在弱模型下的自主韧性。

---

## 4. 三个关键结论

1. **护栏设计是最成熟的部分。** 客观 gate 覆盖 agent 自报、trust 门、退出码告警、截断检测——这些"不信任 LLM"的防御性设计真正奏效,是把 LLM agent 做成工程系统而非玩具的核心。

2. **自主性受两头夹击:弱模型 + 缺自恢复。** 本次大量摩擦(整轮只探索不动手、大文件截断、谎报通过)源于 `deepseek-v4-flash` 偏弱;而 codecoder 默认值(max_tokens 4096、12 工具/turn、needs_fix 不自动重试)**放大**了模型弱点。换强模型 + 补自恢复循环可省掉约一半人工干预。（迭代 4 已治：no-op 探索兜底 nudge——连续 `CODECODER_NOOP_NUDGE_THRESHOLD`（默认 3）个纯探索步后注入 steering 推动动手，ADR 0029 修订）

3. **真实 dogfooding 立刻暴露纸面测试测不到的缺陷。** 247 单测全绿的项目,一放到真实外部构建,`BG_WORKGRAPH` 就假报成功、acceptance 门就执行中文 prose。说明 headless 路径此前未经真实项目端到端捶打。

---

## 5. 本次发现并修复的 codecoder 内核 bug

均已 TDD 修复、配回归测试、合入 master(commit `b623b1a` → merge `6a11098`),ADR 0033 已修订。

### 🔴 Bug B — `BG_WORKGRAPH` 在仅剩 needs_fix 时假报完成(exit 0)
- **现象**:一个 fresh 进程发现唯一可动的里程碑是 `needs_fix`(无 pending-ready)→ `advance_one_milestone` 立即返回 `None` → 循环 `Ok(None)` 分支**无条件**置 `CompletedAllReady` → exit 0。实测第 2 轮 headless **0 工具空跑却退出码 0 报"全部完成"**,而 M2 明明卡在 needs_fix。会让上层调度器/编排者误判成功。
- **修复**:新增 `MissionState::StuckNeedsFix(id)`(退出码 **2**);`Ok(None)` 分支置态前读图,存在 needs_fix → StuckNeedsFix。测试 `stuck_needs_fix_when_only_needs_fix_and_nothing_ready`。

### 🟡 Issue A — milestone acceptance 的 prose 被当 shell 命令执行
- **现象**:`bg_gate::extract_gate_command` 原样返回首个含命令关键字的行交 `sh -c`。agent 用 `milestone add` 写的自然语言 acceptance(尤其 CJK),如 `cargo init --name coedit 创建二进制项目`,整行被执行 → `unexpected argument '创建二进制项目'` → 假 `needs_fix`;或 `cargo test 通过` → 退化成空过滤 → 假 pass。**agent 用自己的工具写的 acceptance,却过不了自己的 gate**,是闭环缺口。
- **修复**:仅当匹配行为**纯 ASCII 命令**时才作命令门,prose 行跳过 → 交注入式 review 门。测试 `extract_gate_command_skips_prose_acceptance_with_command_word`。（迭代 3 已契约化：结构化 command 通道 + 写入引导 + gate_kind 可观测）

### 附:非 codecoder bug(澄清)
- **模型谎报测试通过** → deepseek-flash 行为;codecoder 的客观 gate **正确覆盖**了它(设计奏效的证明,非缺陷)。
- **文件版本竞争** → 编排者(Claude Code)向单个常驻 daemon **并发**发指令的操作失误,非 codecoder bug。
- **max_tokens 截断** → 已被检测并报告(有护栏),属默认调参问题。

---

## 6. 遇到的 footgun 清单(编排者视角)

1. `codecoder.json` 权限 allowlist 仅在 root 被 trust 时加载(需 `CODECODER_DEFAULT_TRUST=always` 或预写 `~/.codecoder/trust.json`)。（迭代 4 已引导：headless 且未 trusted 且存在 `codecoder.json` 时 stderr 一次性告警并指路 trust 途径，`should_warn_untrusted_allowlist`；trust 门本身不放松）
2. 权限 key 粒度:简单命令按 head(`run_command:cargo`),复合命令(`&&`/`|`/`2>&1`)按整串——headless 下复合命令被拒,agent 会自动降级为简单命令(韧性 ✓)。
3. milestone acceptance 应写**独占一行的裸命令**;prose 退到 review 门(弱信号)。
4. headless 只跑 `pending`;`needs_fix` 需手动重置 pending 才重试。（已修：迭代 1 自恢复循环——runner 在 `CODECODER_BG_MAX_FIX_ATTEMPTS` 预算内自动重试 needs_fix，耗尽才落 `StuckNeedsFix`）
5. `max_tokens` 默认 4096,写大文件会截断(已检测但需人为提高或引导小步写)。（已修：迭代 2 自适应预算——默认提至 8192，命中 `StopReason::Length` 时该 turn 有效 max_tokens 翻倍直至 `CODECODER_MAX_TOKENS_CEILING`（默认 32768），并在 system prompt 引导小步写；ADR 0038）
6. 单 turn 12 工具上限;弱模型易在 headless 下"整轮只探索不动手"。（迭代 4 已治：no-op 探索兜底——连续纯探索达 `CODECODER_NOOP_NUDGE_THRESHOLD` 步注入 steering nudge，见 §4.2 与 ADR 0029 修订）
7. 二进制不自动读 `.ccd.env`,启动前须 `source`;裸跑无 mode env 会阻塞。（迭代 4 已自动加载：`ccd`/`cc`/BG 启动即加载项目根 `.ccd.env`——但**只注入安全调参白名单**（`DOTENV_ALLOWED_KEYS`），密钥/端点/trust/loader 变量仍须来自真实 shell；不覆盖已设 env）
8. 切忌向同一常驻 daemon **并发**发消息(共享历史 + 异步写 → 文件版本竞争)。（迭代 4 已文档化编排纪律，见 CLAUDE.md Background 段；ADR 0035 已护 workgraph 并发写，session 历史仍无保护——请独立 root/daemon 或串行化）

---

## 7. 优先建议(补齐完成度)

1. **[P0] 补 needs_fix 自恢复循环**:runner 应把 gate 失败原因喂回 agent、自动重试 N 次(带退避/预算),而非一失败就要人重置。**这是从"监督式"迈向"无人值守"的最关键一步。**
2. **[P1] 默认值调优**:`max_tokens` 默认调高 / 按文件大小自适应;system prompt 引导 agent 小步写文件(截断是本次头号杀手)。
3. **[P1] acceptance 契约化**:`milestone` 工具校验/引导 acceptance 为裸命令,或在写入时把 prose 与命令分离。
4. **[P2] 常规化真实 dogfooding**:一次就找出 2 个 kernel bug,建议把"用 codecoder 建外部项目"纳入定期回归(可作 L3 门控用例)。

---

## 8. 结论

codecoder 已是一个**架构扎实、护栏可靠、能真正交付非平凡项目**的自主 agent——它的客观验收门设计尤其体现了工程成熟度。距离"无人值守"还差**一个 needs_fix 自恢复循环**和**一轮真实项目打磨**;弱模型进一步放大了这段差距。但地基是对的:换强模型 + 补自恢复,它有条件跨过"无人值守"这道线。

> 本次 dogfooding 同时反哺修复了内核 2 处缺陷并合入 master——**验证本身即是一次高性价比的体检。**
