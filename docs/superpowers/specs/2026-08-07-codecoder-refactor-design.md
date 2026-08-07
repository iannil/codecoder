# CodeCoder 重构：二进制重命名、CLI 输出、三层配置、Workgraph 门禁重构

## 1. 动机

CodeCoder 自迭代 1 初始架构以来，经历多轮功能演进后，部分设计决策已不再适应当前用法：

1. **二进制名称**：当前 `codecoder`（daemon）、`cc`（client）、`cc-web`（web）在开发与发布中混淆——`codecoder` 作为库 crate 名与 daemon 二进制名冲突，`cc` 与系统命令冲突，`cc-web` 名不统一。
2. **CLI 可发现性**：`-h`/`--help` 输出过于简略，未结构化，LLM agent 无法自动解析命令用法和能力。
3. **配置管理**：`.ccd.env` 文件机制存在安全白名单限制（密钥/端点必须来自真实 shell），且不支持层级覆盖。用户与项目级配置无法分离。
4. **Workgraph 门禁**：每个里程碑的内嵌命令门和 review 门在推进过程中即时验收，导致验收逻辑与开发编排耦合，且无法后置统一验收。

## 2. 设计决策

### 2.1 二进制重命名

**决策**：统一命名 `ccda`（daemon）/ `ccli`（client）/ `ccweb`（web）。

**文件迁移**：
- `src/main.rs` → `src/bin/ccda.rs`（daemon 入口，默认启动 daemon）
- `src/bin/cc.rs` → `src/bin/ccli.rs`（client 入口，连 daemon 交互）
- `src/bin/cc-web.rs` → `src/bin/ccweb.rs`（web 入口，网页端）

**Cargo.toml**：
```toml
[[bin]]
name = "ccda"
path = "src/bin/ccda.rs"

[[bin]]
name = "ccli"
path = "src/bin/ccli.rs"

[[bin]]
name = "ccweb"
path = "src/bin/ccweb.rs"
```

`src/lib.rs` 保持公共库，外部测试通过 `use codecoder::*` 编译。

### 2.2 CLI 帮助与技能输出

**决策**：三个二进制统一支持 `-h`/`--help`（总览）和 `--skill <name>`（技能详情），默认纯文本 markdown，`--json` 可选结构化。

**`--help` 输出结构**（纯文本）：
- 名称与简介
- USAGE：基本用法
- MODES / SUBCOMMANDS：各模式/子命令说明
- CONFIGURATION：配置路径与说明
- EXAMPLES：常见使用示例
- 提示：`--skill <name>` 查看某技能详情，`--json` 获取结构化输出

**`--skill <name>` 输出结构**（纯文本，每个技能包含）：
- `description`：技能描述
- `usage`：调用示例
- `schema`：参数与输出结构说明
- `template`：可直接复用的模板

**`--json` 选项**：在上述命令后追加 `--json` 时，输出 JSON 结构化数据（`{"name": "...", "description": "...", "usage": [...], "schema": {...}, "template": "...", "skills": [...]}`）。

**`--skill` 查找顺序**：
1. 内置技能表（各二进制预定义的技能/模式/子命令）
2. 仓库 `skills/` 目录下的 `.md` 文件（动态扫描）

**内置技能表**（每个二进制独立）：

**ccda**：
- `daemon`：daemon 模式（默认）
- `bg-task`：headless one-shot 模式
- `bg-workgraph`：workgraph 逐里程碑模式
- `config`：配置说明与模板
- `recovery`：自动恢复模式

**ccli**：
- `send`：发送消息（one-shot）
- `repl`：交互式 REPL
- `ledger`：BG 任务账本
- `session`：会话管理
- `workgraph`：workgraph 状态
- `services`：持久化服务管理
- `autotask`：自动任务管理
- `health`：daemon 健康检查

**ccweb**：
- `server`：HTTP 服务器
- `config`：配置说明

### 2.3 三层 JSON 配置

**决策**：废弃所有 `CODECODER_*` 环境变量配置（除执行路由变量），改为三层 JSON 文件配置，逐层合并覆盖。

**配置层级**（后面的覆盖前面的）：
1. **内置默认值**：`Config::default()` 硬编码，同当前 `Config::from_env()` 的默认值（`model: "gpt-4o"`, `max_tokens: 8192` 等）
2. **用户级**：`$HOME/.codecoder/codecoder.json`（Unix）/ `$USERPROFILE\.codecoder\codecoder.json`（Windows）
3. **项目级**：`$PROJECT_ROOT/.codecoder/codecoder.json`（`CODECODER_ROOT` 或 CWD）

**合并规则**：逐字段合并，JSON 中的 `null` 字段表示"不覆盖"（继承下层值）。

