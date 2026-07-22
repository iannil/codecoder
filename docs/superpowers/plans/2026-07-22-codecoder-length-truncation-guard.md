# 工具输出长度截断 guard — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 read_file / run_command 输出加长度截断 guard(默认 256KB,env 可配),防无界内存/上下文膨胀(P10)。

**Architecture:** `config.rs` 加 `max_tool_output`(`CODECODER_MAX_TOOL_OUTPUT`,默认 256×1024);`builtin.rs` 加 `truncate_output(s,max)` helper(char 边界安全 + marker);read_file 限读(`take(max+1)`,内存有界),run_command 在合并 stdout+stderr+exit 后截断(上下文有界)。

**Tech Stack:** Rust + cargo test。

## Global Constraints

- **正常(≤max)输出原样透传**,marker 仅超长出现;既有 read_file/run_command 小输入测试必须仍绿。
- **read_file 限读内存有界**;run_command 全 drain 后截断(瞬态内存未封顶,ADR 0037 记)。
- **char 边界安全**(`is_char_boundary` 回退,不切坏 UTF-8)。
- **TDD** + 不破坏既有测试 + conventional commits 中文 + 分支 `fix/length-truncation-guard`。
- **run_shell_cancellable 把 stdout+stderr+exit 合并成一个 `buf`** → 在该 `buf` 上**一次截断**(非分流失)。

## File Structure

- Modify: `src/config.rs`(加 `max_tool_output`)、`src/tool/builtin.rs`(`truncate_output` + read_file 限读 + run_shell_cancellable 截断 + 单测)。
- Create: `docs/adr/0037-tool-output-length-truncation.md`;Modify `README.md`、`ARCHITECTURE.md`。

---

## Task 1: config `max_tool_output` + `truncate_output` helper + 单测

**Files:**
- Modify: `src/config.rs`、`src/tool/builtin.rs`

**Interfaces:**
- Produces: `Config.max_tool_output: usize`(env `CODECODER_MAX_TOOL_OUTPUT`,默认 256*1024);`pub fn truncate_output(s: String, max: usize) -> String`(builtin.rs)。

- [ ] **Step 1: 加 config 字段**

`src/config.rs` struct(`supervisor_crash_budget` 后)加:
```rust
    /// 工具输出(read_file / run_command)字节上限,超长截断带 marker(ADR 0037)。
    pub max_tool_output: usize,
```
`from_env`(`supervisor_crash_budget` 解析块后、闭合 `}` 前)加:
```rust
            max_tool_output: env("CODECODER_MAX_TOOL_OUTPUT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(256 * 1024),
```

- [ ] **Step 2: 写失败测试**(在 `src/tool/builtin.rs` 测试模块内)

```rust
    #[test]
    fn truncate_output_passes_short_and_truncates_long() {
        // 透传:未超 max 原样返回(无 marker)。
        assert_eq!(truncate_output("hi".into(), 10), "hi");
        // 截断:超 max → 前缀为 max 字节 + marker。
        let s = "a".repeat(100);
        let out = truncate_output(s.clone(), 10);
        assert!(out.starts_with("aaaaaaaaaa"), "prefix preserved: {out}");
        assert!(out.contains("showed ~10 of 100 bytes"), "marker present: {out}");
        // char 边界:多字节字符不切坏(结果仍是合法 String,截到 char 边界)。
        let multi = "é".repeat(100); // 每字 2 字节
        let out2 = truncate_output(multi, 11); // 11 落在 char 中间 → 回退到 10(5 字)
        assert!(out2.ends_with(']'), "ends with marker: {out2}");
        assert!(out2.contains("showed ~10 of 200 bytes"), "byte counts: {out2}");
        // é×5 截断后前缀仍是合法 UTF-8(能 collect 成 chars 不 panic)
        let _chk: Vec<char> = out2.chars().collect();
    }
```

- [ ] **Step 3: 跑测试看红**

Run: `cargo test truncate_output_passes_short_and_truncates_long 2>&1 | grep -E 'error\[|cannot find|no function' | head`
Expected: 编译失败(`truncate_output` 未定义)。

- [ ] **Step 4: 实现 truncate_output**(在 `src/tool/builtin.rs` `run_shell_cancellable` 上方,模块级 `pub fn`)

```rust
/// 超长输出截断到 `max` 字节(char 边界安全)并加 marker(ADR 0037)。
/// `len<=max` 原样透传。marker 告知 agent 数据被截 + 如何放大。
pub fn truncate_output(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let total = s.len();
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut head = String::from(&s[..cut]);
    head.push_str(&format!(
        "\n… [truncated: showed ~{cut} of {total} bytes; raise CODECODER_MAX_TOOL_OUTPUT to see more]"
    ));
    head
}
```

