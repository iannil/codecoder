# CodeCoder 能力探索计划

> 全面探索 CodeCoder 自主 AI agent 系统的内置能力，编译启动后从浅到深实际运行所有能力并记录结果。

## 探索策略

**广度优先 → 深度优先**：先完整走一遍所有能力，确保每个能力都触达过，再回头深入每个能力的最复杂场景，最后探索架构极限与边界。

## 阶段 1 — 全能力扫描

目标：启动 CodeCoder，依次调用 26 个内置工具，每个至少一次成功执行，建立"能力基线"。

### 覆盖范围

| 工具类别 | 工具清单 | 验证方式 |
|---------|---------|---------|
| 文件操作 | `read_file`, `write_file`, `edit_file`, `list_directory` | 读写临时文件，验证内容正确 |
| 搜索 | `glob`, `grep`（含 AST） | 用 glob 和 grep 搜索项目自身源码 |
| 执行 | `run_command` | 运行简单 shell 命令，验证 stdout/stderr 捕获 |
| 差异比较 | `diff` | 对比两个文件/版本 |
| 联网 | `search_web`, `search_github`, `reverse_api` | 抓取一个网页，搜索 GitHub 仓库，逆向 API |
| 自我进化 | `generate_skill`, `generate_prompt`, `promote_prompt`, `use_skill`, `generate_capability`, `run_capability` | 写一个 skill，写一个 prompt，promote 它，激活 skill，写一个 capability 并执行 |
| 委派/交互 | `agent`, `review`, `ask_user`, `confirm` | 子 agent 执行子任务，review 代码审查，确认交互 |
| 工作/推理 | `plan`, `milestone`, `memory`, `reason` | 创建计划，构建里程碑，读写记忆，推理树操作 |
| Git | `commit` | 提交一个变更 |

### 预期产出

每个工具的成功调用记录——截图/日志、是否有任何工具出错或异常。

## 阶段 2 — 深度压测

选取 8 个关键能力做极限测试，每个都做复杂场景而非简单调用。

### 2.1 自我进化闭环（Skill → Prompt → Capability）

- **Skill 复杂场景**：写一个多步骤的 skill（如"代码审查 skill"），包含条件分支、checklist、输出格式模板。注入后验证 agent 行为确实改变。
- **Prompt 草稿→转正**：写一个 prompt，用 `use_skill` 按 fallback 激活（prompts/ 优先级低于 skills/），然后 `promote_prompt` 转正，验证撞名保护。
- **Capability 全排列**：写 3 个 capability：
  - `Shell + OneShot`：一个 shell 脚本
  - `Wasm + OneShot`：一个 `.wat` 文件，在 wasmtime 中运行
  - `Docker + OneShot`：一个 Dockerfile 容器中运行程序
  - 验证权限闸门（`run_capability` 的 `Ask` 提示 + 天花板规则）

### 2.2 工作图（Work Graph）深度

- 构建一个 5 个里程碑的依赖图（如 A→B→C, A→D→E）
- 用 `milestone add` 逐个添加，设置依赖链
- 用 `plan` 工具审批
- 推进工作图，验证 `next_ready()` 调度正确性
- 中途标记 `needs_fix`，验证阻塞和重试
- 完成全部里程碑

### 2.3 推理树（Inference Tree）深度

- 用 `reason add` 构建一个 4-5 节点的因果树
- 设置 `margin`、`leverage`、`terminal` 元数据
- 用 `reason list` 和 `reason trace` 检索
- 验证跨 session 持久化（退出重进后仍可访问）
- 验证"推理树→工作图"闭环（把高 margin 节点转为 milestone）

### 2.4 审查裁决（Review Verdict）深度

- 写一段包含"架构漂移"的代码：修改基础类型签名、引入不必要的抽象、术语不一致
- 用 `review` 工具审查
- 验证 4 信号检测（foundation / over_engineering / volume / terminology）
- 验证 `foundation` fail 强制 `rebuild` 裁决
- 验证子 agent 输出的 unparsed 回退

### 2.5 子 agent 深度嵌套

- 用一个任务调用子 agent，子 agent 内再尝试调用工具
- 验证子 agent 深度锁 1（不能递归 spawn 子 agent）
- 验证子 agent 只能使用 `Permission::None` 工具的 9 个工具
- 验证子 agent 无用户通道（不能 `ask_user`）

### 2.6 Background Agent 双模式

- **显式 task 模式**：`CODECODER_BG_TASK="列出项目文件结构"` 运行，验证 BgOutcome 和退出码
- **Workgraph 模式**：先构建一个工作图，然后 `CODECODER_BG_WORKGRAPH=1` 运行，验证自动推进里程碑
- 检查 `bg_ledger.jsonl` 账本记录

### 2.7 Daemon 多 client 并发

- 启动 daemon，同时连接两个 cc 客户端
- 验证 socket 复用
- 验证 session 隔离

### 2.8 权限系统深度

- 测试 PermissionKey 细粒度（`run_command:git` 的授权不适用 `run_command:rm`）
- 测试 `AlwaysThisSession` vs `AlwaysThisProject` 持久化
- 测试 `run_capability` 天花板规则（Shell 能力上限 `AlwaysThisSession`）
- 测试复合命令 keying（`&&`/`||`/`;`/`|` 按整串）
- 测试 `codecoder.json` 预授权

## 阶段 3 — 架构极限与边界

### 3.1 Compaction 测试

- 构造一个长对话（大量工具调用和大文件输出），观察 compaction 触发
- 验证 tier-1 压缩（丢 Reasoning + 旧 ToolResult 占位化）
- 验证 anchor 保护（第一个用户目标不被压缩）
- 验证近端 tail 保护

### 3.2 工具输出截断

- 输出超大的文件/命令结果，验证 `CODECODER_MAX_TOOL_OUTPUT` 截断
- 验证截断 marker

### 3.3 Session 持久化

- 创建 session，退出，/resume 恢复
- 验证前向迁移链
- 验证迁移失败时原始文件不变

### 3.4 工具隔离

- 验证 Docker 缺失时显式报错（不降级到宿主）
- 验证 Wasm 无网络/限 FS
- 验证子 agent read-only 工具集

## 最终产出

一份结构化的探索报告，包含每个能力的状态、截图、发现的问题/边界，保存到 `docs/exploration/` 目录。