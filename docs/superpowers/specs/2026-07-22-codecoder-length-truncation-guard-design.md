# 工具输出长度截断 guard — 设计文档

- **日期**: 2026-07-22
- **状态**: 待用户审阅(Pending user review)
- **作者**: Claude Code(brainstorming 产物)
- **起因**: `docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md` P10 发现——master 上 `read_file`(`read_to_string`)与 `run_command` 输出(`read_to_end`)均无长度上限 → 大文件/冗长命令致**无界内存/上下文膨胀**。`feat/length-truncation-guard` 分支经核实为 0-ahead/172-behind 的**陈旧空分支,无任何截断实现** → 须 fresh 实现。
- **关联**: ADR 0018(工具)、`src/tool/builtin.rs`、`src/config.rs`。

## 1. 背景与目标

P10 坐实:read_file 把整文件 `read_to_string` 进工具结果;run_command 把子进程 stdout/stderr `read_to_end` 全捕获。两者无上限 → 一个 10GB 文件或 `yes` 类命令可撑爆内存/上下文。报告建议"合并 feat/length-truncation-guard",但该分支是陈旧空 fork(无实现)。

**目标**: 给 read_file / run_command 输出加**长度截断 guard**——**内存与上下文有界**(read_file 限读;run_command 截断),超长带 marker 提示 agent 数据被截。**不改变正常(小)输出的行为**(透传)。

## 2. 已锁定决策

| 维度 | 决策 |
|---|---|
| cap 配置 | **A: env `CODECODER_MAX_TOOL_OUTPUT`,默认 256 KB**(256×1024) |
| read_file | **限读**(`File + take(max+1) + from_utf8_lossy`)→ 内存有界 max+1 |
| run_command | 全 drain(保既有 pipe 不阻塞模型)后**截断带 marker** → 上下文有界 |
| marker | `… [truncated: showed ~X of Y bytes; raise CODECODER_MAX_TOOL_OUTPUT to see more]` |
| ADR | **新建 ADR 0037** |

**为何 read_file 限读、run_command 截断(不对称)**:read_file 读磁盘,`take(max+1)` 天然限内存且零代价;run_command 子进程往 pipe 写,drain-on-threads 模型下"读到上限就停"需处理 SIGPIPE/子进程阻塞,复杂——v1 先**截断 tool result(挡住 LLM 可见膨胀)**,瞬态捕获内存未完全封顶记为后续(ADR 0037 明示)。

## 3. 架构

```
config.rs: Config { max_tool_output: usize }   // CODECODER_MAX_TOOL_OUTPUT,默认 256*1024

builtin.rs:
  truncate_output(s: String, max: usize) -> String
    len<=max → s 透传
    len> max → 截到 char 边界(is_char_boundary 回退,不切坏 UTF-8)+ marker

  ReadFile::run: File::open → take(max+1).read_to_end → from_utf8_lossy → truncate_output
  run_shell_cancellable: 全 drain stdout/stderr → 各 truncate_output(从 Vec<u8> lossy 转 String 后)
```

**不变量**:正常(≤max)输出**原样透传**(marker 不出现);仅超长才截断 + marker。`ToolCtx` 不改签名(guard 经 `Config::from_env().max_tool_output` 取上限)。

## 4. 实现

### 4.1 config.rs

`Config` struct 加字段 + `from_env` 解析:
```rust
    /// 工具输出(read_file / run_command)字节上限,超长截断带 marker(ADR 0037)。
    pub max_tool_output: usize,
```
```rust
            max_tool_output: env("CODECODER_MAX_TOOL_OUTPUT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(256 * 1024),
```

### 4.2 builtin.rs — `truncate_output`

```rust
/// 超长输出截断到 `max` 字节(char 边界安全)并加 marker(ADR 0037)。
/// len<=max 原样透传。marker 告知 agent 数据被截 + 如何放大。
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

### 4.3 ReadFile::run — 限读

```rust
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        use std::io::Read;
        let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
        if path.is_empty() {
            return Ok(ToolOutput::err("missing required arg: path"));
        }
        let full = ctx.root.join(path);
        let max = crate::config::Config::from_env().max_tool_output;
        match std::fs::File::open(&full).and_then(|mut f| {
            let mut buf = Vec::new();
            f.take((max as u64) + 1).read_to_end(&mut buf)?; // 限读:内存有界 max+1
            Ok(buf)
        }) {
            Ok(buf) => Ok(ToolOutput::ok(truncate_output(
                String::from_utf8_lossy(&buf).into_owned(),
                max,
            ))),
            Err(e) => Ok(ToolOutput::err(format!("cannot read {}: {e}", full.display()))),
        }
    }
```

### 4.4 run_shell_cancellable — 截断 stdout/stderr

在 `stdout_buf`/`stderr_buf` join 后、构造 ToolOutput 前:
```rust
    let max = crate::config::Config::from_env().max_tool_output;
    let stdout_s = truncate_output(String::from_utf8_lossy(&stdout_buf).into_owned(), max);
    let stderr_s = truncate_output(String::from_utf8_lossy(&stderr_buf).into_owned(), max);
```
(后续用 `stdout_s`/`stderr_s` 替代原始 buffer 拼装 ToolOutput。)

## 5. 测试(TDD)

- **`truncate_output` 单测**:
  - `"hi"`(max=10)→ `"hi"`(透传)。
  - `"a".repeat(100)`(max=10)→ 以 marker 结尾、含 `showed ~10 of 100`、前缀是 10 个 `a`。
  - 多字节(`"é".repeat(100)` 即每字 2 字节,max=11)→ 截到 char 边界(5 字 =10 字节),marker,不切坏 UTF-8(结果仍是合法 String)。
- **read_file 大文件限读**:tempdir 写 `(max+100)` 字节文件 → read_file 返回 len<=max+marker,marker 含 total。
- **run_command 大输出截断**:`seq 1 100000`(或 `yes` 限时)→ 输出截断带 marker。
- **回归**:既有 read_file/run_command 测试(小输入)透传不变。

## 6. ADR / 文档

- **新 ADR 0037《工具输出长度截断》**:cap 策略(env 可配 256K 默认)、read_file 限读 vs run_command 截断的不对称 + 理由、marker 语义、run_command 全内存封顶为后续。
- **README** env 表加 `CODECODER_MAX_TOOL_OUTPUT`;**ARCHITECTURE** 工具段补注。

## 7. 不在本范围内(YAGNI)

- **run_command 全内存封顶**(限读 pipe + SIGPIPE/子进程处理):v1 仅截断 tool result;瞬态捕获内存未封顶,ADR 0037 记为后续。
- **按 token 而非字节截断**:v1 用字节(token 计数留给 compaction);简单。
- **可配置 per-tool 上限**:单一全局 cap 足够。
- **search_web/reverse_api 输出截断**:本次只 read_file + run_command(P10 范围);联网工具输出另议。
- **P11**:独立 spec。
