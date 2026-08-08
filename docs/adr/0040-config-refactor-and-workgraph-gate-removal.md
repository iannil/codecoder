# ADR 0040 — 配置重构与 workgraph 门禁移除

- **状态**: Accepted
- **日期**: 2026-08-08
- **关联**: ADR 0026(Background Agent)、ADR 0028(项目信任加载门禁)、ADR 0030(BG 客观验收门)、ADR 0033(BG 任务账本与退出码)、ADR 0034(Persistent Capability 跨重启韧性)

## 背景

截至 2026-08-07，CodeCoder 的配置和 workgraph 验收机制存在以下问题：

1. **配置分散在 30+ 个环境变量**中，用户每次使用需要设置大量 `CODECODER_*` 环境变量，体验差。
2. **`.ccd.env` 自动加载机制**存在安全风险——仓库本地文件注入 env，且白名单逻辑复杂，与"文件系统即自我"原则冲突。
3. **Per-milestone 客观验收门**（`bg_gate.rs` 的命令门 / review 门 / `needs_fix` 自恢复循环）增加了系统复杂度，但实践表明 agent 的自报能力和内置验证工具已足够覆盖验收需求。

## 决策

### 1. 二进制重命名（Task 1-2）

`Cargo.toml` 中的二进制定义从 `codecoder`/`cc` 改为：

| 旧名称 | 新名称 | 路径 |
|--------|--------|------|
| `codecoder` (src/main.rs) | `ccda` | `src/bin/ccda.rs` |
| `cc` (src/bin/cc.rs) | `ccli` | `src/bin/ccli.rs` |
| — | `ccweb` | `src/bin/ccweb.rs` |

移除了 `main.rs` 作为默认二进制（`ccda` 是新的 daemon 入口）。

### 2. 三层 JSON 配置（Task 3）

取代之前的 30+ 个 `CODECODER_*` 环境变量 + `.ccd.env` 自动加载：

```
层级 1 (内置默认)  ── 编译期默认值，无需任何配置文件
层级 2 (用户级)    ── ~/.codecoder/codecoder.json，全局生效
层级 3 (项目级)    ── <project_root>/.codecoder/codecoder.json，项目专属
```

- 后一层覆盖前一层：项目级 > 用户级 > 内置默认
- 每层使用 `ConfigPatch` 结构体（所有字段 `Option<T>`），缺失字段不覆盖下层
- `config.rs` 中的 `Config::load()` 负责三层合并

**保留的环境变量（仅用于进程路由）：** `CODECODER_ROOT`、`CODECODER_DAEMON`、`CODECODER_BG_TASK`、`CODECODER_BG_WORKGRAPH`、`CODECODER_SCRIPT`、`CODECODER_API_KEY`、`GITHUB_TOKEN`。

**`.ccd.env` 自动加载已完全移除。** 所有原 `.ccd.env` 可配置的字段（`MODEL`、`MAX_TOKENS`、`TEMPERATURE`、`BG_*` 等）改为在 `codecoder.json` 中配置。

### 3. Workgraph 门禁移除（Task 4）

Per-milestone 客观验收门（`bg_gate.rs`）已被移除：

- **命令门**（`extract_gate_command`，将 `acceptance` 中的 shell 命令提取执行）——移除
- **Review 门**（独立只读评审子 agent 覆盖 agent 自报 VERDICT）——移除
- **`needs_fix` 自恢复循环**（`retry_one_milestone`、`next_retryable`、`fix_attempts`/`last_failure` 字段）——移除
- **`Milestone.command` 字段**——移除
- **`GateVerdict`/`SubgoalOutcome.gate_kind`**——移除
- **`BgOutcome.denied`/`subgoals`**——简化

`bg_gate.rs` 文件保留为历史 reference（标记为已废弃），不删除。

里程碑现在是简单的依赖有序节点，agent 自报完成（标记 `done`）。验收与验证是 agent 自己的责任，通过内置工具（如 `run_command: cargo test`）完成。

## 后果

### 正面

- **配置大幅简化**：用户只需创建一个 `codecoder.json` 文件，无需设置大量环境变量
- **安全提升**：移除 `.ccd.env` 消除了仓库本地文件注入 env 的攻击面
- **架构简化**：移除了 `bg_gate.rs` 的复杂验收逻辑，减少了测试维护负担
- **二进制命名清晰**：`ccda`/`ccli`/`ccweb` 命名模式更一致，且为 `ccweb` 预留了命名空间

### 代价/约束

- **用户需迁移**：现有 `.ccd.env` 和 `CODECODER_*` 环境变量用户需迁移到 `codecoder.json`
- **验证责任转移**：客观验收门移除后，验证完全依赖 agent 的自报和工具执行，对 agent 的可靠性提出更高要求
- **`bg_gate.rs` 测试保留**：`bg_gate.rs` 的测试代码作为历史参考保留，增加测试集体积

### 不做

- 不添加新的客观验收机制（如外部验证器 hook）
- 不恢复 `.ccd.env` 的兼容性加载
- 不添加 `CODECODER_*` 环境变量到 `codecoder.json` 的自动迁移工具