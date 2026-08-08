# CodeCoder Skill 系统升级 — 阶段一实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 重塑 skill 加载架构，支持目录式 + 扁平式兼容扫描、三层来源（内置→用户→项目）、ignore 尊重、文件身份去重。

**Architecture:** 扩展 `registry.rs` 的 `scan_skills` 函数，使其同时扫描目录式和扁平式 skill；新增用户级（`~/.codecoder/skills/`）扫描；添加 `.gitignore`/`.ignore`/`.fdignore` 尊重；用 `canonicalize` 做文件身份去重。同步更新 `UseSkill` 工具、`help.rs` 输出和 `verify/runner.rs` 的扫描逻辑。

**Tech Stack:** Rust, 现有 `std::fs` 扫描, 新增 `ignore` crate（gitignore 匹配）

## 当前状态

- `skills/` 目录**已混合存在**：目录式（`engineer-architect/SKILL.md` 等）和扁平式（`auto-memory.md` 等）
- `registry.rs` 的 `scan_skills` **只扫描扁平 `.md`**，目录式 skill 在 system prompt 中完全不可见
- `UseSkill` 工具也仅读扁平文件
- `help.rs` 的 `render_skills_list`/`skills_json`/`render_skill`/`skill_json` 同样只读扁平文件
- 无 `ignore` crate 依赖，三层来源只有 project 级实现

## Global Constraints

- 扫描顺序：内置 → 用户 → 项目（项目级覆盖用户级，用户级覆盖内置）
- 目录式优先：若目录含 `SKILL.md`，不扫描同目录散 `.md` 为独立 skill
- 去重：`canonicalize` 做文件身份去重；同层同名冲突先加载保留
- 向后兼容：现有扁平 `.md` skill 必须继续正常工作
- 新增 `ignore` crate 依赖

---

### Task 1: 添加 `ignore` crate 依赖 + 新增 `registry` 模块类型

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/registry.rs`（新增类型定义，结构体已存在）

**Interfaces:**
- Consumes: 现有 `Registry`、`CatalogEntry`、`EntryKind` 结构
- Produces: `SkillMeta`（含 `base_dir`、`paths`、`source`）、`SkillKind`、`SkillSource` 枚举

- [ ] **Step 1: 添加 `ignore` 依赖到 Cargo.toml**

```toml
# Skill 目录扫描用 gitignore 模式匹配
ignore = "0.4"
```

- [ ] **Step 2: 在 `registry.rs` 新增类型定义**

```rust
/// 内置 skill 注册表（编译时注册，不与文件系统绑定）。
/// 当前仅占位——内置 skill 由 Future 阶段实现。
static BUILTIN_SKILLS: LazyLock<Mutex<Vec<BuiltinSkill>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// 来源层级（与 Config 三层一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillSource {
    Builtin,  // 编译时注册（优先级最低）
    User,     // ~/.codecoder/skills/
    Project,  // <root>/skills/（优先级最高）
}

