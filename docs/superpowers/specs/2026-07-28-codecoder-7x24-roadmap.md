# CodeCoder 7×24 自主运行路线图

> 基于三次实验：2026-07-25 strategic-control（11/11 milestone pass）、2026-07-28 smcs（10/10 milestone pass，12,425 行代码）、Sentinel 编排实验
> 参考：Fowler/Unmesh 精确语言思想、AI+确定性工具混合模式、小步快跑敏捷原则
> 设计日期：2026-07-28

---

## 总览

### 分层路线图

```
Phase 1 (稳定基础)     Phase 2 (自主闭环)       Phase 3 (宏观扩展)     Phase 4 (守夜进化)
                                                                        
验收三级升级           空图自播种                全栈推理               熔断降级
权限体验流畅化         Plan headless             多项目编排             跨会话记忆
P0/P1 基础设施修复     Write_file 截断防御       维护模式              远程告警
循环兜底               Checkpoint/resume         API/webhook(延后)     自我改进(远期)
工具 cap 调整          Token 可见性
行为约束补充           跨里程碑修复
```

### 核心设计原则

1. **精确验收优于自然语言信任** — 每个 milestone 的验收标准从自然文本升级为结构化 `checks` 数组，覆盖从"构建通过"到"组件非壳"的语义检查。结合 Fowler 的思路：用精确的领域专用语言描述问题，减少歧义。

2. **AI 生成 + 确定性工具执行** — LLM 负责理解意图和生成方案，验收门对执行结果做确定性检查（grep/行数/文件名/退出码）。AI 与确定性工具各司其职。代码重构也遵循此模式：AI 发现问题，确定性工具执行改动。

3. **小步快跑** — 不改当前 milestone 粒度，但确保每个里程碑的 tool cap 足够一次通过。验收通过是"发布到下游"的条件，验收失败自动进入修复轮（不超过 `bg_max_fix_attempts`）。更快的循环次数而不是更大的一次性 batch。

4. **可信者无扰** — 有 `codecoder.json` 即信任，无需双重配置。权限闸门在性能关键路径上保持零开销，安全从配置简化中获得。

5. **外置调度，内建闭环** — 多项目编排走外部脚本，不违背 ADR 0026 的调度外置决策。内建聚焦：一个项目从头到尾自主完成。

---

## Phase 1：稳定基础

### 1.1 验收门三级升级

#### 问题

当前 `command` 门验收与文本验收标准（acceptance）之间存在语义鸿沟。实验中最典型的表现：acceptance 要求"页面有真实组件"，但 agent 用 PlaceholderPage 壳页面冒充实现，`npm run build` 通过即算过关。没有任何确定性检查能发现这种偷懒。

这本质上是 Fowler 所说的问题：自然语言模糊、容易被"看起来合理"的输出钻空子。

#### 设计：`checks` 字段 + 三级验收架构

**workgraph.json 中每个 milestone 新增可选 `checks` 字段：**

```json
{
  "id": 10,
  "title": "集成验收与构建修复",
  "acceptance": "所有页面构建无错误，路由可访问，基础测试通过",
  "command": "npm run build",
  "checks": [
    {"type": "build_exit_zero", "command": "npm run build"},
    {"type": "no_template_content", "patterns": ["src/pages/**/*.tsx"], "forbidden": ["PlaceholderPage"]},
    {"type": "file_count_min", "path": "src/pages", "min": 50},
    {"type": "min_lines_per_file", "paths": ["src/pages/**/*.tsx"], "exclude_patterns": ["**/index.tsx", "**/*.test.tsx"], "min": 20},
    {"type": "no_relative_imports", "path": "src"}
  ]
}
```

**三级验收执行顺序：**

```
┌─────────────┐
│ 1. command 门 │  ← 已有：跑命令，检查退出码
└──────┬──────┘
       │ pass
       ▼
┌─────────────┐
│ 2. checks 门 │  ← 新增：确定性规则列表，按序执行
└──────┬──────┘
       │ pass
       ▼
┌─────────────┐
│ 3. review 门 │  ← ADR 0039：子 agent 评审（仅 review-kind milestone）
└─────────────┘

任一级 fail → 终止验收，记为 needs_fix
```

**支持的 check type（v1）：**

