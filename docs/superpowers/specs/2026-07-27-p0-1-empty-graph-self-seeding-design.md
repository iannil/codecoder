# P0-1: 空 workgraph 自动建里程碑 — 设计文档

> 7×24 高度自主开发差距 P0-1：当 `workgraph.json` 为空时，自动读取 AGENTS.md 理解使命，调用 `generate_milestones` 工具分解为里程碑，写入后开始推进。

---

## 现状

`src/background.rs` 第 168-173 行：空 workgraph 立即以 `MissionState::EmptyGraph` 退出码 5 返回，不做任何自主建图尝试。

```rust
if graph.nodes.is_empty() {
    obs.emit("empty", "empty workgraph — nothing to advance; seed workgraph.json first");
    out.mission_state = crate::bg_gate::MissionState::EmptyGraph;
    return Ok(out);
}
```

## 设计

### 流程图

```
run_background_cfg() 进入 workgraph 分支
    │
    ▼
WorkGraph::read_checked() 成功
    │
    ▼
nodes.is_empty()?
    ├── No → 进入正常 milestone 推进循环（已有行为）
    │
    └── Yes → seed_workgraph_from_mission()
                    │
                    ▼
             读取 AGENTS.md（若存在）
                    │
                    ▼
             构造 seed prompt
                    │
                    ▼
             新建 AgentLoop（headless）
                    │
                    ▼
             运行一个 turn，LLM 使用 generate_milestones 工具
                    │
                    ▼
             检查 workgraph.json
                    ├── 有里程碑写入 → 进入正常推进循环
                    │
                    └── 仍为空 / 超时 → 回退 EmptyGraph
```

### 核心新增函数

在 `src/background.rs` 中新增：

```rust
/// 空 workgraph 时，通过 agent turn 调用 generate_milestones 工具
/// 自动分解使命为里程碑。成功写入后返回 true，失败回退返回 false。
fn seed_workgraph_from_mission(
    provider: Arc<dyn Provider>,
    model: String,
    max_tokens: u32,
    temperature: f32,
    root: PathBuf,
) -> bool {
    // 1. 读取 AGENTS.md
    let mission = read_mission(&root);
    
    // 2. 构造 seed prompt
    let prompt = format!(
        "你是一个项目规划助手。当前项目是一个空目录，需要你来初始化。

项目使命：
{}

请先使用 list_directory 工具了解项目结构，然后使用 generate_milestones 工具
将上述使命分解为 3-8 个里程碑，每个里程碑包含：
- title（简短、可行动的标题）
- acceptance（具体、可验证的验收标准，尽量包含可执行的命令如 cargo test）

里程碑应按依赖顺序排列，前面的里程碑是后面里程碑的前提。",
        mission
    );
    
    // 3. 新建 headless agent
    let mut agent = AgentLoop::new_background(...);
    // 4. 运行一个 turn
    agent.run_one_turn(prompt, &tx);
    // 5. 检查 workgraph.json 是否已写入节点
    let g = WorkGraph::read(&root);
    !g.nodes.is_empty()
}

/// 读取项目使命描述。优先 AGENTS.md，否则返回通用描述。
fn read_mission(root: &Path) -> String { ... }
```

### 关键决策

| 决策点 | 选择 | 理由 |
|--------|------|------|
| seed 用 agent turn 还是直接调 provider | agent turn（复用 `generate_milestones` 工具） | 减少维护两份解析逻辑；工具本身经过测试；agent 能自然使用工具 |
| seed 后的推进路径 | 复用现有 milestone 循环 | 无需新增循环逻辑，seed 成功后进入已有 `loop { advance_one_milestone ... }` |
| seed prompt 长度控制 | 短 prompt + 让 LLM 通过 list_directory 探索 | 遵循已有交互模式；避免一次性注入过多信息 |
| AGENTS.md 缺失时行为 | 降级为通用 prompt | 不阻塞；即使无 AGENTS.md 也能生成合理里程碑 |
| generate_milestones 失败时行为 | 回退 EmptyGraph 退出码 | 安全失败；避免在错误分解上浪费 token |
| seed turn 的 tool cap | 使用已有 `bg_milestone_tool_cap` 配置 | 和正常 milestone 一致，防止无限循环 |

### seed prompt 设计