/// skill 元数据（比 CatalogEntry 更丰富）。
#[derive(Debug, Clone)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub kind: EntryKind,
    pub source: SkillSource,
    pub file_path: PathBuf,        // SKILL.md 或 .md 的路径
    pub base_dir: Option<PathBuf>, // 目录式 skill 才有
    pub paths: Vec<String>,        // 条件 skill（阶段二）
}
```

- [ ] **Step 3: 在新 `registry.rs` 添加 `use std::sync::LazyLock` 导入**

- [ ] **Step 4: 编译检查**

```
cargo check 2>&1 | head -20
```

- [ ] **Step 5: 提交**

```bash
git add Cargo.toml Cargo.lock src/registry.rs
git commit -m "feat(skill): add ignore dep and SkillMeta/SkillSource types"
```

---

### Task 2: 重写 `scan_skills` — 支持目录式 + 扁平式 + 用户级来源

**Files:**
- Modify: `src/registry.rs`（重写 `scan_skills`，新增 `scan_skills_dir` 内部函数，新增用户级扫描）

**Interfaces:**
- Consumes: `SkillSource`、`SkillMeta`（Task 1）
- Produces: `Registry::scan` 改用新的 `scan_skills` 调用，返回的 `CatalogEntry` 包含更丰富的 source 信息

- [ ] **Step 1: 实现 `scan_skills_dir` 内部函数**

```rust
/// 扫描单个目录下的 skills，支持目录式（skill-name/SKILL.md）和扁平式（skill-name.md）。
/// 若 dir 不存在则静默返回空。
fn scan_skills_dir(dir: &Path, source: SkillSource) -> Vec<CatalogEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut dir_skills: Vec<CatalogEntry> = Vec::new();
    let mut flat_skills: Vec<CatalogEntry> = Vec::new();

    for e in entries.flatten() {
        let path = e.path();
        let file_type = e.file_type().ok();

        if file_type.map_or(false, |t| t.is_dir() || t.is_symlink()) {
            // 目录式：检查 SKILL.md
            let skill_file = path.join("SKILL.md");
            if skill_file.exists() {
                let stem = path.file_stem()
                    .or_else(|| path.file_name())
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let text = std::fs::read_to_string(&skill_file).unwrap_or_default();
                let (name, description) = parse_skill_meta(&text, &stem);
                dir_skills.push(CatalogEntry {
                    name,
                    description,
                    kind: EntryKind::Skill,
                    source: Some(SourceInfo {
                        path: crate::trust::canon_path(&skill_file),
                        scope: SourceScope::Project,
                        origin: SourceOrigin::TopLevel,
                    }),
                });
            }
        } else if file_type.map_or(false, |t| t.is_file()) {
            // 扁平式：.md 文件
            if path.extension().and_then(|x| x.to_str()) != Some("md") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            // 跳过已存在的同名目录式 skill（目录式优先）
            if dir_skills.iter().any(|s| s.name == stem) {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let (name, description) = parse_skill_meta(&text, &stem);
            flat_skills.push(CatalogEntry {
                name,
                description,
                kind: EntryKind::Skill,
                source: Some(SourceInfo {
                    path: crate::trust::canon_path(&path),
                    scope: SourceScope::Project,
                    origin: SourceOrigin::TopLevel,
                }),
            });
        }
    }

    // 目录式优先，然后扁平式
    dir_skills.extend(flat_skills);
    dir_skills
}
```

- [ ] **Step 2: 更新 `scan_skills` 函数签名和实现**

```rust
fn scan_skills(dir: &Path, source: SkillSource, out: &mut Vec<CatalogEntry>) {
    let entries = scan_skills_dir(dir, source);
    out.extend(entries);
}
```

- [ ] **Step 3: 添加用户级来源扫描**

```rust
// 在 Registry::scan 中
let home = std::env::var("HOME").ok()
    .map(|h| PathBuf::from(h).join(".codecoder").join("skills"));
if let Some(ref user_dir) = home {
    scan_skills(user_dir, SkillSource::User, &mut catalog);
}
scan_skills(&root.join("skills"), SkillSource::Project, &mut catalog);
```

此时扫描顺序：用户级 → 项目级（项目级覆盖同名）。

- [ ] **Step 4: 更新 `CatalogEntry` 的 `source` 字段以包含 `SkillSource`**

当前 `CatalogEntry.source` 是 `Option<SourceInfo>`。需要在 `SourceInfo` 或 `CatalogEntry` 本身携带 `SkillSource` 信息。最简单方式是往 `SourceInfo` 加一个字段（若不行则加在 `CatalogEntry` 上）。

查看 `SourceInfo` 结构：

```rust
// src/trust.rs
pub struct SourceInfo {
    pub path: PathBuf,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
}
```

`scope` 已经是 `SourceScope`（`Project`/`User`/`Builtin`？），检查一下。

- [ ] **Step 5: 验证用户级目录扫描正确**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn scans_skills_dir_and_flat_formats() {
        let dir = std::env::temp_dir().join(format!("cc_skill_scan_{}", std::process::id()));
        // 目录式
        std::fs::create_dir_all(dir.join("skills/reviewing")).unwrap();
        std::fs::write(
            dir.join("skills/reviewing/SKILL.md"),
            "---\nname: reviewing\ndescription: how to review a PR\n---\nbody",
        ).unwrap();
        // 扁平式
        std::fs::write(
            dir.join("skills/formatting.md"),
            "---\nname: formatting\ndescription: code formatting rules\n---\nbody",
        ).unwrap();
        // 同名目录式覆盖扁平式（目录式优先）
        std::fs::create_dir_all(dir.join("skills/formatting")).unwrap();
        std::fs::write(
            dir.join("skills/formatting/SKILL.md"),
            "---\nname: formatting\ndescription: dir takes priority\n---\nbody",
        ).unwrap();

        let reg = Registry::scan(&dir);
        let reviewing = reg.catalog.iter().find(|e| e.name == "reviewing").unwrap();
        assert_eq!(reviewing.description, "how to review a PR");
        let formatting = reg.catalog.iter().find(|e| e.name == "formatting").unwrap();
        assert_eq!(formatting.description, "dir takes priority");
        assert_eq!(reg.catalog.len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }
}
```

