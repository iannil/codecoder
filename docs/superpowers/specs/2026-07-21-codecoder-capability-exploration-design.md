# CodeCoder 能力探索与上限压测 — 设计文档

- **日期**: 2026-07-21
- **状态**: 已批准(Approved)
- **作者**: Claude Code(brainstorming 产物)
- **关联**: `ARCHITECTURE.md`、`CONTEXT.md`、`docs/adr/*`、`README.md`

## 1. 目标

分析 codecoder 的内置能力,**编译并启动** codecoder,系统性**探索所有能力**(广度审计),再用一个最大化复杂的端到端任务把最难、最差异化的特性**链起来压测**(深度展示),以此探明 codecoder 的真实**上限**与已知缺口。

**成功标准**: 产出一份可复现的《能力矩阵 + 上限报告》,每条结论附命令+输出片段+落盘产物;诚实标出 limited/unimplemented 项;核验文档计数声明(测试数、26 工具、23 ADR)与代码一致,**不一致即记为审计发现**。

## 2. 已锁定的决策(brainstorming 四岔路 + 一战术选择)

| 维度 | 决策 |
|---|---|
| 探索形态 | **广度审计 + 深度展示**(逐一验证全部能力 → 再链起来压上限) |
| 运行边界 | **隔离工作区**(`codecoder-lab/`,真实仓保持干净,仅最终报告写回) |
| 操作模式 | **交互式 `cc` + headless Background Agent 双模式** |
| 展示目标 | **集成任务:小 Rust crate**(`mdslides`) |
| cc 驱动机制 | **piped-stdin one-shot 驱动**(`drive_cc.sh`):`prompt_user` 读 `io::stdin()` 且 one-shot 模式 `cc "<msg>"` 只为 prompt 回复读 stdin,故可按序管道喂 `y/n/s/p/N`/自由文本 应答五类 Dialog。规划期查明 `pexpect` 未装,此法比 pexpect 更稳更简 |

## 3. 架构

```
真实 codecoder 仓库 (保持干净)
  └─ 仅最终接收 docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md
     (feature branch 提交)

隔离工作区  /Users/rong.zhu/Code/codecoder-lab/   (CODECODER_ROOT 指向此)
  ├─ AGENTS.md / CONTEXT.md          ← 真实目标项目身份(codecoder 的「自我」)
  ├─ codecoder.json                  ← 审计阶段较宽松预授权;展示阶段收紧压拒绝路径
  ├─ skills/ capabilities/ prompts/  ← codecoder 自撰产物(自我进化证据)
  ├─ memory/ causal_tree.json workgraph.json sessions/  ← 一等公民产物
  └─ showcase/mdslides/              ← 深度展示目标 Rust crate

驱动层
  ├─ drive_cc.sh        (piped-stdin 驱动 one-shot cc,Phase 1)
  └─ bg_runner.sh       (CODECODER_BG_TASK 驱动 headless,Phase 2)
```

**两个关键设计点**:
- **(a) codecoder 二进制只编译一次**(真实仓 `cargo build`),运行时 `CODECODER_ROOT=codecoder-lab` 指过去——「文件系统即自我」读取的是 lab 的 `AGENTS.md`/`skills/`,与真实仓彻底隔离。
- **(b) lab 置于真实仓的 sibling 目录**(非 gitignored 子目录),杜绝路径污染真实仓。

**LLM**: `.ccd.env` 已配置真实 key(DeepSeek,经 OpenAI 兼容 base)。Phase 0 先 `source .ccd.env` 注入 shell;真实 LLM 让 codecoder 真能推理(非 StubClient 罐头响应)。

## 4. 阶段

### Phase 0 — 编译、验证、搭台子

1. `cargo build` 编译 codecoder(核验「已落地」属实,无编译错误)。
2. `cargo test` 跑测试套件,核验测试计数声明。**注:仓库内文档自身已不一致**——`CLAUDE.md` 称「244 通过 + 3 `#[ignore]`」,而 `ARCHITECTURE.md`/`README.md` 称「202 通过 + 3」。审计将以 `cargo test` 实际输出为真值,判定哪个文档过时(这本身是一条审计发现)。
3. `cargo build --bin cc` 核验 cc 客户端二进制。
4. 建 `codecoder-lab/`:写一份真实但小的目标项目身份(`AGENTS.md` + `CONTEXT.md`),放审计阶段 `codecoder.json` 预授权。
5. 写 `drive_cc.sh`(piped-stdin 驱动 one-shot `cc "<msg>"`,按序喂五类 Dialog 应答,全量 tee 到日志)+ `bg_runner.sh`。
6. **Smoke**: 跑 `cc> 列出当前目录`,确认 daemon ↔ cc ↔ DeepSeek 全链路通,事件能流到 stdout。

**Phase 0 出口条件**: 二进制编译通过、测试计数与文档一致、lab 建好、smoke 一轮通过。

### Phase 1 — 广度审计(交互式 cc 驱动)

逐类别验证,**每项留证据**(命令 + 关键输出片段 + 落盘产物路径),填入能力矩阵。覆盖:

| 类别 | 工具/特性 | 验证重点 |
|---|---|---|
| 文件(只读) | read_file · list_directory · glob · grep | grep **AST 查询**验证 rust/python/js/go/c 五语法 |
| 文件(写) | write_file · edit_file · diff | edit 精确替换;diff 输出 |
| 执行 | run_command | 按命令类 keying;`run_command:git` 经 codecoder.json 预授权 |
| 自我进化 | use_skill → generate_skill+/reload+use_skill 闭环 → generate_prompt+promote_prompt → generate_capability+run_capability | 草稿转正(ADR 0025);Registry 重扫生效 |
| 委派/交互 | agent · review · ask_user · confirm | 子 agent 只读子集+深度锁 1;review 结构化 Verdict + 四信号 |
| 联网 | search_web · search_github · reverse_api | `repos:`/`code:`;沙箱不通则如实标 limited |
| 开发 | diff · commit | git 真实生效 |
| 规划/推理 | plan · milestone · memory · reason | workgraph 七动作;causal tree 跨 session meta 检索 |
| 一等公民 | Work Graph · Reasoning tree · Review verdict · 统一 Node 模型 | `drive_workgraph` 自动推进;Hypothesis/Locked |
| 横切 | 权限/allowlist · trust 门禁 · compaction · session 树 · wire 往返 | trust `never` 不加载 AGENTS.md/skills(ADR 0028);tier-1+tier-2 造超长上下文触发 |

**额外:诚实核验「已知未实现」清单**(把负面结果也写进矩阵):
- Wasm 只接受预编译 `.wasm`/`.wat`,源码跨编译未实现(ADR 0021)。
- Persistent 服务无跨重启注册表(绑进程生命周期;`Supervisor` 崩溃标 Failed 不重启)。
- 无内置调度器 / 多 runner 资源上限(并发由外置调度器限制)。
- margin/leverage/terminal 仅为字符串元数据,排序未内核化。

**Phase 1 出口条件**: 能力矩阵草稿填满(每项有状态+证据);已知未实现项逐一验证。

### Phase 2 — 深度展示(headless BG + SIGINT,辅以交互式 cc)

**任务**: codecoder 在 `lab/showcase/mdslides/` 端到端造一个小 Rust crate(markdown→slides 转换器,带单测)。任务被设计成**每个硬特性都是必需的**:

| 硬特性 | 被逼着用的方式 |
|---|---|
| workgraph | `milestone add` 拆依赖有序里程碑(parser→slide model→renderer→tests→docs);`drive_workgraph` 跨 turn 自动推进就绪项 |
| generate_skill | 为该 crate 沉淀「怎么写 renderer」`.md` 知识 → `/reload` → `use_skill` |
| generate_capability | 写 Shell capability(重复 lint/build)+ **Wasm capability**(纯计算核,如 slide 计数)→ `run_capability` 两环境都跑 |
| review | `review` 验收自身产物 → 结构化 Verdict + 四信号 |
| reason | 故意引入失败,`reason add` 建因果树定位根因,练跨 session meta 检索 |
| memory | 持久化关于该 crate 的跨 session 事实 |
| Background Agent + SIGINT | 最终集成跑用 `CODECODER_BG_TASK` headless;跑到一半发 **SIGINT**,核验优雅取消(CancelToken、`run_shell_cancellable` kill 子进程、BgOutcome 反映取消);再重跑至完成 |
| compaction | 会话变长时 tier-1/tier-2 触发(必要时灌超长上下文逼出) |

交互式 cc 用于:中途 `ask_user`/`confirm`/`plan` 审批的 Dialog 往返演示(五类弹窗真正走一遍)。

**Phase 2 出口条件**: mdslides crate 跑通(workgraph/review/reason/capability 产物落盘);SIGINT 取消路径验证;一次完整 BG 跑至完成。

### Phase 3 — 综合

写 `docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`(回到真实仓,feature branch 提交)。内容:
- **能力矩阵**: 每项能力 → 状态(works/limited/unimplemented)→ 证据 → 备注。
- **「已知未实现」核验结果**(负面结论)。
- **文档计数核对**(测试/工具/ADR 数与实际是否一致)。
- **上限在哪 / 下一步可突破点**(基于压测观察的洞察)。

## 5. 错误处理与边界

- **网络工具**(search_web/reverse_api/search_github):沙箱可能不通——尝试,不通如实标 `limited/blocked`,不假装成功。
- **Docker capability**:环境大概率无 Docker——`run_capability @docker` 应**显式报错不降级**(ADR 0021),正好验证「隔离不静默降级」契约。
- **DeepSeek 限流/key 失效**:真实 LLM 调用失败时,降级观察点为 StubClient 路径并如实说明,不冒充。
- **SIGINT 测试**:只对 lab 里的 background task 发,绝不波及真实仓或当前会话。

## 6. 验证「探索本身」的可信度

- 每条矩阵结论附**可复现命令 + 输出片段 + 落盘产物路径**(`skills/`/`capabilities/`/`memory/`/`causal_tree.json`/`workgraph.json` 文件)。
- 文档计数声明用实际 `cargo test` / 源码清点核对,不一致标红。

## 7. 不在本范围内(YAGNI)

- **不修改 codecoder 源码**(只读分析 + 运行现成二进制)。发现 bug 记入报告,不顺手修。
- **不做性能基准**(吞吐/延迟)——本次目标是「能力上限」非「性能上限」。
- **不跑 L3 真实 LLM 冒烟测试**(`#[ignore]`,单独门控);真实 LLM 验证由 Phase 1/2 的实际 codecoder 运行覆盖。
- **不实现已知未实现项**(Wasm 源码编译、跨重启注册表、内置调度器)——只验证其确未实现。

## 8. 交付物

1. `codecoder-lab/` 隔离工作区(含 codecoder 自撰的 skills/capabilities/memory/causal_tree/workgraph + mdslides crate)。
2. `docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md` 能力矩阵 + 上限报告(feature branch 提交到真实仓)。
3. (过程产物)`drive_cc.sh`、`bg_runner.sh`、各 Phase 的运行日志。
