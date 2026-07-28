---
name: driver-codecoder
description: >
  Drive codecoder (cc/ccd/headless BG_WORKGRAPH) to build a project autonomously.
  Use when orchestrating codecoder's own binaries to construct a software project
  from spec, including: launching daemon, sending messages via `cc`, managing
  trust/allowlist, driving workgraph milestones headlessly, reading bg_ledger,
  and handling the 5 exit codes. Also use for dogfooding — using codecoder to
  build codecoder itself or any other project.
  ALWAYS use this skill when you need to run codecoder itself, drive it headlessly,
  configure trust/allowlist for a headless run, or debug a BG_WORKGRAPH exit.
---

# driver-codecoder

## 核心原则：这技能是写给 codecoder 自己看的

你是 codecoder 自己。这份技能告诉你如何**驱动自己**去构建项目：
- 你的二进制（cc/ccd）放在哪里
- 在 headless 模式下如何配置 trust 和 allowlist 才能跑起来
- 哪些 env 需要设，哪些不必设
- workgraph 跑完后退出码是什么意思，怎么排查
- 驱动时有哪些陷阱

**用这份技能时，你不需要再去读源代码找这些信息**——它们已经提炼在这里了。

---

## 概念总览

你（codecoder）有三种运行模式：

| 模式 | 触发 | 用户在场 | 用途 |
|------|------|---------|------|
| **Daemon + Client** | `CODECODER_DAEMON=1` → `cargo run` + `cargo run --bin cc` | ✓ | 交互式开发调试 |
| **Headless BG (显式 task)** | `CODECODER_BG_TASK=<task>` | ✗ | 跑完一条指令即退出 |
| **Headless Workgraph** | `CODECODER_BG_WORKGRAPH=1` | ✗ | 自动推进 workgraph 里程碑 |

---

## 快速启动（交互式）

```bash
cd /Users/rong.zhu/Code/codecoder
cargo build

# 启动 daemon（后台）
CODECODER_DAEMON=1 cargo run &

# 连接客户端（交互式）
cargo run --bin cc

# 或者单次发消息（自动连接→发消息→收结果→退出）
cargo run --bin cc -- "列出当前目录的文件"
```

注意：`cc "msg1" && cc "msg2"` 这种方式**串行发出**是安全的——每个 `cc` 等待前一个 turn 完成再退出，`&&` 保证下一个 `cc` 在前一个退出后才启动。但如果 agent 的 `drive_workgraph` 空闲线程与用户 turn 同时写文件，仍可能产生竞争。

---

## 信任与权限配置（headless 关键）

### 第一步：信任（Trust）

Headless 模式下无人在场，**必须**预先授权信任，否则你的 AGENTS.md、skills、codecoder.json allowlist 统统不加载：

```bash
export CODECODER_DEFAULT_TRUST=always
```

或预写 `~/.codecoder/trust.json`：
```json
{"trusted":["/path/to/project"]}
```

### 第二步：权限 allowlist（codecoder.json）

在项目根目录创建 `codecoder.json`，预授权 headless 需要的工具：

```json
{
  "allowlist": [
    "write_file",
    "edit_file",
    "commit",
    "generate_skill",
    "run_command:cargo"
  ]
}
```

**关键规则**：
- 只读工具（`read_file`/`glob`/`grep`/`search_web`/`search_github`/`reason`/`memory`/`diff`/`use_skill`）是 `Permission::None`，**不需要授权**。
- 简单命令按 head 授权（`run_command:cargo` 允许所有以 `cargo` 开头的命令）。
- **复合命令**（含 `&&`/`||`/`;`/`|`/`2>&1`）按整串命令 keying，不可经前缀预授权。
- Shell 环境 Capability 上限 `AlwaysThisSession`，不可 `AlwaysThisProject`。

---

## Headless BG_WORKGRAPH 驱动

### 完整启动命令

```bash
export CODECODER_DEFAULT_TRUST=always
export CODECODER_BG_WORKGRAPH=1
export CODECODER_MAX_TOKENS=16384
cargo run
```

执行流程：
1. `main.rs` 检测到 `CODECODER_BG_WORKGRAPH=1` → 进入 `run_background_cfg`
2. 读 `workgraph.json` → 找 `next_ready()`（依赖已满足的 pending 里程碑）
3. 驱动 agent 完成该里程碑 → 验收门检查（`acceptance` 被当 shell 命令执行）
4. 验收通过 → 标记 `done`；验收失败 → 标记 `needs_fix`，自动重试
5. 重复直到 `max_auto` 耗尽或遇到不可恢复错误
6. 退出码反映最终状态

