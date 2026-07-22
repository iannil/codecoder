# ADR 0037 — 工具输出长度截断

- **状态**: Accepted
- **日期**: 2026-07-22
- **关联**: ADR 0018(工具)、上限压测 P10(`docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md`)

## 背景

P10 发现:`read_file`(`read_to_string`)/ `run_command`(`read_to_end`)输出无上限 → 大文件/冗长命令致无界内存/上下文膨胀。`feat/length-truncation-guard` 分支经核实为陈旧空 fork(0-ahead/172-behind),无实现,故 fresh 实现。

## 决策

1. **env 可配 cap**:`CODECODER_MAX_TOOL_OUTPUT`(默认 256KB)。`truncate_output(s, max, total)` 超长截到 char 边界(`is_char_boundary` 回退,不切坏 UTF-8)+ marker `… [truncated: showed ~X of Y bytes; raise CODECODER_MAX_TOOL_OUTPUT to see more]`。`total` 为**真实**总字节(限读场景由调用方传入,如文件 metadata)。
2. **read_file 限读**:`File::open + metadata(total) + take(max+1) + read_to_end` → 内存有界(max+1);`from_utf8_lossy` 处理边界。
3. **run_command 截断合并 buf**:全 drain(保既有 pipe 不阻塞模型)后,对合并的 stdout+stderr+exit buf 一次 `truncate_output(buf, max, buf.len())` → **上下文有界**。瞬态捕获内存未完全封顶(drain 全读)。

## 后果

- **正面**:read_file 内存有界、run_command 上下文有界;agent 经 marker 知数据被截 + 真实总量。
- **代价**:超长输出被截(agent 看不全,可放大 cap);run_command 病态冗长命令仍占瞬态内存(drain 期)。
- **不做(后续)**:run_command 全内存封顶(限读 pipe + SIGPIPE/子进程处理);按 token 而非字节截断;per-tool 上限;search_web/reverse_api 输出截断。
