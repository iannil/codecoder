# Spec: trust 门禁下的「自我」加载(路线图 #5)

对应 [[0027-pi-comparison-and-borrowing-roadmap]] Wave 0 #5。借鉴 pi 的
`trust-manager.ts` / `project-trust.ts` / `resource-loader.ts` 的
`TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES`。落地时应另开 ADR 0028 记录「双闸门」决策。

## 问题

「文件系统即自我」的锋利边缘:`build_system_prompt`(`src/agent.rs:791`)无条件
`read_to_string(root.join("AGENTS.md"))`,并 `Registry::scan(root)`
(`src/registry.rs:31`)把 `skills/`、`prompts/`、`capabilities/` 载入 catalog 注入
system prompt。**clone 一个仓库并在其中启动 codecoder,该仓库的 `AGENTS.md` 与 skill/
prompt 正文(经 `use_skill` 全文注入)就悄然成为 agent 的身份与指令**——这是对「自我」的
prompt 注入。

更隐蔽:`ProjectAllowlist::load`(`src/permission.rs:72`)从 `<root>/codecoder.json`
读取**预授权执行 allowlist**。一个恶意仓库可随包附带 `codecoder.json`,预先授权
`run_command:rm` 之类——把运行期权限闸门(ADR 0005/0018)也一并绕过。

现状:执行有权限闸门,**加载/身份无任何闸门**。

## 设计:新增一道「加载期 trust 闸门」

trust 是与 permission **正交**的第二道闸门:permission 管「这次工具调用能不能执行」
(运行期),trust 管「磁盘上的『自我』能不能进入身份」(加载期)。

**1. 全局 trust 存储**(`src/trust.rs`,新模块)。

```rust
pub enum TrustDecision { Trusted, Untrusted }
```

- 存 `~/.codecoder/trust.json`(**不在项目内**——仓库不得为自己背书),可用
  `CODECODER_TRUST_FILE` 覆盖。映射 canonical 项目目录 → 决策。
- `decide(root) -> Option<TrustDecision>`:**就近祖先**查找(被信任的父目录信任其子目录);
  `None` = 未决。
- `record(root, TrustDecision)`:持久化(格式风格对齐 `ProjectAllowlist`,`BTreeMap`
  保证有序不 churn)。

**2. 加载点变 trust-aware.** 在 `AgentLoop::build`(`agent.rs:178`)解析一次
`TrustStatus`,并贯穿:

- `build_system_prompt(root, trusted)`:`trusted==false` 时**跳过** `AGENTS.md` 与
  catalog,只保留编译进二进制的基础身份 + 原生工具(原生工具是编译态,非磁盘「自我」,
  永远安全)。
- `Registry::scan` 仅在 `trusted` 时扫描;否则返回空 catalog。
- `ProjectAllowlist`:`trusted==false` 时**不加载** `codecoder.json`(视为空 allowlist)。
- `/reload`(`agent.rs:263`)同样走 trust 检查。

**3. 用户在场的授权流**(有用户时)。未决项目在构造时发一个新事件
(镜像 `Confirm`/`ask_user`,ADR 0016):

```rust
AgentEvent::TrustPrompt { root: PathBuf, reply_tx: Sender<TrustReply> }
```

TUI 渲染一个 Dialog(阻塞式模态,见 CONTEXT.md),三个选项映射到 scope:

- **Trust always** → `record(Trusted)`;
- **Trust once** → 本 session 视为 Trusted,不持久化;
- **Don't trust** → `record(Untrusted)`。

**4. headless 无用户**(ADR 0026)。无人应答 → 取 `CODECODER_DEFAULT_TRUST`
(`never`|`once`|`always`,默认 `never`)。未决 + headless → 当作 Untrusted,跳过加载,
发一条 `Notice` 说明「已跳过项目自我加载(未信任)」,保证可观测。

## 需要 trust 的资源(对齐 pi 的 TRUST_REQUIRING 集)

`AGENTS.md`、`CONTEXT.md`(若注入)、`skills/`、`prompts/`、`capabilities/`、
`codecoder.json`(执行 allowlist)。**不需要** trust:编译进二进制的原生工具与基础 system
prompt——它们不来自磁盘。

## 测试

- `trust.rs` 单测:`record`/`decide` 往返;就近祖先(父信任 → 子信任);
  `CODECODER_TRUST_FILE` 覆盖;未决返回 `None`。
- `agent.rs` 单测:未信任 root → `build_system_prompt` 为空、catalog 为空、
  `project_allowlist` 为空(即使磁盘上有 `AGENTS.md` 与 `codecoder.json`)。
- headless + `CODECODER_DEFAULT_TRUST=never` + 未决 → 跳过加载并发 Notice。
- 信任后 → 现有行为不变(回归)。

## 与现有 ADR 的关系

- 不改 ADR 0005/0018 的运行期权限语义;**新增**正交的加载期闸门,需 ADR 0028 记录双闸门模型。
- 与 ADR 0022 的 Shell capability scope ceiling 叠加:未信任时 capability 根本不进 catalog,
  谈不上运行;信任后才回到既有 scope ceiling 逻辑。

## 范围外

单个 artifact 的签名/校验(pi 也不做 per-file 签名);trust 决策的撤销 UI;远程/团队级
trust 同步。
