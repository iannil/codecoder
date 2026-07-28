# CodeCoder Headless 模式修复计划 — 设计方案

> 基于 2026-07-28 战略管控系统自主构建实验（5 个问题，无里程碑通过，无限循环），
> 分为三个修复包：P1 权限系统增强 / P2 工具 cap 与 agent 行为改进 / P3 循环兜底修复。
> 每个包独立可测，依赖关系 P1 → P2 → P3。

---

## 一、问题回顾

| # | 问题 | 严重度 | 现象 |
|---|------|--------|------|
| 1 | 基础命令被拒 | 🔴 致命 | `ls`/`cd`/`cat`/`pwd` 不在 allowlist，agent 无法诊断 |
| 2 | `npm install` 未执行 | 🔴 致命 | 没有 `node_modules`，后续所有操作无法进行 |
| 3 | 8-tool cap 太紧 | 🟡 中 | M1 写 7 个文件就到上限，无余量做 `npm install` |
| 4 | 无 git 仓库时工具失败 | 🟡 中 | `commit` 和 `diff` 工具在非 git 目录不可用 |
| 5 | 里程碑循环无终止 | 🟢 低 | 所有里程碑 needs_fix 后无限循环，不退出 |

---

## 二、P1：权限系统增强

### 2.1 问题分析

当前 `ProjectAllowlist` 使用 `BTreeSet<String>` 做纯字符串匹配。allowlist 条目如 `"run_command:rm"` 匹配一切以 `rm` 开头的命令（`rm temp`、`rm -rf /` 等同通过）。`run_command` 的 `permission()` 方法只返回命令首词作为 key，不传递 `cwd`/`args` 给检查器。

本次需要在不破坏向后兼容的前提下增加路径范围约束。

### 2.2 数据结构设计

```rust
/// 一条 allowlist 条目：纯字符串（向后兼容）或带范围约束的条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AllowlistEntry {
    /// 原格式："run_command:npm" → 精确匹配
    Plain(String),
    /// 新格式：{"prefix": "run_command:rm", "scope": {"project_bound": true}}
    Scoped {
        prefix: String,
        #[serde(default)]
        scope: ScopeConstraint,
    },
}

/// 范围约束
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeConstraint {
    /// 如果为 true，命令只能在项目根目录及其子目录内执行
    #[serde(default)]
    pub project_bound: bool,
}
```

### 2.3 匹配逻辑变更

```rust
impl ProjectAllowlist {
    // 旧的: pub fn allows(&self, key: &str) -> bool
    // 新的:
    pub fn allows(&self, key: &str, args: &Value, root: &Path) -> bool {
        self.allowlist.iter().any(|entry| match entry {
            AllowlistEntry::Plain(k) => k == key,
            AllowlistEntry::Scoped { prefix, scope } => {
                key.starts_with(prefix) && scope.check(args, root)
            }
        })
    }
}

impl ScopeConstraint {
    pub fn check(&self, args: &Value, root: &Path) -> bool {
        if !self.project_bound {
            return true;
        }
        match args.get("cwd").and_then(|v| v.as_str()) {
            None => true, // 未指定 cwd，默认在 root 执行
            Some(cwd) => {
                let cwd_path = Path::new(cwd);
                cwd_path.is_absolute() && cwd_path.starts_with(root)
            }
        }
    }
}
```

### 2.4 调用点变更

`src/agent.rs:1170`：

```rust
// 旧:
if !self.allowlist.allows(&key) && !self.project_allowlist.allows(&key) {
// 新:
let project_allows = self.project_allowlist.allows(&key, &args, &self.root);
let session_allows = self.allowlist.allows(&key);
if !session_allows && !project_allows {
```

`src/permission.rs` 中 `SessionAllowlist::allows()` 保持签名不变（session 级别暂不引入路径检查）。

### 2.5 `codecoder.json` 使用示例

```json
{
  "allowlist": [
    "write_file",
    "edit_file",
    "commit",
    "generate_skill",
    "generate_milestones",
    "run_command:npm",
    "run_command:node",
    "run_command:git",
    "run_command:cargo",
    "run_command:mkdir",
    "run_command:cp",
    "run_command:mv",
    {"prefix": "run_command:rm", "scope": {"project_bound": true}},
    {"prefix": "run_command:cat", "scope": {"project_bound": true}},
    {"prefix": "run_command:ls", "scope": {"project_bound": true}},
    {"prefix": "run_command:cd", "scope": {"project_bound": true}},
    {"prefix": "run_command:pwd", "scope": {"project_bound": true}}
  ]
}
```