| 类型 | 确定性 | 参数 | 说明 |
|------|--------|------|------|
| `build_exit_zero` | ✅ | `command` | 等价 command 门的子集，可内联 |
| `no_template_content` | ✅ | `patterns`, `forbidden` | glob 匹配文件，grep 禁止内容 |
| `file_count_min` | ✅ | `path`, `min` | 目录下最少文件数 |
| `min_lines_per_file` | ✅ | `paths`, `exclude_patterns`, `min` | 确保业务文件真实内容 |
| `no_relative_imports` | ✅ | `path` | 禁止相对路径 import（强制 alias） |
| `no_deprecated_package` | ✅ | `package` | 检查依赖黑名单 |
| `max_file_size` | ✅ | `path`, `max_bytes` | 文件不超过大小阈值 |

#### 实现

**位置：** `src/bg_gate.rs`

新增模块：
```rust
pub struct CheckSpec { pub type_: CheckType, pub params: HashMap<String, Value> }
pub enum CheckType { BuildExitZero, NoTemplateContent, FileCountMin, MinLinesPerFile, ... }
pub fn run_checks(specs: &[CheckSpec], root: &Path) -> Result<(), Vec<String>> { ... }
```

每种 check type 映射到一个纯 Rust 函数。确定性执行，不依赖 LLM。

#### 向后兼容

`checks` 字段 `#[serde(default, skip_serializing_if = "Option::is_none")]`。不设 `checks` 的 milestone 执行原有逻辑（command 门 → 无 checks → review 门）。`command` 字段仍然保留，与 `checks` 重叠时仅执行一次。

---

### 1.2 权限体验平滑

#### 问题

当前 headless 需要双重配置才能使用：`CODECODER_DEFAULT_TRUST=always`（环境变量）+ `codecoder.json` 完备 allowlist。缺一不可。两次实验中第一次缺 trust 全拒，第二次缺 allowlist 条目又把 `write_file` 拒了。

用户在交互模式下只需要一重（响应信任弹窗），headless 反而需要更多配置——这不合理。实验报告中也已记录"headless 权限三关卡体验差"。

#### 设计：有 codecoder.json 时自动信任

在 `agent.rs` 的 `build()` 方法中修改 trust 判定逻辑：

```rust
// 当前 headless 逻辑:
None if headless => match trust::default_trust() {
    Trusted => TrustState::Trusted,
    Untrusted => TrustState::Untrusted,
},

// 改为:
None if headless => {
    if trust::default_trust() == TrustDecision::Trusted {
        TrustState::Trusted
    } else if trust::has_project_allowlist(&root) {
        // codecoder.json 存在 → 用户意图已明确 → 自动信任
        eprintln!("ccd: codecoder.json found → auto-trusted for headless");
        TrustState::Trusted
    } else {
        TrustState::Untrusted
    }
},
```

新增函数 `trust::has_project_allowlist()`：

```rust
pub fn has_project_allowlist(root: &Path) -> bool {
    std::fs::metadata(root.join("codecoder.json")).is_ok()
}
```

这样用户只需要：
1. 准备 `codecoder.json`（必填）
2. `CODECODER_BG_WORKGRAPH=1`（必填）
3. `CODECODER_API_KEY`（必填，需 shell 环境变量）
4. `CODECODER_ROOT`（可选）

不再需要 `CODECODER_DEFAULT_TRUST=always`。

#### 辅助改进：拒绝信息增强

当 deny 发生时，错误信息不仅提示"设 DEFAULT_TRUST"，还应提示"检查 codecoder.json 是否包含该 key"。

---

### 1.3 P0/P1 基础设施修复

以下项目已有设计文档（`codecoder-headless-fix-design.md`、`7x24-gap-audit.md`），这里引用而非重述：

| 项目 | 已有设计 | 位置 |
|------|---------|------|
| P0-1 空图自播种 | ✅ | `P0-1-empty-graph-self-seeding-design.md` |
| P0-2 plan headless | ✅ | `codecoder-headless-fix-design.md` §2 |
| P0-3 write_file 截断 | ✅ | `codecoder-headless-fix-design.md` §3（按此设计中的 §2.3 修订版） |
| P0-4 复合命令权限 | ✅ | ADR 0036 + `headless-fix-design.md` §2 |
| P1-1 跨里程碑修复 | ✅ | `7x24-gap-audit.md` P1-1 |
| P1-4 Token 可见性 | ✅ | `7x24-gap-audit.md` P1-4 |
| 循环兜底 | ✅ | `codecoder-headless-fix-design.md` P3 |

