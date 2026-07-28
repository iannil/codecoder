# CodeCoder Headless 模式修复 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 headless BG_WORKGRAPH 模式中发现的 5 个问题，使 codecoder 能从零自动构建前端项目时不卡在命令拒绝/无限循环中。

**Architecture:** 三个独立修复包：P1 增强权限系统（AllowlistEntry 枚举 + ScopeConstraint 路径范围约束）、P2 调整工具参数与 agent 行为（milestone tool cap 默认值、diff 非 git fallback、行为约束文档）、P3 循环兜底修复（所有里程碑 needs_fix 且重试耗尽时尽早退出）。

**Tech Stack:** Rust, serde_json, permission/agent/background/dev tools 模块

---

## 全局约束

1. 所有 `serde` 反序列化必须向后兼容旧格式 `"run_command:npm"`
2. 旧 `codecoder.json` 文件在未修改时仍能正常加载
3. 每个修复包独立可测（`cargo test` 全部 pass）
4. 修复顺序：P1 → P2 → P3（P1 涉及多个文件的 API 变更，后面 task 依赖其产生的新签名）

---

## 文件变更总览

| 文件 | 修改类型 | 涉及修复 |
|------|---------|---------|
| `src/permission.rs` | 重构 | P1：AllowlistEntry + ScopeConstraint |
| `src/agent.rs:1170` | 修改调用 | P1：传递 args + root 到 allows() |
| `src/agent.rs` | 修改测试 | P1：更新 allows() 测试调用 |
| `src/config.rs:291` | 修改测试断言 | P2.1：8→15 |
| `src/tool/dev.rs:90-105` | 修改 run() | P2.2：diff 非 git fallback |
| `skills/driver-codecoder.md` | 添加章节 | P2.3：Headless 模式关键时序 |
| `src/background.rs:260-278` | 修改循环逻辑 | P3：卡住检测 + 退出 |
| `src/background.rs` | 测试 | P3：循环兜底测试 |

---

### Task 1: P1 — 定义 AllowlistEntry 枚举和 ScopeConstraint

**Files:**
- Modify: `src/permission.rs:1-97`

**Interfaces:**
- Consumes: 无（全新的枚举类型）
- Produces: `AllowlistEntry` 枚举 + `ScopeConstraint` 结构体 + 修改后的 `ProjectAllowlist` 签名

- [ ] **Step 1: 添加 AllowlistEntry 枚举定义**

在 `permission.rs` 的 `use` 语句之后、`PermScope` 之前添加：

```rust
/// A single entry in the allowlist. Supports plain string keys (backward compatible)
/// and scoped entries with path constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AllowlistEntry {
    /// Plain key: "run_command:npm"
    Plain(String),
    /// Scoped entry with optional constraints:
    /// {"prefix": "run_command:rm", "scope": {"project_bound": true}}
    Scoped {
        prefix: String,
        #[serde(default)]
        scope: ScopeConstraint,
    },
}

/// Constraints on an allowlist entry's usage scope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeConstraint {
    /// When true, the tool call is only allowed when its cwd is within the project root.
    #[serde(default)]
    pub project_bound: bool,
}

impl ScopeConstraint {
    /// Check whether a tool call's args satisfy this constraint.
    /// `root` is the project root directory. Returns true if the constraint passes.
    pub fn check(&self, args: &serde_json::Value, root: &std::path::Path) -> bool {
        if !self.project_bound {
            return true;
        }
        match args.get("cwd").and_then(serde_json::Value::as_str) {
            None => true, // no cwd specified → defaults to project root
            Some(cwd) => {
                let cwd_path = std::path::Path::new(cwd);
                cwd_path.is_absolute() && cwd_path.starts_with(root)
            }
        }
    }
}
```

- [ ] **Step 2: 实现 Ord/PartialOrd/PartialEq/Eq**

```rust
impl Ord for AllowlistEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Delegate to serialized form for deterministic ordering.
        // serde_json serializes untagged enums as plain string for Plain,
        // and as object for Scoped — this gives a natural sort order.
        serde_json::to_string(self).unwrap_or_default()
            .cmp(&serde_json::to_string(other).unwrap_or_default())
    }
}

impl PartialOrd for AllowlistEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for AllowlistEntry {
    fn eq(&self, other: &Self) -> bool {
        serde_json::to_string(self).ok() == serde_json::to_string(other).ok()
    }
}

impl Eq for AllowlistEntry {}
```

- [ ] **Step 3: 修改 ProjectAllowlist 结构体和方法**

将 `BTreeSet<String>` 改为 `BTreeSet<AllowlistEntry>`：