### 2.5.1 关于 `BTreeSet` 排序

由于 `AllowlistEntry` 是 `untagged` 枚举，`BTreeSet` 需要 `Ord` 实现。通过手动实现 `Ord`，委托给 JSON 序列化字符串的字典序：

```rust
impl Ord for AllowlistEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_string().cmp(&other.to_string())
    }
}
impl PartialOrd for AllowlistEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
```

如果 `Ord` 实现复杂度高，也可降级为 `Vec<AllowlistEntry>`（允许条目通常 < 20，线性扫描开销可忽略）。

### 2.6 向后兼容

- `serde(untagged)` 确保旧格式 `"run_command:npm"` 自动反序列化为 `AllowlistEntry::Plain`
- 项目已有的 `codecoder.json` 无需修改
- `AllowlistEntry` 实现了 `Ord`（委托给序列化字符串排序），确保 `BTreeSet` 正常工作

### 2.7 测试要点

1. `AllowlistEntry::Plain` 反序列化与匹配
2. `AllowlistEntry::Scoped` 反序列化与匹配
3. `project_bound = true` 时，`cwd` 在根目录内允许 / 在外拒绝
4. `project_bound = false` 时，不检查路径
5. 未指定 `cwd` 参数时的行为（默认允许）
6. 与 session allowlist 的组合匹配

---

## 三、P2：工具 Cap 与 Agent 行为改进

### 3.1 增大 `bg_milestone_tool_cap` 默认值

**修改位置**：`src/config.rs`

```rust
// 旧: bg_milestone_tool_cap: 8
// 新: bg_milestone_tool_cap: 15
```

**依据**：Scaffold 类里程碑需要写 7+ 个配置文件 + 入口文件 + `npm install` + `git init`。8 次工具调用不够一轮完成，导致 M1 永远无法通过。15 提供足够余量完成初始化所有必要操作后仍有剩余次数用于验收。

### 3.2 修复 `diff` 工具在非 git 目录的 fallback

**修改位置**：`src/tool/builtin.rs` 的 `diff` 工具 `run()` 方法

**现状**：`diff` 工具使用 `git diff`（子进程）。无 git repo 时 git 报错并打印全屏 usage message，污染工具输出。

**修复**：

```rust
fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
    // 首步检测 git 仓库是否有效
    let git_dir_status = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&ctx.root)
        .output();
    
    match git_dir_status {
        Ok(out) if !out.status.success() => {
            return Ok(ToolOutput::err(
                "diff unavailable: no git repository in this directory. \
                 Run `git init` first to enable diff tracking."
            ));
        }
        Err(e) => {
            return Ok(ToolOutput::err(
                format!("diff unavailable: git command failed: {e}")
            ));
        }
        _ => {} // git repo 存在，继续原有逻辑
    }
    
    // 原有逻辑...
}
```

### 3.3 AGENTS.md / driver-codecoder 补充行为约束

**修改位置**：`skills/driver-codecoder.md` 新增 "Headless 模式关键时序"

在技能文档中增加一节的草稿内容：

```
## Headless 模式关键时序

项目初始化时遵循以下固定顺序，避免因步骤错乱导致重复重试：

1. **package.json → npm install**：写完 package.json 后立即执行 `npm install`
2. **git init → commit**：在首次 commit 前必须先 `git init`
3. **优先使用内置工具**：
   - `list_directory` 替代 `ls`
   - `read_file` 替代 `cat`/`head`/`tail`
   - `glob` 替代 `grep -r`
   - `diff` 替代 `git diff`
4. **避免复合 shell 命令**：`&&`/`||`/`|`/`2>&1` 会触发整串 keying 导致被拒
5. **rm 命令范围**：`rm` 仅用于删除项目目录内的文件，不得操作外部路径
```

（此节的语法需与 skill yaml frontmatter 的 markdown 格式兼容，确切输出以最终 skill 文件为准。）

---

## 四、P3：循环兜底修复

### 4.1 问题分析