### 退出码

| 退出码 | 含义 | 处理方式 |
|--------|------|---------|
| 0 | 正常完成（`CompletedAllReady`） | 无需处理 |
| 2 | 硬依赖断裂（`BlockedAt`） | 检查 workgraph.json，补全依赖里程碑 |
| 2 | 重试耗尽（`StuckNeedsFix`） | 人工修复后，将节点 status 改回 `pending` |
| 3 | 连续失败熔断（`CircuitBreaker`） | 检查连续的失败原因 |
| 4 | 系统错误（`Error`） | 查 stderr 输出 |

### 自恢复机制

你（codecoder）在 headless workgraph 模式中内置了自恢复能力：

- **`CODECODER_BG_MAX_FIX_ATTEMPTS`**（默认 3）——单里程碑验收 `needs_fix` 后最多自动重试次数
- 重试时，`retry_one_milestone` 调用 `build_repair_prompt` 将上一轮失败原因注入修复 prompt
- 重试不计入 `max_auto` 推进预算和 `consecutive_fail` 熔断计数
- 预算耗尽仍 `needs_fix` 才落 `StuckNeedsFix`，退出码 2
- 重试计数 `fix_attempts` 持久化在 `workgraph.json` 的里程碑上，跨进程尊重预算

**注意**：自恢复仅限 headless runner（`run_background_cfg`）。交互式 `drive_workgraph` 与 daemon 空闲推进线程仍只标记 `needs_fix`，不会自动重试。

### 关键环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `CODECODER_BG_MAX_AUTO` | 3 | 单次调用最多推进的里程碑数 |
| `CODECODER_BG_CIRCUIT_K` | 2 | 连续失败熔断阈值 |
| `CODECODER_BG_MILESTONE_TOOL_CAP` | 8 | 单里程碑 turn 的工具迭代上限 |
| `CODECODER_BG_MAX_FIX_ATTEMPTS` | 3 | needs_fix 后最多自动重试次数（0=禁用） |
| `CODECODER_MAX_TOKENS` | 8192 | 单次生成 max_tokens（写大文件时建议 16384） |
| `CODECODER_MAX_TOKENS_CEILING` | 32768 | 截断自适应上调封顶值 |
| `CODECODER_NOOP_NUDGE_THRESHOLD` | 3 | 连续纯探索步后注入 steering nudge（0=禁用） |
| `CODECODER_MAX_TOOL_OUTPUT` | 262144 | 工具输出截断阈值（字节） |

### 账本查询

每次 BG 运行追加一条 JSONL 到 `bg_ledger.jsonl`：

```bash
# 最近 10 次
cc ledger

# 最近 N 次
cc ledger --last 50

# 仅失败记录
cc ledger --failed

# 最近一次完整详情
cc ledger --detail
```

---

## 编写 workgraph 里程碑

### 验收门规则

milestone 的 `acceptance` 字段就是验收条件。`bg_gate.rs` 会扫描 `acceptance` 文本，如果包含 `cargo test`/`cargo build`/`make ` 等模式的行，**整行提取为 shell 命令执行**。这意味着：

- **`acceptance` 中独占一行的裸命令**（如 `cargo test`）会被自动执行
- 中文 prose 行可能导致 gate 报 "unexpected argument"
- 不希望走命令门的验收，用纯 prose 描述

### 如何处理 needs_fix

当里程碑标记 `needs_fix` 时：

**headless 模式**：自动通过 `build_repair_prompt` 重试（`CODECODER_BG_MAX_FIX_ATTEMPTS` 次内）。
**交互式模式**：你需要人工读取 `workgraph.json` 中的 `last_failure` 字段，修复问题后，将 status 改回 `pending`。

---

## 推荐：Headless 模式关键时序

项目初始化时遵循以下固定顺序，避免因步骤错乱导致重复重试：

1. **package.json → npm install**：写完 package.json 后立即执行 `npm install`，不要等所有文件写完后才装
2. **git init → commit**：在首次 commit 前必须先 `git init`，否则 `diff`/`commit`/`review` 工具不可用
3. **优先使用内置工具**：
   - `list_directory` 替代 `ls`
   - `read_file` 替代 `cat`/`head`/`tail`
   - `glob` 替代 `grep -r`
   - `diff`（内置工具）替代 `git diff`