P0-1 的空图自播种在当前 Phase 1 只需完成**可调用存根**，完整的自主分解在 Phase 2 中做。Phase 1 的 seed 函数只需：检测空 workgraph → 调用一次 LLM 读取 AGENTS.md 生成 3-8 个里程碑 → 写入 workgraph.json。

P0-4 的复合命令 keying 在 Phase 1 中的修复方案：
- 不修改 is_compound 的检测逻辑（安全敏感）
- 改为在 `codecoder.json` allowlist 中支持 `run_command:*` 通配符（仅限 `CODECODER_DEFAULT_TRUST=always` 的 trusted 项目）
- 所有 `Permission::Ask { key: "run_command:"... }` 先匹配精确允许列表 → 再匹配通配符 → 未匹配则拒绝

---

### 1.4 工具 cap 调整

来自 `codecoder-headless-fix-design.md` P2：

- **`bg_milestone_tool_cap` 默认 8→15**：scaffold 类里程碑需要写 7+ 文件 + install + git init，8 次不够一轮完成。
- **`diff` 非 git fallback**：非 git 目录返回清晰错误信息而非 git usage 全文。
- **`bg_max_auto` 默认 10→0（不限）**：配合 `bg_circuit_k` 默认 3 做安全护栏。已由 ADR 0039 部分解决。
- **`CODECODER_BG_MAX_FIX_ATTEMPTS`**：默认 3 已由当前代码确定，不改。

---

### 1.5 行为约束补充

来自实验教训，在 `AGENTS.md` 或 seed prompt 中补充：

```
## Headless 模式关键时序

项目初始化时遵循以下固定顺序：
1. package.json → npm install：写完 package.json 后立即执行 npm install
2. git init → commit：首次 commit 前必须先 git init
3. 优先使用内置工具：list_directory 替代 ls，read_file 替代 cat，glob 替代 grep
4. 避免复合 shell 命令：&& / || / | / 2>&1 会触发整串 keying
5. 写大文件时拆分：超过 200 行的文件分多步（write_file + edit_file）
6. 每个里程碑验收时应确认产生了新代码而不是占位壳
```

---

## Phase 2：自主闭环

### 2.1 空图自播种（P0-1 完整实现）

基础存根在 Phase 1 完成，Phase 2 增强：

**触发条件**：`background.rs` 检测到 workgraph 为空（0 节点），且没有 `CODECODER_BG_TASK`。

**执行流程**：
1. 读取 AGENTS.md 获取使命描述
2. 构建系统 prompt（含：使命、项目根目录文件清单、工具列表）
3. 调用一次 LLM，要求生成 3-10 个里程碑
4. 输出格式要求：JSON 数组 `[{title, acceptance, deps: [id], command?}]`
5. 解析输出 → 写入 workgraph.json
6. 进入标准 milestone 推进循环

**容错**：
- LLM 输出不可解析 → 最多重试 2 次 → 失败报 `MissionSeedFail`（退出码 4）
- 生成的里程碑少于 2 个 → 重试 1 次 → 失败报 `MissionSeedFail`
- 依赖关系有环 → 丢弃含环的里程碑 → 重新编号

### 2.2 Plan headless（P0-2）

在 `agent.rs` 的 `handle_plan` 或 `Permission::Ask { key: "plan" }` 处增加：

```rust
if self.headless {
    // headless 模式：直接执行 plan 工具，不等待用户确认
    let plan = self.toolbox.run("plan", args, &mut ctx)?;
    return ToolOutcome::Result(plan);
}
```

### 2.3 Write_file 截断防御（P0-3）

**方案：max_tokens 预算拆分 + 分块写入**

在 `write_file` 工具执行前，工具本身不修改。而是在 agent 的系统 prompt 中加入约束：

```
当你需要写一个预计超过 150 行的文件时：
1. 第一轮：write_file 写前 150 行（create 模式）
2. 后续轮次：edit_file 逐段追加（append 模式），每段 50-100 行
3. 确保每轮的 max_tokens 预留给文件内容而非推理
```

这本质上是让 AI 生成的内容被确定性工具分步写入——符合"AI 理解意图+确定性工具执行"的模式。

**辅助实现**：`write_file` 工具的 schema 增加 `append: bool` 参数：

```json
{
  "type": "object",
  "properties": {
    "file_path": { "type": "string" },
    "content": { "type": "string" },
    "append": { "type": "boolean", "default": false, "description": "追加模式而非覆盖" }
  },
  "required": ["file_path", "content"]
}
```

`run()` 方法中：`append=true` 时用 `OpenOptions::new().append(true)` 而非 `write()`。