当前 `background.rs` 的 milestones 主循环结构：

```rust
loop {
    // 1. 找下一个 ready pending 里程碑
    // 2. 没 ready 的则尝试 retry_one_milestone()
    // 3. retry 返回 None 则... 继续循环轮询
    // 4. advanced >= max_auto 才 break
}
```

当所有里程碑 `needs_fix` 且 `retry_one_milestone` 全部返回 `None`（已耗尽 `fix_attempts`）时，循环不会退出，因为 `advanced` 一直为 0，永远达不到 `max_auto`。

### 4.2 修复方案

**修改位置**：`src/background.rs`，`run_background_cfg` 函数中 `retry_one_milestone` 返回 `None` 后的处理逻辑。

```rust
// 在第 260 行附近的 retry_one_milestone → Ok(None) 分支后增加兜底检查

// retry_one_milestone 返回 None → 既无就绪 pending 也无可重试 needs_fix
// or retry_all_exhausted 按以下逻辑判断：

// 检查是否所有里程碑都无法再取得进展
// 条件：所有里程碑的 status 都是 needs_fix，且至少一个已耗尽 fix_attempts
let g = crate::workgraph::WorkGraph::read(&root);
let all_needs_fix = g.nodes.iter().all(|n| n.status == "needs_fix");
let any_exhausted = g.nodes.iter().any(|n| {
    n.status == "needs_fix" && n.fix_attempts >= max_fix_attempts
});

if all_needs_fix && any_exhausted {
    out.mission_state = crate::bg_gate::MissionState::StuckNeedsFix;
    out.events.push(format!(
        "all {} milestones stuck in needs_fix ({} fix_attempts exhausted) — exiting",
        g.nodes.len(), max_fix_attempts,
    ));
    obs.emit("stuck", &format!(
        "all {} milestones needs_fix, at least one exhausted — giving up",
        g.nodes.len(),
    ));
    break;
}
// 否则继续原有逻辑
```

### 4.3 额外改进：依赖链上游检查

当 `next_ready()` 返回的不是 `pending` 里程碑，而是 `needs_fix` 里程碑的依赖链下游时，可以在 `advance_one_milestone` 的 `resolve_bg_task` 阶段增加依赖链检查。

但 P3 的核心兜底（"所有 `needs_fix` + 至少一个耗尽 → 退出"）已覆盖大多数情况，此改进可延后。

### 4.4 测试要点

1. 空 workgraph → EmptyGraph（现有测试）
2. 6 个里程碑全部 `needs_fix`，其中一个 `fix_attempts=3` → 退出码 2（StuckNeedsFix）
3. 部分 `done` + 部分 `needs_fix` → 不触发退出条件
4. 新增 `fix_attempts` 未耗尽时 → 继续重试

---

## 五、修复优先级与依赖

```
P1 (Permission)
  └─ 必须优先：agent 无法运行任何命令 → 所有后续修复的前提
P2 (Tool cap + 行为改进)
  ├─ 调整默认值：纯配置变更，可独立修复
  ├─ diff fallback：纯工具层修复，可独立修复
  └─ 补充行为约束：文档变更，可独立修复
P3 (循环兜底)
  └─ 依赖 P1 和 P2 修复后验证效果时使用
```

建议修复顺序：
1. P1（权限数据结构 + 匹配逻辑 + 旧格式兼容）
2. P2.1（改默认值 8→15）
3. P2.2（diff fallback）
4. P2.3（行为约束文档）
5. P3（循环兜底）
6. 编写回归测试

---

## 六、验证方法

每个修复包完成后通过以下测试验证：

### P1 验证
```
1. cargo test（全部 pass）
2. 手动创建一个 codecoder.json 含 Scoped 和 Plain 混合条目
3. 检查 BTreeSet 排序和反序列化是否正确
4. project_bound + cwd 在根目录内/外分别测试
```

### P2 验证
```
1. cargo test（全部 pass）
2. 检查 diff 工具在非 git 目录返回清晰错误消息而非 usage
3. 启动 headless BG_WORKGRAPH 验证 M1 能否完成 npm install + git init
```

### P3 验证
```
1. 删除 strategic-management-system 重新运行 headless
2. 确认退出码不为 0 时不进入无限循环
3. 验证信息输出"all milestones stuck"等描述
```
