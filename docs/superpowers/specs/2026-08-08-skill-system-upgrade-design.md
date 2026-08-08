# 设计：CodeCoder Skill 系统升级

> 基于 archived/claudecode 与 archived/pi 的 skill 设计调研，提炼可借鉴点。

## 背景与目标

CodeCoder 当前 skill 系统（`registry.rs`）较为简约：扫描 `skills/*.md` 提取 name+description 注入 system prompt，`use_skill` 工具按名读取全文。相较 Claude Code 和 Pi 的 skill 系统，缺少以下能力：

- 目录式结构（`skill/SKILL.md` + 附带脚本/文档）
- 多层级来源（内置→用户→项目）
- 条件 skill（按路径自动激活）
- 参数替换与模板变量
- 动态发现（子目录嵌套 skill）
- 内置（bundled）skill

本设计分三阶段落地，每阶段可独立交付。

## 关键决策

| 决策点 | 选择 | 理由 |
|--------|------|------|
| 目录结构 | 目录式 `skill/SKILL.md` + 扁平 `.md` 都支持 | 兼容现有技能，也允许技能附带脚本。与 Pi 一致 |
| 来源层级 | 三层：内置 → 用户 → 项目 | 与现有 three-layer search path 模式一致（见 memory） |
| 条件 skill | 引入 `paths` frontmatter + 路径匹配激活 | 来自 Claude Code 的最强信号——agent 触碰文件时自动注入相关规范 |
| 验证 | 不引入 | 保持零验证的灵活性 |
| 工具预授权 | 不引入 | skill 仅提供知识，权限由现有 permission 系统控制 |
| 参数/模板 | 引入参数替换 + 模板变量 | 支持 `${arg}`、`${CODECODER_SKILL_DIR}` 等 |
| prompt 注入 | 保持纯文本列表 + `use_skill` | 保持简洁，与现有 Agent 行为兼容 |
| 动态发现 | 引入（编辑路径向上冒泡） | 项目内子目录专有 skill |
| 内置 skill | 引入（懒提取到磁盘） | 系统级能力作为 skill 分发 |

## 三个阶段

```
阶段一：加载架构重塑
  ├── 目录式 + 扁平式兼容
  ├── 三层来源（内置→用户→项目）
  ├── ignore 尊重
  └── 去重（canonicalize 身份）

阶段二：注入与模板增强
  ├── 参数替换（${arg}）
  ├── 模板变量（${CODECODER_SKILL_DIR} 等）
  └── 条件 skill（paths 自动激活）

阶段三：高级特性
  ├── 动态发现（冒泡子目录）
  └── 内置 skill 机制（懒提取）
```

---

## 阶段一：加载架构重塑

### 1. 目录结构

**变更**：`scan_skills` 从当前只扫描 `skills/*.md` 改为两种模式兼容：

```
# 目录式（优先）：skill-name/SKILL.md
skills/
├── reviewing/
│   ├── SKILL.md          # 技能正文
│   ├── scripts/           # 辅助脚本
│   │   └── checklist.sh
│   └── references/        # 参考文档
│       └── review-guide.md
└── formatting/
    └── SKILL.md

# 扁平式（兼容）：skill-name.md
skills/
├── reviewing.md           # 直接作为 skill
└── formatting.md
```

**扫描规则**：
1. 遍历目录下每个条目
2. 若是目录 → 检查其中是否有 `SKILL.md` → 有则作为 skill（name = 目录名，不递归）
3. 若是 `.md` 文件 → 作为扁平 skill（name = stem）
4. 目录式优先：若目录包含 `SKILL.md`，不再将同目录下的散 `.md` 视为独立 skill

**name 确定**：保持现有 `parse_skill_meta` 逻辑——frontmatter `name` 优先，否则降级为 stem（目录名或文件名）。

### 2. 三层来源

```
内置（built-in / 编译时注册）
  ↓ 优先级高于用户级
用户（~/.codecoder/skills/）
  ↓ 优先级高于项目级
项目（<root>/skills/）
```

**扫描顺序**：用户级 → 项目级（与 `Config` 的 three-layer 一致）。内置 skill 不通过文件扫描，通过 `register_builtin_skill()` 编程注册。

**优先级规则**：同名的 skill，**项目级覆盖用户级，用户级覆盖内置**（与现有 three-layer search path 一致：离项目最近的优先级最高）。

**去重规则**：同名的**文件**（通过 symbolic link 指向同一文件）用 `canonicalize` 检测并跳过重复加载。

### 3. Ignore 尊重

**变更**：扫描时尊重 `.gitignore`/`.ignore`/`.fdignore` 文件（参考 Pi 实现）。

- 扫描每个目录时，尝试读取该目录下的 ignore 文件
- 将模式**前缀化**（相对路径前缀）后交给 `ignore` 库匹配
- 被 ignore 匹配的 skill 文件跳过不加载

### 4. 去重机制

**文件身份去重**：用 `canonicalize`（symlink 穿透）检测同一文件的多重路径，避免重复加载。

**名称冲突**：同一层内同名冲突先加载保留；跨层用户覆盖内置。