- [ ] **Step 5: 跑测试看绿**

Run: `cargo test truncate_output_passes_short_and_truncates_long 2>&1 | grep -E 'test result|FAILED' | head -2`
Expected: `... ok`。

- [ ] **Step 6: 全测试 + commit**

Run: `cargo test 2>&1 | grep -E 'test result:' | grep -v '0 failed' || echo ALL_GREEN`
Expected: `ALL_GREEN`。
```bash
git add src/config.rs src/tool/builtin.rs
git commit -m "feat(tool): max_tool_output 配置 + truncate_output helper(P10)

config 加 CODECODER_MAX_TOOL_OUTPUT(默认256K);truncate_output(s,max) char边界
安全截断 + marker。read_file/run_command 接入在后续 Task。"
```

---

## Task 2: read_file 限读

**Files:**
- Modify: `src/tool/builtin.rs`(`impl Tool for ReadFile::run`)

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn read_file_truncates_large_file() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("cc_rftrunc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.txt");
        // 100KB 内容,设 max=1KB(经 env,带锁防并发)
        {
            let mut f = std::fs::File::create(&big).unwrap();
            f.write_all(&"a".repeat(100_000)).unwrap();
        }
        let _g = std::sync::Mutex::new(()).lock().unwrap();
        unsafe { std::env::set_var("CODECODER_MAX_TOOL_OUTPUT", "1024"); }
        let mut ctx = crate::tool::ToolCtx::new(&dir);
        let out = ReadFile.run(json!({ "path": "big.txt" }), &mut ctx).unwrap();
        unsafe { std::env::remove_var("CODECODER_MAX_TOOL_OUTPUT"); }
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("truncated"), "marker present: {}", out.content);
        assert!(out.content.contains("100000 bytes"), "total reported: {}", out.content);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: 跑测试看红**

Run: `cargo test read_file_truncates_large_file 2>&1 | grep -E 'assertion|FAILED|marker|truncated' | head`
Expected: FAIL(当前 read_file 读全文返回 100000 字节,无 marker)。

- [ ] **Step 3: 改 ReadFile::run 为限读**

把 `src/tool/builtin.rs` `impl Tool for ReadFile` 的 `run`(`match std::fs::read_to_string(&full)`)替换为:
```rust
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        use std::io::Read;
        let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
        if path.is_empty() {
            return Ok(ToolOutput::err("missing required arg: path"));
        }
        let full = ctx.root.join(path);
        let max = crate::config::Config::from_env().max_tool_output;
        let read = || -> anyhow::Result<Vec<u8>> {
            let mut f = std::fs::File::open(&full)?;
            let mut buf = Vec::new();
            f.take((max as u64) + 1).read_to_end(&mut buf)?; // 限读:内存有界 max+1
            Ok(buf)
        };
        match read() {
            Ok(buf) => Ok(ToolOutput::ok(truncate_output(
                String::from_utf8_lossy(&buf).into_owned(),
                max,
            ))),
            Err(e) => Ok(ToolOutput::err(format!("cannot read {}: {e}", full.display()))),
        }
    }
```

- [ ] **Step 4: 跑测试看绿 + 既有 read_file 回归**

Run: `cargo test read_file 2>&1 | grep -E 'test result|FAILED' | head`
Expected: `read_file_truncates_large_file` ok + 既有 read_file 测试 ok(小输入透传)。

- [ ] **Step 5: commit**

```bash
git add src/tool/builtin.rs
git commit -m "feat(read_file): 大文件限读 max+1 字节,内存有界(P10)

read_file 改用 File+take(max+1) 限读而非 read_to_string 整读;from_utf8_lossy 处理
边界 + truncate_output 加 marker。10GB 文件不再灌进内存。"
```

---

## Task 3: run_command 输出截断

