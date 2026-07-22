# 复合命令 keying 加固 — 设计文档

- **日期**: 2026-07-22
- **状态**: 待用户审阅(Pending user review)
- **作者**: Claude Code(brainstorming 产物)
- **起因**: `docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md` P5 发现——`run_command` 的 PermissionKey 取首空白 token(`builtin.rs:48` `split_whitespace().next()`),故 `cd X && rm -rf Y` 只被 `run_command:cd` 门控;**预授权良性前缀(`cd`/`ls`/`echo`)隐式授权后缀任意命令**。
- **关联**: ADR 0018(Tool trait 与 PermissionKey)、`src/tool/builtin.rs`。

## 1. 背景与目标

上限压测(P5)坐实:`run_command` 把整条命令经 `sh -c` 执行(builtin.rs:77),但权限 key 只取首 token。后果:allowlist 里预授权 `run_command:cd`(良性)后,`cd X && <任意破坏性命令>` 整条跑通而不另问 —— 良性前缀成了任意后缀的通行证。

**目标**: 让**复合命令**(含 shell 控制运算符)**不可经良性前缀预授权**;**简单命令**(单命令)的既有快速 keying 与可预授权性**不变**;**执行路径(`sh -c`)不动**。

## 2. 已锁定决策

| 维度 | 决策 |
|---|---|
| 机制 | **D:复合命令 → key 为整条命令串**(`run_command:<整 cmd>`),简单命令保持 `run_command:<首token>` |
| 检测 | **保守偏向安全**:含 `&&`/`||`/`;`/`|`/`` ` ``/`$(`/`<`/`>` 或末尾 `&` 即判定复合(引号内运算符会误判→过度弹窗,但**绝不漏判**) |
| 执行 | `sh -c <整 cmd>` **不变**(复合仍能跑,只是权限 key 变) |
| ADR | **新建 ADR 0036**(引用 0018) |

**为何 D 而非 C(逐子命令授权)/ B(拒复合)**:C 需 shell 运算符拆分,引号/`$()`/嵌套是雷区,**解析错比现状更危险**(虚假安全感);B 过严,破合法流水线(`ls | grep`)、逼 agent 全改习惯。D **简单、无解析风险、安全**:整串 key 几乎不可经前缀预授权 → 交互每次弹(显全文)、headless 拒绝(agent 自适应,报告已观察 codecoder 会改 `cargo test --manifest-path`)。

## 3. 架构

```
RunCommand::key_for(cmd)
  ├─ is_compound(cmd)=true  → format!("run_command:{cmd}")     [整串,不可前缀预授权]
  └─ is_compound(cmd)=false → format!("run_command:{首token}")  [同今,可预授权]

is_compound(cmd):含 && || ; | ` $( < > 任一,或 trim_end 末尾 & → true
执行:Command::new("sh").arg("-c").arg(cmd)   [不变]
```

**行为**:
- **交互(cc+daemon)**:复合命令每次弹 `🔐 Permission request: run_command:<整条命令>`,用户看清全文才批;简单命令同今(可 AlwaysThisSession/Project 预授权)。
- **headless(BG)**:复合命令的整串 key 不在 allowlist → 自动拒绝(`denied: no user present`),agent 自适应改单命令等价物。
- **保守误判**:引号内的 `>`/`<`(如 `grep ">" f`)会被判复合 → 过度弹窗,**安全**(不漏判);v1 接受此代价。

## 4. 实现(builtin.rs)

`src/tool/builtin.rs` `impl RunCommand` 的 `key_for`(:47-50)改为:

```rust
    /// Permission key(ADR 0018)。简单命令按命令类(`run_command:git`);
    /// **复合命令(含 shell 运算符)按整条命令串**(`run_command:cd X && rm`),
    /// 使其不可经良性前缀预授权(ADR 0036,P5 加固)。
    fn is_compound(cmd: &str) -> bool {
        ["&&", "||", ";", "|", "`", "$(", "<", ">"]
            .iter()
            .any(|op| cmd.contains(op))
            || cmd.trim_end().ends_with('&')
    }
    fn key_for(cmd: &str) -> String {
        if Self::is_compound(cmd) {
            format!("run_command:{cmd}")
        } else {
            let head = cmd.split_whitespace().next().unwrap_or("");
            format!("run_command:{head}")
        }
    }
```

`permission()`/`run()` 不变(key_for 已被 permission 调用;run 仍 `sh -c` 整串)。

## 5. 测试(TDD)

- **`key_for` 单测**(builtin.rs 测试模块,既有 `run_command_keys_by_command_class` 旁):
  - `"git status"` → `"run_command:git"`(简单不变)。
  - `"cd X && cargo test"` → `"run_command:cd X && cargo test"`(整串)。
  - `"ls | grep foo"` → `"run_command:ls | grep foo"`(整串)。
  - `"a; b"` → `"run_command:a; b"`(整串)。
  - `"echo hi"` → `"run_command:echo"`(简单不变)。
  - `"sort &"` → `"run_command:sort &"`(末尾 & 触发复合)。
- **既有回归**:`run_command_keys_by_command_class`(builtin.rs:910,断言 `run_command:git`)仍绿(`"git ..."` 是简单命令)。
- **live 复验**(`codecoder-probe/` lab):`cd showcase/mini && cargo test` 经 cc → 弹 `run_command:cd showcase/mini && cargo test`(整串,**非** `run_command:cd`),证预授权 `run_command:cargo` 不再隐式覆盖复合。

## 6. ADR / 文档

- **新建 ADR 0036《复合命令 keying 加固》**(引用 0018):记录复合→整串 key、保守检测(引号内运算符误判偏向安全)、headless 拒绝语义、执行路径不变的取舍。
- **README / ARCHITECTURE**:权限段补一句"复合命令按整串 keying(ADR 0036),不可经前缀预授权"。

## 7. 不在本范围内(YAGNI)

- **逐子命令授权(C)**:不做(shell 拆分雷区;D 已堵住良性前缀漏洞)。
- **精确 shell 解析**(识别引号内的非运算符 `>`/`<`):不做(v1 保守误判可忍;精确解析引入复杂度与风险)。
- **复合命令 allowlist 通配/前缀匹配**:不做(整串精确匹配,operator 若要放行某固定复合可手动加整串到 allowlist)。
- **P10/P11**(各自独立 spec)。
