# 复合命令 keying 加固 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让含 shell 运算符的复合命令按整条命令串 keying,不可经良性前缀预授权(P5);简单命令 keying 不变。

**Architecture:** `RunCommand::key_for` 加 `is_compound` 检测;复合 → `run_command:<整 cmd>`,简单 → `run_command:<首token>`(同今)。执行路径(`sh -c` 整串)不动。

**Tech Stack:** Rust + cargo test。

## Global Constraints

- **简单命令 keying 不变**(`git status`→`run_command:git`),既有测试 `run_command_keys_by_command_class` 必须仍绿。
- **执行路径不变**(`sh -c <整 cmd>`)。
- **保守检测偏向安全**:引号内运算符误判为复合可忍(过度弹窗,绝不漏判)。
- **TDD** + 不破坏既有测试 + conventional commits 中文 + 分支 `fix/compound-command-keying`。

## File Structure

- Modify: `src/tool/builtin.rs` — `RunCommand::key_for` + `is_compound` + 单测。
- Create: `docs/adr/0036-compound-command-keying.md`;Modify `README.md`、`ARCHITECTURE.md`。

---

## Task 1: `is_compound` + `key_for` 两路 keying + 单测

**Files:**
- Modify: `src/tool/builtin.rs`(`impl RunCommand` 的 `key_for` :47-50;测试模块加用例)

**Interfaces:**
- Produces: `RunCommand::key_for(cmd: &str) -> String`(复合→整串,简单→首token);`RunCommand::is_compound(cmd: &str) -> bool`。

- [ ] **Step 1: 写失败测试**(在 `src/tool/builtin.rs` `run_command_keys_by_command_class`(:910)之后)

```rust
    #[test]
    fn run_command_compound_keys_by_full_string() {
        // 复合命令(含 shell 运算符)→ 整条命令串 key,不可经良性前缀预授权(ADR 0036)。
        let cases: &[(&str, &str)] = &[
            ("cd X && cargo test", "run_command:cd X && cargo test"),
            ("ls | grep foo", "run_command:ls | grep foo"),
            ("a; b", "run_command:a; b"),
            ("sort &", "run_command:sort &"),
            ("echo `whoami`", "run_command:echo `whoami`"),
            ("tee <(x)", "run_command:tee <(x)"),
        ];
        for (cmd, want) in cases {
            match RunCommand.permission(&json!({ "cmd": cmd }), std::path::Path::new(".")) {
                Permission::Ask { key } => assert_eq!(key, *want, "cmd={cmd:?}"),
                _ => panic!("expected Ask for {cmd:?}"),
            }
        }
    }

    #[test]
    fn run_command_simple_keys_by_first_token() {
        // 简单命令(无运算符)→ 首 token,同既有行为,可预授权。
        let cases: &[(&str, &str)] = &[
            ("git status --short", "run_command:git"),
            ("echo hi", "run_command:echo"),
            ("cargo test", "run_command:cargo"),
        ];
        for (cmd, want) in cases {
            match RunCommand.permission(&json!({ "cmd": cmd }), std::path::Path::new(".")) {
                Permission::Ask { key } => assert_eq!(key, *want, "cmd={cmd:?}"),
                _ => panic!("expected Ask for {cmd:?}"),
            }
        }
    }
```

- [ ] **Step 2: 跑测试看红**

Run: `cargo test run_command_compound_keys_by_full_string run_command_simple_keys_by_first_token 2>&1 | grep -E 'assertion|FAILED|right|left' | head`
Expected: 复合用例 FAIL(当前 key_for 返回首 token,如 `run_command:cd` ≠ `run_command:cd X && cargo test`)。

- [ ] **Step 3: 实现 is_compound + 改 key_for**