### 5. 数据结构变更

```rust
// 新增：Skill 元数据（比当前 catalog entry 更丰富）
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub kind: SkillKind,
    pub source: SkillSource,
    pub file_path: PathBuf,        // SKILL.md 或 .md 的路径
    pub base_dir: Option<PathBuf>, // 目录式 skill 才有（SKILL.md 所在目录）
    pub paths: Vec<String>,        // 条件 skill 的路径模式（阶段二）
}

pub enum SkillKind {
    Mature,     // skills/
    Draft,      // prompts/（已有）
    Builtin,    // 内置 skill
}

pub enum SkillSource {
    Builtin,     // 编译时注册
    User,        // ~/.codecoder/skills/
    Project,     // <root>/skills/
}
```

### 6. 子系统变更范围

| 文件 | 变更 |
|------|------|
| `src/registry.rs` | `scan_skills` 支持目录式+扁平式；新增三层来源扫描；添加 ignore 尊重；去重逻辑重写 |
| `src/tool/builtin.rs` | `UseSkill` 读取路径改为目录式优先（`skill/SKILL.md` → `skill.md`） |
| `src/agent.rs` | `build_system_prompt_with_catalog` 无变化（catalog 格式不变） |
| `src/config.rs` | 新增 `user_skills_dir` 配置项（默认 `~/.codecoder/skills/`） |

---

## 阶段二：注入与模板增强

### 1. 参数替换

**语法**：在 skill `.md` 正文中使用 `${arg_name}` 占位符，`use_skill` 调用时传入参数。

```json
// use_skill 调用
{
  "name": "reviewing",
  "args": {
    "file": "src/main.rs",
    "severity": "high"
  }
}
```

```markdown
# Reviewing Skill

## 指令

请审查文件 `${file}`，重点关注严重性为 `${severity}` 的问题。
```

### 2. 模板变量

| 变量 | 说明 |
|------|------|
| `${CODECODER_SKILL_DIR}` | skill 所在目录（目录式）或父目录（扁平式） |
| `${CODECODER_SESSION_ID}` | 当前 session ID |
| `${CODECODER_ROOT}` | 项目根目录 |
| `${CODECODER_CWD}` | 当前工作目录 |

变量替换在 `use_skill` 的 `run()` 内部完成，替换后的文本返回给 agent。

### 3. 条件 skill（paths 激活）

**frontmatter 新增字段**：

```yaml
---
name: rust-review
description: Rust 代码审查规范
paths:
  - "**/*.rs"
---
```

**激活机制**：

1. 扫描时，检测到 `paths` 字段的 skill 标记为**条件 skill**，不注入 system prompt
2. 条件 skill 存入独立的 `conditional_skills: Vec<SkillMeta>`
3. 当 agent 调用 `Read`/`Write`/`Edit` 等工具时，工具返回的路径信息被收集
4. 在 `process_turn` 的每次工具迭代后，用 `ignore` 库匹配路径与条件 skill 的 `paths`
5. 匹配命中 → 将该 skill 的全文注入**下一条消息**的 system prompt 中

**设计要点**：

- 条件 skill 激活后**保持活跃**直至 session 结束（不自动移除）
- 多个条件 skill 可同时激活
- 条件 skill 激活后，在 `use_skill` 的 catalog 中仍然可见
- 条件 skill 不参与 `disable-model-invocation` 逻辑（仅控制是否自动注入）

### 4. 子系统变更范围

| 文件 | 变更 |
|------|------|
| `src/registry.rs` | 新增 `paths` 字段解析；条件 skill 分离存储 |
| `src/tool/builtin.rs` | `UseSkill::run` 支持 `args` 参数 + 参数/变量替换 |
| `src/agent.rs` | `process_turn` 工具迭代后调用条件 skill 匹配逻辑；匹配命中时重建 system prompt |
| `src/tool/builtin.rs`（Read/Write/Edit） | 返回路径信息供条件 skill 匹配（或通过 `ToolCtx` 传递） |

---

## 阶段三：高级特性

### 1. 动态发现（嵌套子目录 skill）

**动机**：monorepo 或多模块项目中，子目录可能有自己的专有 skill（如 `frontend/` 下的 React 规范）。

**机制**：

1. 在 `ToolCtx` 中记录当前 turn 中被访问的文件路径
2. agent 每次工具调用后，`process_turn` 检查是否有新路径被访问
3. 对每个新路径，**从文件目录向上冒泡到项目根**，沿途检查 `skills/` 目录
4. 发现新 skill 目录 → 加载并合并到 skill 列表
5. 深度优先（离文件近的优先于离根近的）

**限制**：

- 仅在项目根内冒泡（不超出项目根）
- 跳过 gitignored 目录
- 动态发现的 skill 不持久化——/reload 会重新扫描
- 名称冲突：动态发现的 skill 优先级低于三层来源的同一名称

### 2. 内置（Bundled）Skill

**动机**：一些系统级能力（如 `/update-config`、`/verify`）作为 skill 分发，可通过 `use_skill` 调用，无需额外编译。

**机制**：