**保留的 env 例外**（仅执行路由，不在 JSON 中）：
- `CODECODER_ROOT`：项目根目录（定位项目级配置所在）
- `CODECODER_DAEMON`：daemon 模式触发（已由二进制名替代，暂保留兼容）
- `CODECODER_BG_TASK`：headless one-shot 任务
- `CODECODER_BG_WORKGRAPH`：workgraph 模式
- `CODECODER_SCRIPT`：script provider 注入

**配置 JSON 结构**（完整字段列表，与当前 `Config` 结构体一致）：

```json
{
  // 必需
  "api_key": null,
  "model": "gpt-4o",
  "api_base": "https://api.openai.com/v1",

  // 推理参数
  "max_tokens": 8192,
  "max_tokens_ceiling": 32768,
  "temperature": 0.7,

  // 敏感凭证
  "github_token": null,

  // BG 参数
  "bg_max_auto": 0,
  "bg_circuit_k": 2,
  "bg_milestone_tool_cap": 15,
  "bg_max_fix_attempts": 3,

  // 运行时参数
  "supervisor_crash_budget": 3,
  "max_tool_output": 262144,
  "command_timeout_secs": 0,
  "compaction_tier2": true,
  "noop_nudge_threshold": 3,

  // 间隔/周期
  "wg_tick_secs": 30,
  "supervisor_tick_secs": 1,
  "ondemand_reaper_secs": 5,
  "auto_task_interval_secs": 300,
  "auto_task_source": "github_issues",

  // Provider 重试
  "provider_retry_max": 3,
  "provider_retry_initial_ms": 1000,
  "fallback_api_base": null,
  "fallback_model": null,

  // 告警
  "alert_webhook": null,
  "alert_on_failure_only": true,

  // 自恢复
  "daemon_auto_restart": false,
  "probe_failure_threshold": 5,

  // Workgraph
  "wg_auto_renew": true,

  // 存储限制
  "max_sessions": 100,
  "max_ledger_lines": 10000,

  // 诊断
  "self_observe": false
}
```

**API 变更**：
- 废弃：`Config::from_env()` → 保留作为兼容 shim，内部调用 `Config::load()`
- 新增：`Config::load()` → 读三层 JSON 合并返回
- 新增：`Config::merge_json(path)` → 从单文件读 JSON 合并到 `self`
- 新增：`Config::default()` → 返回默认值

**敏感项处理**：
- `api_key`、`github_token`、`alert_webhook` 等敏感字段在用户级和项目级 JSON 中存储
- 用户级 `$HOME/.codecoder/codecoder.json` 建议权限 `0600`
- 项目级 `.codecoder/codecoder.json` 建议权限 `0600`（gitignore）

**`.ccd.env` 废弃**：
- 删除 `autoload_ccd_env()` 和 `autoload_ccd_env_from()`
- 删除 `parse_dotenv()` 函数
- 删除 `DOTENV_ALLOWED_KEYS` 白名单
- 相关测试全部删除或迁移

### 2.4 Workgraph 门禁取消

**决策**：删除 milestone 推进过程中的命令门和 review 门，验收由独立的验收里程碑节点承载，agent 自报完成。

**移除的字段**：
- `Milestone.acceptance`：验收标准（不再用于门禁判定）
- `Milestone.checks`：检查列表
- `Milestone.command`：验收命令
- `Milestone.verdict`：评审结果

**移除的状态**：
- `NodeStatus::NeedsFix`：不再需要修复态
- 废弃 `Verdict` 枚举
- 废弃 `GateKind` 枚举

**移除的模块/函数**：
- `bg_gate.rs` 中全部函数：`evaluate`、`run_command_gate`、`extract_gate_command`、`gate_kind`、`gate_command`、`run_checks`、`execute_check`、`build_output_check`、`runtime_verify_html`、`next_action`
- `bg_gate.rs` 中全部枚举：`GateVerdict`、`NextAction`、`GateKind`
- `MissionState` 简化为：`Running`、`Completed`、`EmptyGraph`、`Error`。移除门禁专用状态 `BlockedAt`、`CircuitBreaker`、`StuckNeedsFix`

**保留的字段**：
- `Milestone.deps`：依赖关系，开发顺序约束
- `Milestone.title`：里程碑标题
- `Milestone.id`：标识
- `Milestone.touched`：touch 文件列表（agent 开发时记录改动范围，用于后续验收参考）
- `Milestone.fix_attempts`：保留但不再用于 `NeedsFix` 决策（可能用于 BG 的 turn 预算控制）

**保留的状态**：
- `NodeStatus::Pending`：待开发
- `NodeStatus::InProgress`：开发中
- `NodeStatus::Done`：agent 自报完成
- `NodeStatus::Blocked`：因依赖未完成而阻塞

