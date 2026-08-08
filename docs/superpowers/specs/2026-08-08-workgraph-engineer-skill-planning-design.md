# 设计：workgraph 里程碑的 engineer-skill 计划化 + 外循环质量闭环

## 背景与目标

workgraph（`workgraph.json`）是 CodeCoder 的"事前构造之图"——一个依赖有序的里程碑节点图。当前 `advance_one_milestone`（`src/background.rs`）推进一个就绪里程碑时，只注入一句 "Complete this milestone. When done, it will be marked complete automatically"，然后让 agent 自报完成。里程碑既没有详细的开发计划，也没有验收标准，质量完全依赖 agent 的自觉。

本设计的目标：

1. **每个里程碑先出详细开发计划再执行**——自动选择合适的 engineer* 系列 skill，按其方法论生成带验收标准、文件范围、风险点的开发计划，持久化到磁盘。
2. **复用 engineer* skill 生态**——不引入 engineer-workflow（太重，自带子里程碑拆解，与 workgraph 节点结构冲突），而是根据里程碑内容自动选择更合适的 skill（architect / frontend-architect / legacy-recon / qa / inspector / coach / requirements）。
3. **外循环质量闭环**——所有里程碑完成后，LLM 做整体质量检查；若发现质量问题，基于实际进展**增量补充** workgraph（保留已完成节点，不重做），重新执行。直到 LLM 判断"无需再生成，所有目标已高质量完成"。
4. 覆盖 **Background（headless）与交互式（ccli）** 两种模式。

## 关键决策

| 决策点 | 选择 | 理由 |
|--------|------|------|
| skill 选择方式 | agent 根据里程碑内容**自动选择** | 里程碑性质各异，无单一 skill 适用；agent 自主判断最灵活 |
| engineer-workflow 是否使用 | **不用** | 自带子里程碑拆解，与 workgraph 节点结构冲突 |
| 计划持久化路径 | `.codecoder/milestone-plans/N-plan.json` | 一律在 `.codecoder/` 下，与配置同目录，可追溯、可恢复 |
| 里程碑执行阶段 | **Plan Turn + Exec Turn 两阶段** | 上下文干净，计划与执行分离，engineer skill 完整发挥 |
| 质量门禁 | **纯依赖 engineer skill 方法论自验收**，不加代码级命令门 | 用户明确选择；客观命令门曾引入又被移除（Task 4），依赖 skill 验收更轻 |
| 外循环终止 | LLM 自主判断；≤3 轮自动，超过暂停询问 | `bg_max_auto_cycles` 默认 3，可在 `codecoder.json` 配置 |
| 重建语义 | **增量补充**：保留已完成节点，追加新节点 | 不重做已高质量完成的内容 |

## 架构

```
workgraph 里程碑节点
  └── 每个里程碑（advance_one_milestone）:
        [Plan Turn]  use_skill 选 engineer skill → 生成计划 → .codecoder/milestone-plans/N-plan.json
        [Exec Turn]  读取/注入计划 → 编码 → 逐条自验收 → 标记 Done
  → 所有里程碑完成（无 Pending 节点）
  → [Quality Review Turn] 读所有已完成 plan + touched → LLM 判断
        ├── 有质量问题 → generate_milestones 增量补充 → 回到 milestone loop
        └── 目标已高质量完成 → 终止
  → 外循环计数 ≤ bg_max_auto_cycles（默认 3），超过暂停询问
```

## 模块一：计划持久化结构

### 路径

```
.codecoder/
├── milestone-plans/
│   ├── N-plan.json        # 里程碑 #N 的开发计划
│   └── ...
└── codecoder.json         # 已有配置
```

### `N-plan.json` 结构

```json
{
  "milestone_id": 1,
  "title": "实现数据模型",
  "skill_used": "engineer-architect",
  "created_at": "2026-08-08T10:00:00Z",
  "acceptance_criteria": [
    "使用词汇表定义的术语命名",
    "所有字段有类型注解",
    "数据库迁移脚本可回滚"
  ],
  "scope": {
    "files_to_create": ["src/models/user.rs", "src/models/article.rs"],
    "files_to_modify": [],
    "estimated_lines": 150
  },
  "risks": ["数据模型变更可能影响后续里程碑"],
  "test_requirements": "每个模型至少 1 happy + 2 edge case 测试"
}
```