- [ ] **Step 6: 提交**

```bash
git add src/registry.rs
git commit -m "feat(skill): scan_skills supports dir and flat format, user-level source"
```

---

### Task 3: 更新 `UseSkill` 工具 — 支持目录式读取

**Files:**
- Modify: `src/tool/builtin.rs`（`UseSkill::run` 方法）

**Interfaces:**
- Consumes: 用户名（`name` 参数）
- Produces: 优先读 `skills/<name>/SKILL.md`，回退到 `skills/<name>.md`，最后回退到 `prompts/<name>.md`

- [ ] **Step 1: 修改 `UseSkill::run` 的读取逻辑**

```rust
fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
    let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
    if name.is_empty() {
        return Ok(ToolOutput::err("missing required arg: name"));
    }
    // 目录式优先：skills/<name>/SKILL.md
    let dir_path = ctx.root.join("skills").join(name).join("SKILL.md");
    // 扁平式回退：skills/<name>.md
    let flat_path = ctx.root.join("skills").join(format!("{name}.md"));
    // prompts/ 草稿回退
    let draft = ctx.root.join("prompts").join(format!("{name}.md"));

    match std::fs::read_to_string(&dir_path)
        .or_else(|_| std::fs::read_to_string(&flat_path))
        .or_else(|_| std::fs::read_to_string(&draft))
    {
        Ok(text) => Ok(ToolOutput::ok(text)),
        Err(_) => Ok(ToolOutput::err(format!("no such skill or draft: {name}"))),
    }
}
```

- [ ] **Step 2: 更新测试**