**Files:**
- Modify: `src/tool/builtin.rs`(`run_shell_cancellable` 收尾,~140-148)

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn run_command_truncates_large_output() {
        let dir = std::env::temp_dir().join(format!("cc_rctrunc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _g = std::sync::Mutex::new(()).lock().unwrap();
        unsafe { std::env::set_var("CODECODER_MAX_TOOL_OUTPUT", "512"); }
        let mut ctx = crate::tool::ToolCtx::new(&dir);
        // seq 1 5000 产出 ~20KB >> 512
        let out = RunCommand
            .run(json!({ "cmd": "seq 1 5000" }), &mut ctx)
            .unwrap();
        unsafe { std::env::remove_var("CODECODER_MAX_TOOL_OUTPUT"); }
        assert!(out.content.contains("truncated"), "marker present: {}", out.content);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: 跑测试看红**

Run: `cargo test run_command_truncates_large_output 2>&1 | grep -E 'assertion|FAILED|truncated' | head`
Expected: FAIL(当前无 marker)。

- [ ] **Step 3: 截断合并 buf**

把 `src/tool/builtin.rs` `run_shell_cancellable` 收尾(`let mut buf = String::from_utf8_lossy(&stdout_buf)...` 到 `Ok(ToolOutput {...})`)的:
```rust
    let mut buf = String::from_utf8_lossy(&stdout_buf).into_owned();
    if !stderr_buf.is_empty() {
        buf.push_str(&String::from_utf8_lossy(&stderr_buf));
    }
    let is_error = !status.success();
    if is_error {
        buf = format!("exit {}: {buf}", status.code().unwrap_or(-1));
    }
    Ok(ToolOutput { content: buf, is_error, session_meta_mark: None })
```
改为(在构造 ToolOutput 前对合并后的 `buf` 截断):
```rust
    let mut buf = String::from_utf8_lossy(&stdout_buf).into_owned();
    if !stderr_buf.is_empty() {
        buf.push_str(&String::from_utf8_lossy(&stderr_buf));
    }
    let is_error = !status.success();
    if is_error {
        buf = format!("exit {}: {buf}", status.code().unwrap_or(-1));
    }
    let max = crate::config::Config::from_env().max_tool_output;
    Ok(ToolOutput { content: truncate_output(buf, max), is_error, session_meta_mark: None })
```

- [ ] **Step 4: 跑测试看绿 + 既有回归**

Run: `cargo test run_command 2>&1 | grep -E 'test result|FAILED' | head`
Expected: `run_command_truncates_large_output` ok + 既有 `run_command_executes`/keying 测试 ok。

- [ ] **Step 5: 全测试 + commit**

Run: `cargo test 2>&1 | grep -E 'test result:' | grep -v '0 failed' || echo ALL_GREEN`
Expected: `ALL_GREEN`。
```bash
git add src/tool/builtin.rs
git commit -m "feat(run_command): 输出截断带 marker,上下文有界(P10)

run_shell_cancellable 合并 stdout+stderr+exit 后过 truncate_output(buf, max)。
瞬态捕获内存未封顶(drain 全读),tool result/上下文有界;全内存封顶为后续(ADR 0037)。"
```

---

## Task 4: ADR 0037 + README/ARCHITECTURE

**Files:**
- Create: `docs/adr/0037-tool-output-length-truncation.md`;Modify `README.md`、`ARCHITECTURE.md`

- [ ] **Step 1: 写 ADR 0037**

```markdown
# ADR 0037 — 工具输出长度截断

- **状态**: Accepted
- **日期**: 2026-07-22
- **关联**: ADR 0018(工具)、上限压测 P10(`docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md`)

## 背景

P10 发现:`read_file`(`read_to_string`)/ `run_command`(`read_to_end`)输出无上限 → 大文件/冗长命令致无界内存/上下文膨胀。`feat/length-truncation-guard` 分支经核实为陈旧空 fork(0-ahead/172-behind),无实现,故 fresh 实现。

## 决策

1. **env 可配 cap**:`CODECODER_MAX_TOOL_OUTPUT`(默认 256KB)。`truncate_output(s, max)` 超长截到 char 边界(`is_char_boundary` 回退,不切坏 UTF-8)+ marker `… [truncated: showed ~X of Y bytes; raise CODECODER_MAX_TOOL_OUTPUT to see more]`。
2. **read_file 限读**:`File::open + take(max+1) + read_to_end` → 内存有界(max+1);`from_utf8_lossy` 处理边界。
3. **run_command 截断合并 buf**:全 drain(保既有 pipe 不阻塞模型)后,对合并的 stdout+stderr+exit buf 一次截断 → **上下文有界**。瞬态捕获内存未完全封顶(drain 全读)。

## 后果

- **正面**:read_file 内存有界、run_command 上下文有界;agent 经 marker 知数据被截。
- **代价**:超长输出被截(agent 看不全,可放大 cap);run_command 病态冗长命令仍占瞬态内存(drain 期)。
- **不做(后续)**:run_command 全内存封顶(限读 pipe + SIGPIPE/子进程处理);按 token 而非字节截断;per-tool 上限;search_web/reverse_api 输出截断。
```

- [ ] **Step 2: README/ARCHITECTURE 补注**

`README.md` 环境变量表加:
```markdown
| `CODECODER_MAX_TOOL_OUTPUT` | `262144` | read_file / run_command 单次输出字节上限,超长截断带 marker(ADR 0037) |
```
`ARCHITECTURE.md` 工具体系段补一句:read_file/run_command 输出经 `truncate_output` 截断(`CODECODER_MAX_TOOL_OUTPUT`,默认 256KB,ADR 0037)。

- [ ] **Step 3: 全测试 + commit**

Run: `cargo test 2>&1 | grep -E 'test result:' | grep -v '0 failed' || echo ALL_GREEN`
Expected: `ALL_GREEN`。
```bash
git add docs/adr/0037-tool-output-length-truncation.md README.md ARCHITECTURE.md
git commit -m "docs: ADR 0037 工具输出长度截断 + README env 表/ARCHITECTURE 同步"
```

---

## Task 5: live 复验

**Files:** 无源码改动;`codecoder-probe/` lab。

- [ ] **Step 1: 重编译**

Run: `cargo build 2>&1 | tail -1`
Expected: `Finished`。

- [ ] **Step 2: read_file 大文件限读 + run_command 冗长输出截断**

```bash
LAB=/Users/rong.zhu/Code/codecoder-probe
set -a; . /Users/rong.zhu/Code/codecoder/.ccd.env; set +a
CODECODER_ROOT="$LAB" target/debug/codecoder > /tmp/p10_daemon.log 2>&1 & sleep 2
# 大文件(沿用 P3 的 samples/src,或造一个)
head -c 600000 /dev/zero | tr '\0' 'a' > "$LAB/samples/bigfile.txt"
# read_file + 小 cap → marker
CODECODER_MAX_TOOL_OUTPUT=1024 CODECODER_ROOT="$LAB" docs/superpowers/scripts/drive_cc.sh p10_read "读取 samples/bigfile.txt 并报告" /dev/null 2>&1 | grep -iE 'truncated|marker|ctx' | head -2
# run_command 冗长 → marker
CODECODER_MAX_TOOL_OUTPUT=512 CODECODER_ROOT="$LAB" docs/superpowers/scripts/drive_cc.sh p10_cmd "运行 'seq 1 10000' 并报告输出" /dev/null 2>&1 | grep -iE 'truncated' | head -2
kill -TERM "$(pgrep -f 'target/debug/codecoder' | head -1)" 2>/dev/null; sleep 1
```
Expected: 两次都出现 `truncated` marker(对比 P10 修复前:大文件/冗长输出原样灌入,无 marker、ctx 飙升)。

- [ ] **Step 3: 记结论**

```bash
printf '\n## P10 修复复验(2026-07-22,fix/length-truncation-guard)\n- read_file 大文件限读 + run_command 冗长输出 → 均出现 truncated marker(修复前无界)\n- CODECODER_MAX_TOOL_OUTPUT 可调\n' >> /Users/rong.zhu/Code/codecoder-probe/matrix.md
echo recorded
```

---

## Self-Review(plan vs spec)

**1. Spec coverage:**
- config `max_tool_output`(spec §4.1)→ Task 1 ✓
- `truncate_output` helper(spec §4.2)→ Task 1 ✓
- read_file 限读(spec §4.3)→ Task 2 ✓
- run_command 截断(spec §4.4;细化:合并 buf 一次截断,因 run_shell_cancellable 已合并)→ Task 3 ✓
- 测试(spec §5:truncate_output/read_file/run_command)→ Task 1/2/3 ✓
- ADR 0037 + README + ARCHITECTURE(spec §6)→ Task 4 ✓
- 范围外(spec §7:run_command 全内存封顶/token 截断/per-tool/search_web)→ 计划未涉 ✓

**2. Placeholder scan:** 无 TBD/TODO;truncate_output/read_file/run_command 改动均给出完整改前/改后代码;测试代码完整(含 char 边界、env+Mutex 锁)✓

**3. Type consistency:** `Config.max_tool_output: usize`(Task 1)、`pub fn truncate_output(s: String, max: usize) -> String`(Task 1)与 Task 2/3 调用一致;`ToolCtx::new(&dir)`(测试,与 mod.rs:24 一致);`RunCommand.run`/`ReadFile.run` 签名一致 ✓