JSON 格式让执行 turn 能程序化读取验收标准逐条检查，也让外循环 review turn 能客观评估完成质量。

## 模块二：Plan Turn — 里程碑计划生成

### 流程

```
advance_one_milestone 检测到里程碑 #N 就绪
  └── 检查 .codecoder/milestone-plans/N-plan.json 是否存在
       ├── 存在 → 跳过 Plan Turn，直接进入 Exec Turn（支持中断恢复）
       └── 不存在 → 进入 Plan Turn
```

### Plan Turn 的 prompt 构造

```
workgraph milestone #N: <title>

任务分解：
1. 先用 `use_skill` 工具加载与当前里程碑内容最匹配的 engineer skill
   - 选择依据：里程碑标题/描述中的关键词
   - 架构/数据模型 → engineer-architect
   - 前端/UI → engineer-frontend-architect
   - 遗留代码改造 → engineer-legacy-recon
   - 测试/验收 → engineer-qa / engineer-inspector
   - 通用编码 → engineer-coach
   - 需求模糊 → engineer-requirements
2. 按照所选 skill 的方法论，生成详细的开发计划
3. 将计划写入 .codecoder/milestone-plans/N-plan.json
4. 完成后标记该节点就绪（不执行编码）
```

### 关键设计决策

- **Plan Turn 只做计划，不执行编码**——上下文干净，skill 方法论完整发挥
- **Plan Turn 完成后，里程碑仍保持 Pending 状态**——由 Exec Turn 推进
- **Plan 文件存在即跳过**——支持中断恢复，不会重复生成

### 交互式模式的处理

`ccli` 中，Plan Turn 执行后，agent 展示计划摘要并询问是否继续：

```
里程碑 #N 开发计划已生成：
- 使用的 skill: engineer-architect
- 创建的验收标准: 3 条
- 预计变更: 150 行，2 个文件

是否按此计划开始执行？[y/n]
```

## 模块三：Exec Turn — 里程碑执行

### 流程

```
Plan Turn 完成后（或 plan 已存在时）→ 进入 Exec Turn
  └── 读取 .codecoder/milestone-plans/N-plan.json
  └── 注入计划关键信息到 prompt
  └── agent 按计划逐项编码
  └── 逐条对照验收标准自验收
  └── 通过 → 标记 Done【保留现有自报完成机制】
```

### Exec Turn 的 prompt 构造

```
workgraph milestone #N: <title>

已为你生成开发计划（.codecoder/milestone-plans/N-plan.json）：

验收标准：
1. <标准1>
2. <标准2>

文件范围：
- 创建: <files_to_create>
- 修改: <files_to_modify>

风险点：
- <风险1>

请按此计划逐项执行。完成每一项后用 `diff` 或检查确认，逐条对照验收标准自验收。
全部通过后，里程碑将被自动标记为完成。
```

### 关键设计决策

- **保留现有 `advance_one_milestone` 的自报完成机制**——不从代码层面强制验收，符合"纯依赖 engineer skill 验收"
- **计划注入为结构化文本**——直接内联到 prompt，agent 无需再读文件，省一次工具调用
- **验收依赖 skill 方法论**——engineer-qa 等 skill 加载后，agent 会在执行中自行跑测试、查覆盖

## 模块四：外循环质量检查 + 增量补充

### 流程

```
所有里程碑完成后（无 Pending 节点）
  └── 进入 Quality Review Turn
       ├── 读取所有已完成里程碑的 plan + touched 文件
       ├── 注入质量评估指令（见下）
       └── LLM 判断：
            ├── 有质量问题 → 调用 generate_milestones 增量补充新里程碑
            │   └── 回到 milestone loop 继续执行
            └── 目标已高质量完成 → 终止
```

### Quality Review prompt