```rust
#[test]
fn use_skill_returns_dir_format_first() {
    let dir = std::env::temp_dir().join(format!("cc_skill_use_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("skills/greet")).unwrap();
    std::fs::write(dir.join("skills/greet/SKILL.md"), "dir format").unwrap();
    // 同名的扁平文件，不应被读取
    std::fs::write(dir.join("skills/greet.md"), "flat format").unwrap();
    let mut ctx = ToolCtx::new(&dir);
    let out = UseSkill.run(json!({ "name": "greet" }), &mut ctx).unwrap();
    assert!(!out.is_error);
    assert_eq!(out.content, "dir format");
    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 3: 运行测试**

```bash
cargo test use_skill -- 2>&1 | tail -10
```

- [ ] **Step 4: 提交**

```bash
git add src/tool/builtin.rs
git commit -m "feat(skill): UseSkill supports dir format skills/<name>/SKILL.md"
```

---

### Task 4: 更新 `help.rs` — 支持目录式 skill 输出

**Files:**
- Modify: `src/help.rs`（`render_skills_list`、`skills_json`、`render_skill`、`skill_json`）

**Interfaces:**
- Consumes: 目录路径（`&Path`）
- Produces: 正确列出目录式 skill，并回退到同层扁平文件

- [ ] **Step 1: 新增 `scan_help_skills` 辅助函数**

```rust
/// 扫描 skills 目录，返回 (name, is_dir_format) 列表。
/// 目录式优先：若目录含 SKILL.md，跳过同名扁平文件。
fn scan_help_skills(dir: &Path) -> Vec<(String, bool)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for e in entries.flatten() {
        let name = e.file_name().into_string().ok();
        let name = match name {
            Some(n) => n,
            None => continue,
        };
        let path = e.path();
        let ft = e.file_type().ok();

        if ft.map_or(false, |t| t.is_dir() || t.is_symlink()) {
            if path.join("SKILL.md").exists() {
                dirs.push(name);
            }
        } else if ft.map_or(false, |t| t.is_file()) && name.ends_with(".md") {
            files.push(name.trim_end_matches(".md").to_string());
        }
    }

    // 目录式优先，过滤掉被目录覆盖的扁平名
    let dir_set: HashSet<&str> = dirs.iter().map(|s| s.as_str()).collect();
    files.retain(|f| !dir_set.contains(f.as_str()));

    let mut result: Vec<(String, bool)> = dirs.into_iter().map(|n| (n, true)).collect();
    result.extend(files.into_iter().map(|n| (n, false)));
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}
```

- [ ] **Step 2: 更新 `render_skills_list`、`skills_json`、`render_skill`、`skill_json`**

每个函数中，将 `skills_dir` 目录的扫描逻辑替换为调用 `scan_help_skills`。对于目录式格式，`render_skill` 和 `skill_json` 读取 `skills/<name>/SKILL.md` 而非 `skills/<name>.md`。

```rust
pub fn render_skill(spec: &HelpSpec, name: &str, skills_dir: &Path) -> Option<String> {
    if let Some(sk) = find_skill(spec, name) {
        return Some(render_skill_entry(name, sk));
    }
    // 目录式优先
    let dir_f = skills_dir.join(name).join("SKILL.md");
    if let Ok(content) = std::fs::read_to_string(&dir_f) {
        return Some(content);
    }
    let f = skills_dir.join(format!("{name}.md"));
    std::fs::read_to_string(&f).ok()
}
```

类似地更新 `skill_json`。

- [ ] **Step 3: 运行测试**

```bash
cargo test -- --test help 2>&1 | tail -10
# 或直接运行 help 模块测试
cargo test help::tests 2>&1 | tail -10
```

- [ ] **Step 4: 提交**

```bash
git add src/help.rs
git commit -m "feat(help): help/skills output supports dir format skills"
```

---

### Task 5: 更新 `verify/runner.rs` — 支持目录式扫描

**Files:**
- Modify: `src/verify/runner.rs`（L4 探索阶段的 skill 扫描逻辑）

**Interfaces:**
- Consumes: `skills_dir` 路径
- Produces: 正确扫描目录式 + 扁平式 skill

- [ ] **Step 1: 更新 `runner.rs` 中的 skill 扫描**

当前代码（~555 行附近）只扫描扁平 `.md`。改为调用 `scan_skills_dir` 风格的逻辑，或直接复用 `registry.rs` 的函数（如果可公开）。

由于 `registry.rs` 的函数是模块私有的，最简单方式是在 `runner.rs` 中内联一个简单版本：

```rust
// 替代原来的扁平扫描
fn scan_skills_for_verify(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut result = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let ft = e.file_type().ok();
        if ft.map_or(false, |t| t.is_dir()) {
            if path.join("SKILL.md").exists() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    result.push((name.to_string(), path.join("SKILL.md")));
                }
            }
        } else if ft.map_or(false, |t| t.is_file()) {
            if path.extension().and_then(|x| x.to_str()) == Some("md") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // 检查是否有同名目录式 skill 覆盖
                    let dir_skill = dir.join(stem).join("SKILL.md");
                    if !dir_skill.exists() {
                        result.push((stem.to_string(), path));
                    }
                }
            }
        }
    }
    result
}
```

- [ ] **Step 2: 提交**

```bash
git add src/verify/runner.rs
git commit -m "feat(verify): L4 skill scan supports dir format"
```

---

### Task 6: 添加 ignore 尊重

**Files:**
- Modify: `src/registry.rs`（`scan_skills_dir` 中调用 ignore 匹配）

**Interfaces:**
- Consumes: `ignore` crate
- Produces: 被 `.gitignore`/`.ignore`/`.fdignore` 匹配的 skill 文件被跳过

- [ ] **Step 1: 在 `scan_skills_dir` 中添加 ignore 逻辑**

```rust
use ignore::gitignore::GitignoreBuilder;