4. **避免复合 shell 命令**：`&&`/`||`/`|`/`2>&1` 会触发整串 keying 导致被拒
5. **rm 命令范围**：`rm` 仅用于删除项目目录内的文件，不得操作外部路径

---

## 陷阱与注意事项

### 1. Trust 门是加载时的第一道闸

不设 `CODECODER_DEFAULT_TRUST=always` → `codecoder.json` 的 allowlist **不加载** → 所有 Ask-tool 被拒绝，拒绝消息会明确说明原因。这个问题在 eval 测试中多次出现。

### 2. 不要并发驱动同一 daemon

`cc` 向同一 daemon 并发发消息 → agent 共享会话历史 → 异步写文件版本竞争。**必须串行**，等每条 turn 完成后再发下一条。如果你需要并发工作，用独立 root/daemon。

### 3. 客观验收门会执行 shell 命令

milestone `acceptance` 中含 `cargo test`/`cargo build`/`make` 的行会被提取为 shell 命令执行。验收门结果影响里程碑状态（`done`/`needs_fix`）。

### 4. 验证纪律

headless 模式下你（或 agent）可能谎报测试通过。**务必独立运行 `cargo test` 验证**，不要相信 "all pass"。

### 5. 弱模型在 headless 下容易过度探索

弱模型（如小参数模型）倾向于把整个 turn 花在 `read_file` 上而不动手。通过在指令中明确"禁止探索"并给出内联类型签名可以有效破解。

### 6. `.ccd.env` 安全限制

`.ccd.env` 只注入安全调参白名单（`MODEL`/`MAX_TOKENS`/`TEMPERATURE`/`BG_*` 等），**不覆盖**已设进程 env。API key、trust 门、PATH 变量一律拒绝注入——这些必须来自真实 shell。

### 7. 编译后运行

修改代码后需先 `cargo build` 再运行。`cargo run` 虽会自动编译，但可能忽略未跟踪的文件变更。

---

## 示例：完整 dogfooding 流程

### 场景 1：交互式开发

```bash
cargo build \
  && export CODECODER_DEFAULT_TRUST=always \
  && export CODECODER_API_KEY=sk-... \
  && CODECODER_DAEMON=1 cargo run &

sleep 2

# 创建 allowlist
cat > codecoder.json << 'EOF'
{"allowlist":["write_file","edit_file","commit","run_command:cargo"]}
EOF

# 发指令
cargo run --bin cc -- "请实现一个 Rust 函数：parse_config 从文件路径读取 TOML 配置"

# 独立验证
cargo test
```

### 场景 2：Headless workgraph 自动构建

```bash
export CODECODER_DEFAULT_TRUST=always
export CODECODER_BG_WORKGRAPH=1
export CODECODER_MAX_TOKENS=16384

cargo run
ret=$?

if [ $ret -eq 0 ]; then
  echo "✅ 成功"
elif [ $ret -eq 2 ]; then
  echo "⚠️ 需要人工介入，检查 workgraph.json 和 bg_ledger.jsonl"
  cargo run --bin cc -- "ledger --detail"
elif [ $ret -eq 3 ]; then
  echo "⚠️ 熔断，检查连续失败原因"
elif [ $ret -eq 4 ]; then
  echo "❌ 系统错误，查看 stderr"
fi
```

### 场景 3：排查退出码 2

```bash
# 1. 查看账本
cargo run --bin cc -- "ledger --detail"

# 2. 查看 workgraph 状态
cat workgraph.json | python3 -m json.tool | grep -A5 '"status"'

# 3. 修复后重置
# 编辑 workgraph.json，把 needs_fix/blocked 改为 pending
# 然后重新运行 headless
CODECODER_BG_WORKGRAPH=1 cargo run
```

---

## 运行时架构速查

```
cc 客户端 (stdin/stdout)  ←→  ccd daemon (Unix socket)  ←→  AgentLoop (线程)
                                │
                            AgentEvent 流
                            (StreamDelta / ToolStarted / ToolFinished / TurnComplete /
                             Prompt / Notice / Error)
```

- `ccd` 监听 `$CODECODER_ROOT/.ccd.sock`
- `cc` 无状态，每次连接发送 `ClientRequest`，接收 `ServerEvent` 流
- 5 种交互式提示（Permission/Ask/Confirm/Plan/Trust）经 `Prompt`/`PromptReply` 往返
- 工具串行执行，取消是协作式（`SigInt` 翻转 `CancelToken`）