seed prompt 的设计目标是：引导 LLM **先探索、再规划、再写入**。不直接告诉它要写什么里程碑，而是让它自主完成「理解项目 → 规划 → 执行」的闭环。

```
你是一个项目规划助手。当前项目是一个空目录，需要你来初始化。

项目使命：
{mission}

请先使用 list_directory 工具了解项目结构，然后使用 generate_milestones 工具
将上述使命分解为 3-8 个里程碑，每个里程碑包含：
- title（简短、可行动的标题）
- acceptance（具体、可验证的验收标准，尽量包含可执行的命令如 cargo test）

里程碑应按依赖顺序排列，前面的里程碑是后面里程碑的前提。
```

### 新增的模块级函数

```rust
/// src/background.rs 新增
fn seed_workgraph_from_mission(...) -> bool
fn read_mission(root: &Path) -> String
```

### 修改点汇总

| 文件 | 变更 | 类型 |
|------|------|------|
| `src/background.rs` | `run_background_cfg()` 空图路径改为调用 `seed_workgraph_from_mission` | 修改 |
| `src/background.rs` | 新增 `seed_workgraph_from_mission()` 函数 | 新增 |
| `src/background.rs` | 新增 `read_mission()` 函数 | 新增 |
| `src/workgraph.rs` | 无变更（`generate_milestones` 工具已通过 `WorkGraph::with_lock` 写入） | — |

### 验收标准

1. **空目录 + AGENTS.md 描述使命 → 自动生成 3-8 个里程碑并开始推进**
   - 测试：在临时目录中创建 AGENTS.md，运行 `run_background_cfg`（workgraph 模式），验证 workgraph.json 被写入且节点数 ≥ 1，mission_state 非 EmptyGraph

2. **AGENTS.md 不存在 → 降级为通用分解，不崩溃**
   - 测试：空临时目录，无 AGENTS.md，运行 seed，验证不 panic，返回 false 或 EmptyGraph

3. **LLM 不调用 generate_milestones → 超时后回退 EmptyGraph**
   - 测试：用 StubClient（固定响应，不调用工具），验证 seed 返回 false，mission_state 为 EmptyGraph

4. **已有非空 workgraph → 行为不变**
   - 测试：已有 workgraph.json（含节点），运行 workgraph 模式，验证不走 seed 路径

### 边界情况

- AGENTS.md 内容为空 → 降级为通用 prompt
- generate_milestones 调用成功但 write_file 阶段因 provider 截断 → 写不完整 → 后续 agent turn 的 `advance_one_milestone` 正常推进即可（已有截断容忍逻辑）
- seed turn 的 agent 进程被 cancel（SIGINT） → cancel token 机制保护，seed 失败，回退 EmptyGraph

---

## 实现计划

### Step 1: 在 background.rs 中新增 `read_mission()` 函数

- 读取 `<root>/AGENTS.md`，若存在返回其内容
- 若不存在，返回通用描述 `"Initialize and develop the project in this directory"`
- 单元测试：有 AGENTS.md / 无 AGENTS.md / AGENTS.md 为空

### Step 2: 在 background.rs 中新增 `seed_workgraph_from_mission()` 函数

- 接收 provider/model/max_tokens/temperature/root
- 构造 seed prompt（含 mission 信息）
- 新建 AgentLoop::new_background()
- 运行一个 turn
- 读取 workgraph.json 检查是否有节点写入
- 返回 bool（成功/失败）
- 注意：不注册 SIGINT（避免与已有 milestone 循环的 cancel token 冲突）

### Step 3: 修改 `run_background_cfg()` 的空图分支

- 原 `EmptyGraph` 直接 return 改为调用 `seed_workgraph_from_mission()`
- 成功 → 清空 `out` 状态，重新进入 milestone 循环
- 失败 → 仍返回 `EmptyGraph`

### Step 4: 单元测试

- `seed_workgraph_from_mission_success` — StubClient 配合模拟 generate_milestones 写入
- `seed_workgraph_from_mission_failure` — StubClient 不调用工具，验证返回 false
- `seed_workgraph_from_mission_no_agents_dot_md` — 无 AGENTS.md，验证降级
- `workgraph_self_seeds_and_advances` — 集成测试：空图 → seed → 推进