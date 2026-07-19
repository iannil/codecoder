# Spec: 溯源统一 SourceInfo(一等公民 #5)

> 来源:`docs/audit/0002-first-class-citizen-analysis-2026-07-19.md` 候选 **#5**。
> 给 Registry 扫描的每个加载物(AGENTS.md / CONTEXT.md / skills / prompts / capabilities)
> 挂统一 `SourceInfo`,让「从哪来、什么信任级」成为一等字段。
> [[0028-project-trust-load-gate]] 的自然延伸,增量小、防御性。

## 目标

- 新增 `SourceInfo` 类型:每个加载物携带**路径 / scope / origin** 三字段。
- `CatalogEntry` 扩展出 `source: Option<SourceInfo>`。
- Registry 扫描时**记录来源**:扫描路径对应 `scope`(project 或 user 级)、`origin`(top-level 或 package)。
- **不破坏**现有 `render_catalog` 的输出形状,也不改变 `use_skill`/`run_capability` 的运行时行为。
- 为后续(如注入 system prompt 时标注来源)提供结构化的溯源数据,但**本 spec 不落地那条注入逻辑**——只做数据层。

## 现状(必须兼容)

- `Registry::scan` 只扫 `skills/`/`prompts/`/`capabilities/` 三个目录,不区分来源路径。
- `CatalogEntry` 只有 `name` / `description` / `kind`——**无来源信息**。
- `trust::has_config_resources` 已识别 `AGENTS.md` / `CONTEXT.md` / `codecoder.json` / `skills/` / `prompts` / `capabilities/` 为信任需要资源,但没有把「哪个文件从哪来」统一记录。
- AgentLoop 的 `system_prompt` 构建(`agent.rs:793`附近)用 `AGENTS.md` + `CONTEXT.md` + `Registry::render_catalog`——**无来源标注**。

## 设计

### 1. SourceInfo 类型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    /// The resolved absolute path on disk.
    pub path: String,
    /// Whether the source lives in the user's global config dir, the project dir,
    /// or is a temporary/injected resource.
    pub scope: SourceScope,
    /// How the resource was loaded: top-level (directly from disk) or via a package.
    pub origin: SourceOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    /// `~/.codecoder/` or equivalent global config path.
    Global,
    /// The project root (`.codecoder/` or the repo root).
    Project,
    /// Temporary or injected (e.g., created by a tool, not loaded from a persistent dir).
    Temporary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceOrigin {
    /// Loaded directly from the filesystem (top-level skills/ dir).
    TopLevel,
    /// Loaded from a package (future extension; placeholder for now).
    Package,
}
```

### 2. CatalogEntry 扩展

```rust
pub struct CatalogEntry {
    pub name: String,
    pub description: String,
    pub kind: EntryKind,
    /// The source provenance of this entry. `None` means the provenance is unknown
    /// (e.g., a legacy or synthetic entry that hasn't been migrated).
    pub source: Option<SourceInfo>,
}
```

- `source: Option<SourceInfo>`——`None` 表示未知溯源(未迁移的旧记录或合成条目)。**不破坏反序列化**。
- 所有**现有构造站点**补上 `source: None`——不需要改渲染逻辑。

### 3. Registry::scan 记录来源

`Registry::scan` 接收 `root: &Path` 参数,扫描三个目录。每个目录的路径已知,据此可推断 scope:

- `root.join("skills")` → `scope: Project, origin: TopLevel`
- `root.join("prompts")` → `scope: Project, origin: TopLevel`
- `root.join("capabilities")` → `scope: Project, origin: TopLevel`

**未来扩展**(本 spec 不落地):扫描 `~/.codecoder/skills/` → `scope: Global, origin: TopLevel`;扫描 `skills/` 下的 `package.<name>.md` → `origin: Package`。

### 4. 系统 prompt 注入来源标注(可选,本 spec 不落地)

AgentLoop 将 system prompt 的 skills 部分改为:

```
Skills (activate with the `use_skill` tool):
- reviewing — how to review a PR  [project/top-level]
- fetcher — fetch a url  [global/top-level]
```

**本 spec 不落地这段**:只把 `SourceInfo` 数据准备好,渲染方式留给后续(如 `render_catalog` 的参数化扩展)。

## 内核改动点

- **`src/trust.rs` 或新 `src/source.rs`**:新增 `SourceInfo`、`SourceScope`、`SourceOrigin` 类型。**建议放在 `trust.rs`**(它是信任/溯源的同层设施,且 0028 已建立信任门禁);不拆新文件(改动极小)。
- **`src/registry.rs`**:
  - `CatalogEntry` 加 `source: Option<SourceInfo>` 字段。
  - `Registry::scan` 扫描时构建 `SourceInfo`(从目录路径推断 scope/origin)。
  - `scan_skills`/`scan_prompts`/`scan_capabilities` 签名增加 `source: SourceInfo` 参数。
  - `render_catalog` **不变**——来源信息不改变当前输出格式。
- **`src/agent.rs`**:`build` 将 `root` 传给 `Registry::scan`(已有 `root` 参数,只需传下去)。
- **`src/tool/builtin.rs`**:`use_skill`/`run_capability` 的 `run` 方法**不依赖** `SourceInfo`——来源信息只用于元数据,不改变执行逻辑。

## 测试

- `trust.rs` 新增 `SourceInfo` 序列化/反序列化 round-trip 测试。
- `registry.rs` 新增测试:扫描后 `CatalogEntry` 包含 `source` 字段,`scope` 为 `Project`、`origin` 为 `TopLevel`。
- 现有 `render_catalog` 测试**不变**(输出格式无变化)。

## 风险

- **反序列化兼容性**:`CatalogEntry` 加 `source: Option<SourceInfo>` 字段,需确保 `serde(default)` 使得旧序列化数据(无 `source` 字段)反序列化后 `source: None`。
- **API 向后兼容**:`CatalogEntry` 是 `pub` 类型,加 `source` 字段不会破坏二进制兼容性(编译时加),但下游代码(如果有)需要更新。codecoder 内无下游依赖。

## 刻意不做

- 不改变 `render_catalog` 输出格式——来源信息不渲染到 system prompt。(延后)
- 不注入 system prompt 时的来源标注——本 spec 只做数据层。(延后)
- 不扫描全局 `~/.codecoder/skills/`——本 spec 只记录 project 级来源。(延后)
- 不进 `AGENTS.md` 和 `CONTEXT.md` 的溯源——它们在 trust 门禁已处理,但 `SourceInfo` 类型可复用。(延后)