```
请综合评估整个项目当前的质量状态。
对照每个里程碑的验收标准，检查是否所有目标都已高质量完成。

如果存在质量问题（测试不足、实现不完整、代码质量不达标等），
请调用 generate_milestones 工具增量补充新的里程碑。
已完成的里程碑不可修改，但可追加新的里程碑来修复/增强。

如果认为所有目标都已高质量完成，就不需要再生成任何里程碑。
```

### 外循环计数器

```
.milestone_cycle = 0（每次 start 或 seed 时重置）

每轮外循环结束：
  cycle += 1
  if cycle >= config.bg_max_auto_cycles（默认 3）:
    如果是 Background 模式 → 暂停，设置退出码为 EXIT_NEEDS_REVIEW，记录到 BG observer
    如果是交互式模式 → 询问用户是否继续
```

### 增量补充的语义

- `generate_milestones` 的 context 参数中注入当前 workgraph 状态（已完成/进行中/阻塞的节点）
- 新生成的里程碑**自动追加依赖到已有节点**（如依赖所有已完成的节点，或精确依赖相关节点）
- 已完成的节点不会被修改，它们的 plan 文件保留

### 与 Background 模式的集成

当前 `background.rs` 的 milestone loop：

```rust
while let Some(out) = advance_one_milestone(...)? {
    // 每轮处理一个里程碑
}
// 里程碑全部跑完 → 退出
```

改为：

```rust
while cycle < max_cycles {
    // 执行现有的 milestone loop
    while let Some(out) = advance_one_milestone(...)? { ... }

    // 里程碑全部跑完 → 质量检查 turn
    let needs_rework = run_quality_review_turn(...)?;
    if !needs_rework { break; }  // 高质量完成

    cycle += 1;
    // 自动下一轮（seed 已由 generate_milestones 在 turn 内完成）
}
```

## 模块五：配置项与集成要点

### 新增配置项

在 `codecoder.json` 中追加：

```json
{
  "bg_max_auto_cycles": 3,
  "milestone_plan_dir": ".codecoder/milestone-plans"
}
```

- `bg_max_auto_cycles`：外循环自动轮数上限，默认 3，超过则暂停询问
- 交互式模式下，`milestone` 工具增加 `plan` 子命令，手动触发计划生成

### 改动文件清单

| 文件 | 改动内容 |
|------|---------|
| `src/background.rs` | `advance_one_milestone` 改为双阶段（Plan/Exec）；主循环加外循环质量检查 |
| `src/workgraph.rs` | 无改动（不修改数据结构） |
| `src/config.rs` | 新增 `bg_max_auto_cycles` 配置项 |
| `src/agent.rs` | 无改动（但 prompt 构造会用到 plan 信息） |
| `src/tool/dev.rs`（milestone 工具） | 交互式模式下展示 plan 摘要 + 询问确认 |
| `src/tool/generate_milestones.rs` | 接收已有 workgraph 状态做增量补充 |

### 通知与错误处理

| 场景 | 处理 |
|------|------|
| Plan Turn 失败（skill 加载失败等） | 重试 1 次，失败后标记里程碑为 Blocked，记录原因 |
| Exec Turn 时 plan 文件损坏 | 重新触发 Plan Turn |
| 外循环质量检查 turn 超时 | 视为"需要人工检查"，暂停轮询 |
| 增量 seed 后新节点依赖旧节点 | 自动追加 dep 到所有已完成的里程碑 |

## 测试策略

- **单元测试**：`advance_one_milestone` 的 Plan/Exec 双阶段逻辑；plan 文件存在与否的分支；外循环计数器
- **行为测试**：`background.rs` 相关测试更新，验证双阶段流程与增量补充
- **L3 冒烟**（可选）：真实 LLM 下验证 Plan Turn 生成计划 + Exec Turn 执行

## 文档同步

- 更新 `ARCHITECTURE.md` 中 Background Agent 部分（双阶段 + 外循环）
- 更新 `README.md` 相关数字（如有）
- 新增 ADR（如 `0041-workgraph-engineer-skill-planning.md`）