**验收即里程碑**：
- 验收不再作为门禁嵌入开发里程碑，而是由 agent 在 workgraph 末尾添加验收里程碑节点
- 验收里程碑的 `title` 如 "验收: 统一测试"、"验收: code review"、"验收: 性能基准"
- 验收里程碑同样依赖开发里程碑，但不再有命令门/review 门
- agent 自报完成即算通过

**`engineer*`/`rc*` 技能**：
- 保留 `skills/` 目录下的 `engineer*` 和 `rc*` 系列技能
- 这些技能负责指导 agent 在 workgraph 末尾构造合适的验收里程碑
- workgraph 内核不关心验收的具体内容，只负责依赖编排

**BG 推进循环变更**：
- 当前 `background.rs` 中的 `run_background_cfg` 在每个 milestone 完成后调用 `evaluate` → 根据 verdict 决定 `next_action`（Pass/NeedsFix/CircuitBreaker）
- 改为：每个 milestone 完成后直接标记 `Done`，若此 milestone 有等待的 `pending` 依赖者，标记为 `InProgress` 并推进下一个
- 不再需要 `consecutive_fail` 计数、熔断逻辑、`NeedsFix` 重试

## 3. 影响范围

### 3.1 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/main.rs` | 删除 | 迁移到 `src/bin/ccda.rs` |
| `src/bin/ccda.rs` | 新建 | 从 `main.rs` 迁移 |
| `src/bin/cc.rs` | 删除 | 迁移到 `src/bin/ccli.rs` |
| `src/bin/ccli.rs` | 新建 | 从 `cc.rs` 迁移 + 增强 help/skill 输出 |
| `src/bin/cc-web.rs` | 删除 | 迁移到 `src/bin/ccweb.rs` |
| `src/bin/ccweb.rs` | 新建 | 从 `cc-web.rs` 迁移 + 增强 help/skill 输出 |
| `src/config.rs` | 重写 | 三层 JSON 配置加载，废弃 `.ccd.env` |
| `src/bg_gate.rs` | 删除 | 门禁逻辑全部移除 |
| `src/bg_ledger.rs` | 简化 | 移除 `MissionState` 相关逻辑 |
| `src/background.rs` | 修改 | 移除门禁调用，简化推进循环 |
| `src/workgraph.rs` | 修改 | 移除 `NeedsFix`/`Verdict`/`GateKind` 等验收相关字段和状态 |
| `src/lib.rs` | 修改 | 移除 `bg_gate` 模块导出，更新 `BgOutcome` |
| `Cargo.toml` | 修改 | 二进制重命名 |
| `ARCHITECTURE.md` | 更新 | 同步所有变更 |
| `README.md` | 更新 | 同步二进制名、配置方式、用法 |
| `CONTEXT.md` | 更新 | 移除 `.ccd.env` 相关术语 |
| `docs/adr/` | 更新 | 相关 ADR 修订（ADR 0028/0029/0033/0034 等） |

### 3.2 测试影响

- `bg_gate.rs` 的测试全部删除（~40 个测试）
- `config.rs` 的 `.ccd.env` 测试全部删除或迁移（~8 个测试）
- `background.rs` 中依赖门禁的测试需重构
- `workgraph.rs` 中依赖 `NeedsFix`/`Verdict` 的测试需重构

## 4. 实现顺序

4 个需求的实现顺序按依赖关系编排：

1. **二进制重命名**（无依赖，最安全）——纯文件移动 + Cargo.toml 修改，不涉及行为变更
2. **CLI 帮助输出**（依赖 1）——在 `ccli`/`ccda`/`ccweb` 三个二进制上增强 help/skill 输出
3. **三层 JSON 配置**（依赖 1，与 2 无依赖可并行）——`config.rs` 重写，迁移所有配置消费者
4. **Workgraph 门禁取消**（依赖 3，因为配置中的 `bg_*` 参数可能受影响）——删除 `bg_gate.rs`，简化推进循环

## 5. 风险与回退

- **二进制重命名**：风险极低，纯文件操作。外部脚本和 CI 引用旧二进制名需要同步更新。
- **CLI 帮助输出**：风险低，不影响运行时行为。
- **三层 JSON 配置**：中等风险——需要确保所有配置消费者（`Config` 结构体使用者）正确迁移，测试覆盖各层级合并行为。回退：保留 `Config::from_env()` 作为兼容路径。
- **Workgraph 门禁取消**：高风险——改变了 BG 推进的核心语义。`NeedsFix` 移除后，失败不会自动修复，完全依赖 agent 自报。需确保 `bg_ledger` 的账本记录兼容。