把 `src/tool/builtin.rs` `impl RunCommand` 的(:45-51):
```rust
impl RunCommand {
    /// Permission key at the command-class sweet spot (ADR 0018): `run_command:git`.
    fn key_for(cmd: &str) -> String {
        let head = cmd.split_whitespace().next().unwrap_or("");
        format!("run_command:{head}")
    }
}
```
改为:
```rust
impl RunCommand {
    /// 复合命令(含 shell 控制运算符)?保守偏向安全:引号内的 `>`/`<` 会误判
    /// 为复合(过度弹窗),但**绝不漏判**(ADR 0036)。
    fn is_compound(cmd: &str) -> bool {
        ["&&", "||", ";", "|", "`", "$(", "<", ">"]
            .iter()
            .any(|op| cmd.contains(op))
            || cmd.trim_end().ends_with('&')
    }

    /// Permission key(ADR 0018)。简单命令按命令类(`run_command:git`);
    /// **复合命令按整条命令串**(`run_command:cd X && rm`),使其不可经良性
    /// 前缀预授权(ADR 0036,P5 加固)。
    fn key_for(cmd: &str) -> String {
        if Self::is_compound(cmd) {
            format!("run_command:{cmd}")
        } else {
            let head = cmd.split_whitespace().next().unwrap_or("");
            format!("run_command:{head}")
        }
    }
}
```

- [ ] **Step 4: 跑测试看绿 + 既有回归**

Run: `cargo test run_command_ 2>&1 | grep -E 'test result|FAILED' | head`
Expected: `run_command_compound_keys_by_full_string`、`run_command_simple_keys_by_first_token`、`run_command_keys_by_command_class`(既有)、`run_command_executes`(既有)全 `ok`。

- [ ] **Step 5: 全测试 + commit**

Run: `cargo test 2>&1 | grep -E 'test result:' | grep -v '0 failed' || echo ALL_GREEN`
Expected: `ALL_GREEN`。
```bash
git add src/tool/builtin.rs
git commit -m "feat(run_command): 复合命令按整串 keying(P5)