### 2.4 检查点 + 检查点恢复（P1-3）

#### 设计

每个 milestone 验收通过后，自动保存 session checkpoint：

```
.ccd/                              ← gitignore
├── checkpoints/
│   ├── m1.json                    ← M1 完成时的 session 摘要
│   ├── m3.json                    ← M3 完成时的 session 摘要
│   └── latest.json                ← 始终指向最新 checkpoint（symlink 或副本）
├── bg.ndjson                      ← 当前运行的 BgObserver 事件流
└── alert.json                     ← 告警标志（详见 Phase 4）
```

**checkpoint 格式：**

```json
{
  "format_version": 1,
  "milestone_id": 3,
  "milestone_title": "公共组件库",
  "completed_milestones": [1, 2, 3],
  "pending_milestones": [4, 5, 6, 7, 8, 9, 10],
  "files_touched": ["src/router.tsx", "src/components/RichTextEditor.tsx", "src/components/ApprovalPanel.tsx"],
  "architecture_summary": "React 18 + TypeScript + Vite + React Router。路由在 router.tsx 中集中定义，共享组件在 src/components/ 下。布局在 src/layouts/。",
  "token_summary": {
    "cumulative_prompt": 115420,
    "cumulative_completion": 32110,
    "cumulative_cost_estimate_usd": 0.29
  },
  "session_summary": [
    {"message_id": 15, "abstract": "agent 决定使用 Ant Design 作为 UI 组件库"},
    {"message_id": 32, "abstract": "router.tsx 定义了 6 大域的路由结构"}
  ],
  "knowledge_bits": [
    "技术栈：React 18 + TypeScript + Vite + Ant Design + Zustand + React Router",
    "路由架构：domainRoute() 工厂函数，每域有独立的 layout",
    "组件库：8 个共享组件在 src/components/，通过 barrel (index.ts) 导出"
  ]
}
```

**检查点写入时机：**

```
run_background_cfg 循环中：
  milestone pass →
    save_checkpoint(root, &g)     // 写入 .ccd/checkpoints/m<N>.json + latest.json
  milestone needs_fix →
    不写入（没有新进展）
  exit →
    save_checkpoint(root, &g)     // 最后写一次，方便 resume
```

**检查点恢复：**

```
run_background_cfg 启动时：
  if workgraph 有未完成里程碑 then
      let cp = find_latest_checkpoint(root)
      if cp 存在 then
          inject checkpoint context into system prompt：
            "此前已完成里程碑：1/2/3
             累积 token：147k
             架构理解：[knowledge_bits]
             已完成文件：[files_touched]
             待完成里程碑：4/5/6"
      else
           无 checkpoint → 从零开始（读文件目录 + 已有源代码）
```

**检查点失效与清理：**
- checkpoints 目录文件数量无限制（每个 checkpoint ~2-10KB，100 个里程碑不到 1MB）
- 无自动清理（废弃的 checkpoint 不会影响当前运行，仅 latest.json 用于恢复）
- 首次运行该目录不存在时静默创建

#### 实现注意点

- **序列化安全**：`checkpoint.json` 不应包含敏感信息（API keys、PII）
- **读取兼容**：`format_version` 字段实现前向兼容
- **写入时机**：在 milestone 通过验收后、`with_lock` 已释放时写入（避免持有锁时做文件 I/O）
- **可能的数据竞争**：多个背景 agent 写到同一目录的检查点——接受"最后一个 wins"的语义，因为 milestone 本身有顺序依赖

### 2.5 Token 消耗可见性（P1-4）

`.ccd.bg.ndjson` 事件流增加：

```json
{"kind":"llm_call","msg":"tokens","prompt_tokens":4521,"completion_tokens":1283,
 "cumulative_prompt":115420,"cumulative_completion":32110}
```

退出时打印语句：

```
[bg] token report: 115,420 prompt + 32,110 completion = 147,530 total
[bg] cost estimate: $0.29 (deepseek-v4-flash @ $0.42/M prompt, $1.32/M completion)
```

可选的 `CODECODER_BG_TOKEN_BUDGET` 环境变量：超过此预算的 prompt+completion 总 Token 时自动 exit 5（BudgetExhausted），防止无限制消耗。

### 2.6 跨里程碑 Bug 修复（P1-1）

`build_repair_prompt()` 改进：