```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProjectAllowlist {
    #[serde(default)]
    allowlist: BTreeSet<AllowlistEntry>,
}

impl ProjectAllowlist {
    // ...path(), load() unchanged...

    /// Check if a permission key is allowed. For Scoped entries, also checks
    /// path constraints against the tool call's args and project root.
    pub fn allows(&self, key: &str, args: &serde_json::Value, root: &Path) -> bool {
        self.allowlist.iter().any(|entry| match entry {
            AllowlistEntry::Plain(k) => k == key,
            AllowlistEntry::Scoped { prefix, scope } => {
                (prefix == key || key.starts_with(prefix.as_str())) && scope.check(args, root)
            }
        })
    }

    // ...grant(), save() unchanged except type changes...
}
```

注意 `grant()` 方法签名需要从 `PermissionKey` 改为 `AllowlistEntry`。检查 `agent.rs` 中调用 `grant()` 的位置（在 `agent.rs:1214` 附近），需要传 `AllowlistEntry::Plain(key)`。

- [ ] **Step 4: 更新 SessionAllowlist**

`SessionAllowlist` 暂时保持 `HashSet<PermissionKey>` 不变（session 级别暂不引入路径范围）。不需要修改。

- [ ] **Step 5: 编译验证**

```bash
cd /Users/rong.zhu/Code/codecoder && cargo build 2>&1 | head -30
```

预期：编译错误，因为 `allows()` 签名变更了，且 `BTreeSet<AllowlistEntry>` 与测试代码不兼容。这是预期行为——下一步修复调用点。

---

### Task 2: P1 — 更新 agent.rs 调用点和 grant() 签名

**Files:**
- Modify: `src/agent.rs:1170`（permission 检查点）
- Modify: `src/agent.rs:1214`附近（grant 调用）

**Interfaces:**
- Consumes: Task 1 产生的 `AllowlistEntry` 和 `ProjectAllowlist::allows(key, args, root)` 新签名

- [ ] **Step 1: 修改 permission 检查点**

在 `agent.rs:1169-1170`，将：

```rust
if let Permission::Ask { key } = tool.permission(&args, &self.root) {
    if !self.allowlist.allows(&key) && !self.project_allowlist.allows(&key) {
```

改为：

```rust
if let Permission::Ask { key } = tool.permission(&args, &self.root) {
    let session_allows = self.allowlist.allows(&key);
    let project_allows = self.project_allowlist.allows(&key, &args, &self.root);
    if !session_allows && !project_allows {
```

- [ ] **Step 2: 修改 grant 调用点**

找到 `agent.rs:1214` 附近的 grant 调用（当用户手动授权时）。将：

```rust
self.project_allowlist.grant(&self.root, key)
```

改为（若 key 类型是 `String`）：

```rust
self.project_allowlist.grant(&self.root, AllowlistEntry::Plain(key))
```

确保 `grant()` 方法接收 `AllowlistEntry` 而非 `String`。

- [ ] **Step 3: 编译验证**

```bash
cd /Users/rong.zhu/Code/codecoder && cargo build 2>&1 | head -50
```

预期：编译通过，可能仍有测试代码中 `allows()` 调用需要修复。

---

### Task 3: P1 — 更新测试代码

**Files:**
- Modify: `src/agent.rs`（测试代码中的 `allows()` 调用）

- [ ] **Step 1: 找到并修复测试中的 allows() 调用**

查找所有 `project_allowlist.allows("...")` 调用（在 `agent.rs:2487` 和 `agent.rs:2579` 附近）。

将每个：

```rust
agent.project_allowlist.allows("run_command:git")
```

改为：

```rust
agent.project_allowlist.allows("run_command:git", &serde_json::json!({}), &agent.root)
```

- [ ] **Step 2: 运行全部测试**

```bash
cd /Users/rong.zhu/Code/codecoder && cargo test 2>&1 | tail -20
```

预期：全部 pass。

---

### Task 4: P2.1 — 增大 bg_milestone_tool_cap 默认值为 15

**Files:**
- Modify: `src/config.rs:107`（env 解析时的默认值）
- Modify: `src/config.rs:291`（测试断言 8→15）
- Modify: `src/recovery.rs:149`（硬编码值）
- Modify: `src/daemon/mod.rs:594`（测试中的硬编码值）

- [ ] **Step 1: 修改 config.rs 默认值**

在 `src/config.rs:107` 附近找到将 `CODECODER_BG_MILESTONE_TOOL_CAP` env 解析为 usize 的地方。默认值为 `8`，改为 `15`。

```rust
bg_milestone_tool_cap: env("CODECODER_BG_MILESTONE_TOOL_CAP")
    .and_then(|s| s.parse().ok())
    .unwrap_or(15),
```

- [ ] **Step 2: 修复测试断言**

```rust
// config.rs:291
assert_eq!(c.bg_milestone_tool_cap, 8);  →  assert_eq!(c.bg_milestone_tool_cap, 15);
```

- [ ] **Step 3: 修复 recovery.rs 和 daemon/mod.rs 中的硬编码值**