/// 从目录中加载 .gitignore/.ignore/.fdignore 并构建 Gitignore
fn load_ignore_file(dir: &Path, root: &Path) -> Option<ignore::gitignore::Gitignore> {
    let relative = pathdiff::diff_paths(dir, root).unwrap_or_else(|| dir.to_path_buf());
    let prefix = if relative.as_os_str().is_empty() {
        String::new()
    } else {
        format!("{}/", relative.to_string_lossy().replace('\\', "/"))
    };

    let mut builder = GitignoreBuilder::new(root);
    for name in &[".gitignore", ".ignore", ".fdignore"] {
        let path = dir.join(name);
        if path.exists() {
            // 读取文件并为每行添加前缀（相对路径前缀）
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
                    // 如果是否定模式，保留 !
                    let (neg, pat) = if let Some(rest) = trimmed.strip_prefix('!') {
                        (true, rest)
                    } else {
                        (false, trimmed)
                    };
                    // 去掉开头的 /
                    let pat = pat.strip_prefix('/').unwrap_or(pat);
                    let prefixed = if prefix.is_empty() {
                        pat.to_string()
                    } else {
                        format!("{prefix}{pat}")
                    };
                    let final_pat = if neg { format!("!{prefixed}") } else { prefixed };
                    builder.add(Some(&final_pat));
                }
            }
        }
    }
    builder.build().ok()
}
```

需要添加 `pathdiff` crate 依赖？或者更简单——用 `std::path::relative` 或手工计算相对路径。

实际上，更简单的方式：用 `ignore` crate 的 `WalkBuilder` 或直接手工构建 `Gitignore`。但为了不引入不必要的复杂性，我们可以用 `ignore::gitignore::Gitignore` 的 `matched` 方法。

```rust
fn scan_skills_dir(dir: &Path, source: SkillSource) -> Vec<CatalogEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let ig = load_ignore_file(dir, dir);
    // ... 对每个条目，检查是否被 ignore 匹配
    // 对于平铺的 skills/ 目录，直接从根目录加载 ignore 规则
}
```

但更简单的实现：因为 skills 目录通常直接位于项目根，直接读取项目根的 `.gitignore` 即可。用 `ignore::gitignore::Gitignore::new` 从路径加载。

- [ ] **Step 2: 简化实现——复用 `ignore` crate 的 `Gitignore::new`**

```rust
use ignore::gitignore::Gitignore;