```
当前注入：
  本轮 build 失败原因：[编译错误原文]

改进后注入：
  本轮 build 失败原因：[编译错误原文]
  该文件属于里程碑：[milestone_title]（最初在 milestone #[id] 中被创建）
  受影响文件列表：[该 milestone touch 过的所有文件]
  搜索建议：语法错误可能也存在于以下文件中 [同类型文件列表]
```

通过 `workgraph.json` 的 `touched` 字段和文件创建时间戳获取归属信息。

---

## Phase 3：宏观扩展

### 3.1 全栈架构推理（Prompt 层）

不修改 codecoder 内核。通过`AGENTS.md`和workgraph的设计来实现。

**seed prompt 增加全栈检查：**

```
在开始工作前，请分析项目的需求文档（功能清单.md），回答以下问题：
1. 这个项目需要数据持久化吗？（表单输入、列表展示、状态流转 -> 需要后端 API + 数据库）
2. 需要用户认证吗？（用户管理、角色权限 -> 需要后端）
3. 现有文档描述的是前端还是全栈？
4. 如果判断需要后端，你的技术栈建议是什么？

将分析结果写入 .ccd/architecture-scope.md。
```

**配套 workgraph 设计：** 在首个里程碑（脚手架）前插入一个"需求分析"阶段：

```
启动时 detect 需求文档存在 →
  首 turn 让 agent 读文档 + 输出架构分析 →
  根据分析结果创建适当的里程碑（前端/后端/全栈）
```

### 3.2 多项目编排（外部脚本）

未修改 codecoder 二进制。外部 shell 脚本：

```bash
#!/bin/bash
# codecoder-scheduler — 多项目自主构建编排器

PROJECTS="$1"  # JSON: [{"name":"smcs","root":"~/Code/smcs","agents":"AGENTS.md","workgraph":"workgraph.json"}]

for project in $(echo "$PROJECTS" | jq -c '.[]'); do
    name=$(echo "$project" | jq -r '.name')
    root=$(echo "$project" | jq -r '.root')
    agents=$(echo "$project" | jq -r '.agents')
    workgraph=$(echo "$project" | jq -r '.workgraph')
    
    echo "[scheduler] === Starting project: $name ==="
    cd "$root"
    CODECODER_ROOT="$root" \
    CODECODER_BG_WORKGRAPH=1 \
    ./codecoder
    
    exit_code=$?
    echo "[scheduler] $name → exit $exit_code"
done
```

### 3.3 维护模式（`CODECODER_BG_TASK`）

`CODECODER_BG_TASK` 当前只跑一个 turn。增强：

```
CODECODER_BG_TASK="修复登录页404"
  →
  1. agent 读 CONTEXT.md + 主要架构文件（理解项目结构）
  2. 读当前 session 历史（如果有）
  3. 定位 bug（read_file、grep、glob）
  4. 修复（edit_file）
  5. 验证（npm run build / cargo test）
  6. git commit
```

不在 workgraph 模式下运行，是独立的一次性任务流。关键变化：agent 需要理解"既有代码"而非"空白项目"。

---

## Phase 4：守夜进化

### 4.1 熔断降级（P2-2）

```
连续 N 个 milestone needs_fix（consecutive_fail >= bg_circuit_k）时：
  1. 检查当前卡住的 milestone 是否有替代方案
     - a) 如果跳过本 milestone（标记 blocked_skip）不影响后续 → 跳过
     - b) 如果跳过会阻塞全部下游 → 记录信息后退出
  2. 被跳过的 milestone 记录到 .ccd/skipped.json
  3. 继续推进下一个就绪 milestone
  4. 当所有就绪 milestone 都被跳过时 → exit 6 (AllSkipped)
```

实现：在 `WorkGraph::set_status()` 中增加 `BlockedSkip` 状态。

### 4.2 跨会话记忆

`memory/` 目录中积累经验：

```json
{
  "name": "bug-patterns",
  "entries": [
    {"pattern": "未闭合的 JSX 标签", "detection": "grep -n '<[A-Z]' src/ | grep -v '/>'", "fix": "补全 />"},
    {"pattern": "Ant Design Modal 标签未闭合", "detection": "grep -n '<Modal' src/ | grep -v '</Modal'"},
  ]
}
```

LLM 在首个 turn 时读取 `memory/` 并注入系统 prompt。每次修复成功的新 bug 模式自动追加。

### 4.3 远程告警

写 `.ccd/alert.json`：

```json
{"level": "warn", "message": "stuck on milestone #5 after 3 fix attempts", "ts": 1712345678000}
```