含 shell 运算符(&&/||/;/|/\`/\$(/</>或末尾&)的复合命令 key 改为整条命令串,
不可经良性前缀(cd/ls/echo)预授权;简单命令保持首token keying。执行路径不变。
保守检测偏向安全(引号内运算符误判可忍)。"
```

---

## Task 2: ADR 0036 + README/ARCHITECTURE 同步

**Files:**
- Create: `docs/adr/0036-compound-command-keying.md`;Modify `README.md`、`ARCHITECTURE.md`

- [ ] **Step 1: 写 ADR 0036**

```markdown
# ADR 0036 — 复合命令 keying 加固

- **状态**: Accepted
- **日期**: 2026-07-22
- **关联**: ADR 0018(Tool trait 与 PermissionKey)、上限压测 P5(`docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md`)

## 背景

P5 发现:`run_command` 的 PermissionKey 取首空白 token(`split_whitespace().next()`),而命令经 `sh -c` 整串执行。故 `cd X && rm -rf Y` 只被 `run_command:cd` 门控——**预授权良性前缀(`cd`/`ls`/`echo`)隐式授权后缀任意命令**。

## 决策

1. **复合命令按整条命令串 keying**:`run_command:<整 cmd>`,几乎不可经前缀预授权 → 交互每次弹(显全文)、headless 拒绝。简单命令(无运算符)保持首 token keying(`run_command:git`),既有可预授权性不变。
2. **保守检测偏向安全**:`is_compound` 命中 `&&`/`||`/`;`/`|`/`` ` ``/`$(`/`<`/`>` 或末尾 `&` 即判复合。引号内的 `>`/`<` 会误判(过度弹窗),但**绝不漏判**——安全优先于 UX。
3. **执行路径不变**:仍 `sh -c <整 cmd>`,复合命令仍能跑,只是权限 key 变。

## 后果

- **正面**:堵住"良性前缀→任意后缀"漏洞;用户审批复合命令时看到整条命令全文。
- **代价**:合法复合(`cd X && cargo test`)交互下每次弹(不再能经 `run_command:cargo` 预授权);headless 下复合被拒,agent 须自适应(改 `cargo test --manifest-path`,报告已观察此行为)。引号内含 `>`/`<` 的简单命令被误判为复合(过度弹窗)。
- **不做**:逐子命令授权(shell 拆分雷区);精确 shell 解析识别引号;复合命令通配 allowlist。
```

- [ ] **Step 2: README/ARCHITECTURE 补注**

`README.md` 权限相关段(或工具表 `run_command` 行)补一句:复合命令(含 shell 运算符)按整条命令串 keying,不可经前缀预授权(ADR 0036)。`ARCHITECTURE.md` 权限与安全段补同样一句。

- [ ] **Step 3: 全测试 + commit**

Run: `cargo test 2>&1 | grep -E 'test result:' | grep -v '0 failed' || echo ALL_GREEN`
Expected: `ALL_GREEN`。
```bash
git add docs/adr/0036-compound-command-keying.md README.md ARCHITECTURE.md
git commit -m "docs: ADR 0036 复合命令 keying 加固 + README/ARCHITECTURE 同步"
```

---

## Task 3: live 复验

**Files:** 无源码改动;`codecoder-probe/` lab。

- [ ] **Step 1: 重编译**

Run: `cargo build 2>&1 | tail -1`
Expected: `Finished`。

- [ ] **Step 2: 复合命令 keying 整串(对比 P5)**

```bash
LAB=/Users/rong.zhu/Code/codecoder-probe
set -a; . /Users/rong.zhu/Code/codecoder/.ccd.env; set +a
# 起 daemon
CODECODER_ROOT="$LAB" target/debug/codecoder > /tmp/p5_daemon.log 2>&1 & sleep 2
# 复合命令 → 应弹整串 key(非 run_command:cd);喂 n 不批,只看 key
CODECODER_ROOT="$LAB" /Users/rong.zhu/Code/codecoder/docs/superpowers/scripts/drive_cc.sh p5_recheck "在 showcase/mini 下运行 'cd showcase/mini && cargo test'" <(printf 'n\n') 2>&1 | grep -E '🔐|run_command:' | head -3
# 关 daemon
kill -TERM "$(pgrep -f 'target/debug/codecoder' | head -1)" 2>/dev/null; sleep 1
```
Expected: `🔐 Permission request: run_command:cd showcase/mini && cargo test`(**整串**,对比 P5 修复前的 `run_command:cd`)。

- [ ] **Step 3: 记结论**

```bash
printf '\n## P5 修复复验(2026-07-22,fix/compound-command-keying)\n- 复合命令 cd X && cargo test → key 整串 run_command:cd X && cargo test(修复前是首token run_command:cd)\n- 预授权 run_command:cargo 不再隐式覆盖复合\n' >> /Users/rong.zhu/Code/codecoder-probe/matrix.md
echo recorded
```

---

## Self-Review(plan vs spec)

**1. Spec coverage:**
- is_compound + key_for 两路(spec §3/§4)→ Task 1 ✓
- 保守检测清单(spec §2/§4)&& 末尾 &→ Task 1 ✓
- 单测:简单不变 + 复合整串(spec §5)→ Task 1 ✓
- 既有 `run_command_keys_by_command_class` 回归(spec §5)→ Task 1 Step 4 ✓
- live 复验(spec §5)→ Task 3 ✓
- ADR 0036 + README/ARCHITECTURE(spec §6)→ Task 2 ✓
- 范围外(spec §7:不做逐子命令/精确解析)→ 计划未涉 ✓

**2. Placeholder scan:** 无 TBD/TODO;key_for/is_compound 给出完整代码;测试用例覆盖 6 复合 + 3 简单 ✓

**3. Type consistency:** `RunCommand::key_for(cmd: &str) -> String`、`is_compound(cmd: &str) -> bool`(Task 1)与既有调用(`permission()` 调 `Self::key_for`)一致;测试经 `RunCommand.permission(...)` 匹配 `Permission::Ask { key }`(同既有 :910 模式)✓