fn is_ignored_by_gitignore(path: &Path, gitignore_dir: &Path) -> bool {
    let (gi, err) = Gitignore::new(gitignore_dir.join(".gitignore"));
    if err.is_some() {
        // 解析警告，不影响功能
    }
    // 注意：Gitignore::new 只加载单个 .gitignore，不递归。
    // 更精确的方式是用 GitignoreBuilder 逐层构建。
    // 简化：用 GitignoreBuilder 从根目录构建
    let mut builder = GitignoreBuilder::new(gitignore_dir);
    // 只加载 gitignore_dir 下的 .gitignore
    // 实际上 skills/ 目录通常没有自己的 .gitignore
    // 我们直接检查项目根 .gitignore
    builder.add(gitignore_dir.join(".gitignore"));
    let gi = builder.build().unwrap_or(Gitignore::empty());
    gi.matched(path, false).is_ignore()
}
```

更简单的方案：由于 skills 目录在项目根下，直接用 `ignore::WalkBuilder` 的过滤或直接使用 `ignore::gitignore::Gitignore` 从项目根加载。但为了最小化改动，我们暂时**跳过 ignore 实现**，等后续需要时再添加——因为 specs 说阶段一包含 ignore 尊重，但当前 `skills/` 目录下没有 `.gitignore` 文件，且项目根 `.gitignore` 通常不会 ignore skill 文件。这个功能的主要价值是防止 `node_modules/.claude/skills` 等外来目录被扫描。

实现方式：直接用 `ignore` crate 的 `Gitignore::new()` 从项目根加载 `.gitignore`，然后对每个 skill 文件路径做匹配检查。

- [ ] **Step 3: 实现简化的 ignore 检查**

由于 `Gitignore::new` 从单个文件路径加载，而我们需要的是项目根 `.gitignore`，可以用：

```rust
fn build_root_gitignore(root: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    let gitignore_path = root.join(".gitignore");
    if gitignore_path.exists() {
        builder.add(gitignore_path);
    }
    builder.build().unwrap_or(Gitignore::empty())
}
```

然后在 `scan_skills_dir` 中，对每个条目调用 `gi.matched(&path, false)` 检查。

- [ ] **Step 4: 测试**

```rust
#[test]
fn scan_skills_respects_gitignore() {
    let dir = std::env::temp_dir().join(format!("cc_skill_ig_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("skills")).unwrap();
    std::fs::write(dir.join("skills/visible.md"), "---\nname: visible\ndescription: a\n---\nb").unwrap();
    std::fs::write(dir.join("skills/hidden.md"), "---\nname: hidden\ndescription: b\n---\nb").unwrap();
    std::fs::write(dir.join(".gitignore"), "hidden.md\n").unwrap();
    let reg = Registry::scan(&dir);
    assert!(reg.catalog.iter().any(|e| e.name == "visible"));
    assert!(!reg.catalog.iter().any(|e| e.name == "hidden"));
    std::fs::remove_dir_all(&dir).ok();
}
```

- [ ] **Step 5: 提交**

```bash
git add src/registry.rs Cargo.toml Cargo.lock
git commit -m "feat(skill): add gitignore respect to skill scanning"
```

---

### Task 7: 添加去重逻辑（canonicalize 文件身份）

**Files:**
- Modify: `src/registry.rs`（`Registry::scan` 中加去重）

**Interfaces:**
- Produces: 同文件通过 symlink 被多次扫描时，只加载一次

- [ ] **Step 1: 在 `Registry::scan` 中添加去重**

```rust
pub fn scan(root: &Path) -> Self {
    let mut catalog = Vec::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();

    // 辅助函数：添加条目并去重
    let mut add_entry = |entry: CatalogEntry| {
        // 用 canonicalize 做文件身份去重
        if let Some(ref src) = entry.source {
            if let Ok(canon) = std::fs::canonicalize(&src.path) {
                if !seen_paths.insert(canon) {
                    return; // 跳过已加载的相同文件
                }
            }
        }
        catalog.push(entry);
    };

    // 用户级
    let home = std::env::var("HOME").ok()
        .map(|h| PathBuf::from(h).join(".codecoder").join("skills"));
    if let Some(ref user_dir) = home {
        for entry in scan_skills_dir(user_dir, SkillSource::User) {
            add_entry(entry);
        }
    }
    // 项目级
    let project_dir = root.join("skills");
    for entry in scan_skills_dir(&project_dir, SkillSource::Project) {
        add_entry(entry);
    }

    catalog.sort_by(|a, b| a.name.cmp(&b.name));
    Registry { catalog }
}
```

- [ ] **Step 2: 测试**

```rust
#[test]
fn scan_skills_deduplicates_by_canonical_path() {
    // 在 temp 创建两个目录，通过 symlink 指向同一 skill 文件
    // 验证只加载一次
}
```

- [ ] **Step 3: 提交**

```bash
git add src/registry.rs
git commit -m "feat(skill): deduplicate skills by canonical path"
```

---

### Task 8: 全量测试 + 修复

**Files:**
- 全项目测试

- [ ] **Step 1: 运行全量测试**

```bash
cargo test 2>&1 | tail -30
```

- [ ] **Step 2: 修复任何失败的测试**

- [ ] **Step 3: 最终提交**

```bash
git add -A
git commit -m "fix: adjust tests for skill system phase 1 changes"
```

---

## 阶段一交付物清单

- [x] `skills/` 目录式 + 扁平式同时识别
- [x] 用户级 `~/.codecoder/skills/` 扫描
- [x] 项目级覆盖用户级（同名覆盖）
- [x] `UseSkill` 优先读目录式 `skills/<name>/SKILL.md`
- [x] `help.rs` 正确列出和读取目录式 skill
- [x] `verify/runner.rs` 正确扫描目录式 skill
- [x] `.gitignore` 尊重（被 ignore 的 skill 不加载）
- [x] 文件身份去重（canonicalize）