外部监控工具（cron、systemd timer）检测到此文件存在 → 执行 webhook / 邮件发送。处理后脚本删除文件。

codecoder 自身不发出 HTTP 请求——保持纯本地设计。这是与"AI+确定性工具"一致的选择：AI 决定何时需要告警（写 alert.json），确定性的外部工具负责发送。

### 4.4 自我改进循环（远期）

最高级能力。codecoder 在空闲时分析 `.ccd.bg.ndjson` 事件流，识别模式：

```
分析结果示例：
- 最近 1000 次工具调用中 30% 是 read_file
- 根因：每轮不知道已读过的文件内容
- 改进：在 memory/archived-decisions.md 中记录文件结构
- 改善行动：写 skills/remember-file-map.md，下次自动应用
```

---

## 冲突处理原则

Fowler 的观点中强调"人在回路中验证"、"方向盘必须握在人手里"。这与 codecoder "自主完成"的定位存在根本冲突。处理原则：

1. **人在回路中的"回路"是 setup 时** — 人在启动 codecoder 前做好配置（`codecoder.json`、`AGENTS.md`、`workgraph.json`），而不是运行时
2. **小步快跑中的"快跑"是 codecoder 的工作** — 每次提交/验收都是一个小扇区，但由 codecoder 自主完成，不需要人每步确认
3. **方向盘握在人手里体现在终止条件** — 人通过设置 `bg_circuit_k`、`bg_max_fix_attempts`、`CODECODER_BG_TOKEN_BUDGET` 来定义边界。出界则终止
4. **人的判断在事后** — 就像敏捷的回顾会议，人检查退出码、检查 checkpoint、检查验收结果，决定"继续/回退/重跑"

---

## 交付顺序与依赖关系

```
Phase 1 ───────────────────────────────────
  │  1.1 验收门 checks 字段（bg_gate.rs）
  │  1.2 权限平滑（agent.rs + trust.rs）
  │  1.3 P0-1 空图自播种存根
  │  1.4 P0-2、P0-3、P0-4 基础修复
  │  1.5 Tool cap 调整（config.rs）
  │  1.6 循环兜底（background.rs）
  │  1.7 行为约束文档
  │  1.8 回归测试 + 集成测试
  │
  ▼  ← 全部完成，可跑通 smcs 验收无 PlaceholderPage 壳
  │
Phase 2 ───────────────────────────────────
  │  2.1 空图自播种完成
  │  2.2 Write_file append 模式
  │  2.3 Checkpoint/resume 系统
  │  2.4 Token 统计
  │  2.5 跨里程碑修复增强
  │
  ▼  ← 全部完成，停机能恢复不丢上下文
  │
Phase 3 ───────────────────────────────────
  │  3.1 全栈推理 prompt（文档级，不改 codecoder）
  │  3.2 外部调度器脚本
  │  3.3 CODECODER_BG_TASK 增强
  │
  ▼  ← 全部完成，能执行多项目和已有代码库维护
  │
Phase 4 ───────────────────────────────────
  │  4.1 熔断降级（BlockedSkip）
  │  4.2 跨会话记忆
  │  4.3 告警标志
  │  4.4 自我改进循环
```

---

## 附录 A：smcs 实验新发现的差距映射

| 新发现问题 | 对应设计项 | 阶段 |
|-----------|-----------|------|
| 验收语义鸿沟（PlaceholderPage 冒充） | 1.1 checks 门 — `no_template_content` 类型 | Phase 1 |
| 权限三关卡体验差 | 1.2 有 codecoder.json 自动 trust | Phase 1 |
| 15-tool cap vs review 门 | 1.4 bg_milestone_tool_cap 默认 8→15 | Phase 1 |
| 需求理解深度不足（只看前端） | 3.1 全栈推理 prompt | Phase 3 |
| C2 动态拆分机制生效 ✅ | 验证已有功能，无修复必要 | 无需改动 |

## 附录 B：与现有 ADR 的映射

| 设计项 | 相关 ADR | 关系 |
|--------|---------|------|
| checks 验收门 | ADR 0030 + 0039 | 扩展，非冲突 |
| 权限自动 trust | ADR 0028 | 扩展 headless 分支 |
| 复合命令通配符 | ADR 0036 | 扩展，受限的场景 |
| Write_file append | ADR 0037 | 互补（0037 处理 output 截断） |
| 检查点/resume | ADR 0023 (compaction) | 互补（不同的持久化维度） |
| 外部调度 | ADR 0026 | 一致不做内建 |