```rust
// recovery.rs:149
bg_milestone_tool_cap: 8,  →  bg_milestone_tool_cap: 15,

// daemon/mod.rs:594
bg_max_auto: 3, bg_circuit_k: 2, bg_milestone_tool_cap: 8,  →  ...bg_milestone_tool_cap: 15,...
```

- [ ] **Step 4: 运行测试确认**

```bash
cd /Users/rong.zhu/Code/codecoder && cargo test 2>&1 | tail -20
```

预期：全部 pass，config 测试断言匹配新默认值。

---

### Task 5: P2.2 — 修复 diff 工具在非 git 目录的 fallback

**Files:**
- Modify: `src/tool/dev.rs:90-105`（Diff::run 方法）

- [ ] **Step 1: 在 Diff::run 开头添加 git 仓库检测**

```rust
fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
    // Check if git repo exists before running git diff
    let git_check = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&ctx.root)
        .output();
    match git_check {
        Ok(out) if !out.status.success() => {
            return Ok(ToolOutput::err(
                "diff unavailable: no git repository. Run `git init` first."
            ));
        }
        Err(e) => {
            return Ok(ToolOutput::err(
                format!("diff unavailable: git check failed: {e}")
            ));
        }
        _ => {} // git repo exists, proceed
    }

    let mut a = vec!["diff"];
    // ...rest unchanged...
}
```

- [ ] **Step 2: 运行测试**

```bash
cd /Users/rong.zhu/Code/codecoder && cargo test 2>&1 | tail -20
```

预期：全部 pass。

---

### Task 6: P2.3 — 补充行为约束文档

**Files:**
- Modify: `skills/driver-codecoder.md`（添加 "Headless 模式关键时序" 章节）

- [ ] **Step 1: 在 driver-codecoder.md 中定位插入点**

在文件中找到合适位置（建议在 "陷阱与注意事项" 之前或之后添加一个新章节）。

- [ ] **Step 2: 添加章节内容**

在 `skills/driver-codecoder.md` 中添加以下内容：

```markdown
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
```

---

### Task 7: P3 — 里程碑循环兜底修复

**Files:**
- Modify: `src/background.rs:259-278`（retry_one_milestone Ok(None) 分支）
- Modify: `src/background.rs`（新增测试函数）

- [ ] **Step 1: 修改 retry_one_milestone Ok(None) 分支后的退出逻辑**

将 `src/background.rs:264-278` 的现有逻辑替换为增强版：

```rust
Ok(None) => {
    // 既无就绪、也无可重试 needs_fix → 判断是否应退出。
    if out.mission_state == crate::bg_gate::MissionState::Running {
        let g = crate::workgraph::WorkGraph::read(&root);
        // 兜底检查：所有里程碑 needs_fix 且至少一个已耗尽 fix_attempts
        let all_needs_fix = g.nodes.iter().all(|n| n.status == "needs_fix");
        let any_exhausted = g.nodes.iter().any(|n| {
            n.status == "needs_fix" && n.fix_attempts >= max_fix_attempts
        });
        if all_needs_fix && any_exhausted && max_fix_attempts > 0 {
            // 使用第一个 needs_fix 节点的 ID（降级为 0 兜底）
            let fallback_id = g.nodes.first().map(|n| n.id).unwrap_or(0);
            out.mission_state = crate::bg_gate::MissionState::StuckNeedsFix(fallback_id);
            out.events.push(format!(
                "all {} milestones needs_fix ({} fix_attempts exhausted) — exiting",
                g.nodes.len(), max_fix_attempts,
            ));
            obs.emit("stuck", &format!(
                "all {} milestones needs_fix, at least one exhausted — giving up",
                g.nodes.len(),
            ));
        } else {
            // 原有逻辑（找 needs_fix 的节点报告 ID）
            let needs_fix = g
                .nodes
                .iter()
                .find(|n| n.status == crate::workgraph::NodeStatus::NeedsFix);
            out.mission_state = match needs_fix {
                Some(n) => crate::bg_gate::MissionState::StuckNeedsFix(n.id),
                None => crate::bg_gate::MissionState::CompletedAllReady,
            };
        }
    }
    break;
}
```

- [ ] **Step 2: 运行测试检查**

```bash
cd /Users/rong.zhu/Code/codecoder && cargo test 2>&1 | tail -20
```

预期：全部 pass。

---

### Task 8: 集成验证

- [ ] **Step 1: 完整编译**

```bash
cd /Users/rong.zhu/Code/codecoder && cargo build 2>&1 | tail -10
```

预期：编译成功，无错误。

- [ ] **Step 2: 运行完整测试套件**

```bash
cd /Users/rong.zhu/Code/codecoder && cargo test 2>&1 | tail -20
```

预期：全部 pass（348+ pass，3 ignore）。

- [ ] **Step 3: 清理之前的实验目录**

```bash
rm -rf ~/Code/strategic-management-system
```