```rust
pub struct BuiltinSkill {
    pub name: String,
    pub description: String,
    pub content: String,           // skill 正文
    pub files: HashMap<String, String>, // 附加引用文件
}

impl Registry {
    pub fn register_builtin(&mut self, skill: BuiltinSkill) {
        // 插入到 catalog 最前面（内置最高优先级由用户层覆盖）
    }
}
```

**懒提取**：当 `files` 非空时，首次 `use_skill` 调用时**提取到磁盘**（`~/.codecoder/.builtin-skills/<name>/`），并前缀 `Base directory for this skill: <dir>` 到 skill 正文前。

**安全**：参考 Claude Code 的 `O_NOFOLLOW|O_EXCL` 防 symlink 攻击，目录 `0o700`/文件 `0o600`。

**调用**：`use_skill` 对内置 skill 和文件 skill 一视同仁——`run()` 内部检查内置注册表，命中则直接返回内容（不读文件系统）。

### 3. 子系统变更范围

| 文件 | 变更 |
|------|------|
| `src/registry.rs` | 新增 `BuiltinSkill` 注册表；`scan` 方法合并内置 skill |
| `src/tool/builtin.rs` | `UseSkill` 优先检查内置注册表；工具调用后上报路径 |
| `src/agent.rs` | `process_turn` 中路径收集 + 冒泡发现逻辑；`ToolCtx` 增加路径历史 |
| `src/daemon/mod.rs` | 内置 skill 注册点（初始化时调用 `register_builtin`） |

---

## 整体架构

```
┌─────────────────────────────────────────────────────┐
│                    Registry                          │
│  ┌──────────┐  ┌──────────┐  ┌───────────────────┐  │
│  │  Builtin  │  │  User    │  │  Project          │  │
│  │  skills   │  │  skills  │  │  skills           │  │
│  └──────────┘  └──────────┘  └───────────────────┘  │
│                       │                              │
│                       ▼                              │
│  ┌────────────────────────────────────────────────┐  │
│  │           Catalog (name + description)          │  │
│  │  ┌──────────────┐  ┌──────────────────────┐    │  │
│  │  │  Unconditional│  │  Conditional (paths) │    │  │
│  │  └──────────────┘  └──────────────────────┘    │  │
│  └────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
           │                      │
           ▼                      ▼
  ┌─────────────────┐   ┌──────────────────┐
  │  render_catalog  │   │  process_turn    │
  │  → system prompt │   │  工具路径匹配     │
  └─────────────────┘   │  → 激活条件 skill  │
                         └──────────────────┘
           │
           ▼
  ┌─────────────────────────────────────┐
  │  use_skill 工具                      │
  │  ├── 检查内置注册表                   │
  │  ├── 读 skills/<name>/SKILL.md       │
  │  │   或 skills/<name>.md             │
  │  ├── 参数替换 + 模板变量              │
  │  └── 返回全文                        │
  └─────────────────────────────────────┘
```

## 测试策略

### 阶段一

- **单元测试**：目录式 skill 扫描（`skill/SKILL.md`）；扁平式 skill 扫描（`skill.md`）；混合目录扫描
- **单元测试**：三层来源扫描（内置→用户→项目）；同名覆盖顺序
- **单元测试**：ignore 尊重（`.gitignore` 匹配跳过）
- **单元测试**：`canonicalize` 去重（symlink 指向同一文件）
- **现有测试**：更新 `registry.rs` 已有测试（`scans_skill_frontmatter_and_capability_manifest`、`tick_reload_picks_up_new_skill` 等）确保向后兼容

### 阶段二

- **单元测试**：参数替换（`${arg}` 占位符替换）；模板变量替换
- **单元测试**：条件 skill 路径匹配（`ignore` 库匹配规则）
- **单元测试**：条件 skill 激活（路径匹配后系统 prompt 包含 skill 内容）
- **集成测试**：`use_skill` 带参数调用

### 阶段三

- **单元测试**：动态发现（文件路径向上冒泡找 skill 目录）
- **单元测试**：内置 skill 注册与懒提取
- **安全测试**：内置 skill 提取的 symlink 攻击防护（`O_NOFOLLOW|O_EXCL`）
- **集成测试**：动态发现的 skill 优先级低于三层来源

## 文档同步

- 更新 `ARCHITECTURE.md` 中 Registry 和 Skill 部分
- 更新 `CONTEXT.md` 术语表（新增 `条件 skill`、`内置 skill` 等）
- 更新 `README.md` 相关数字与描述
- 新增 ADR（如 `0043-skill-system-upgrade.md`）
- 更新 `docs/superpowers/specs/` 中的 skill 相关说明

## 未纳入当前设计的项

以下项在调研中识别但未纳入本设计，标记为后续方向：

- **验证与诊断**：Pi 式的标准验证（name 规则、description 必填）——当前保持零验证
- **工具预授权**（`allowed-tools`）：skill 激活时自动授权工具——权限由现有 permission 系统控制
- **Agent Skills 标准 XML 注入格式**：当前保持纯文本列表 + `use_skill` 显式加载
- **skill 市场/仓库**：从远程仓库安装 skill——超出当前 scope