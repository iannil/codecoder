# WorkGraph 补齐：system prompt 注入 + review→milestone 回写 + background 循环推进

> 审计报告 `docs/audit/0002-first-class-citizen-analysis-2026-07-19.md` 的 P1 补完。
> 在已有的 `drive_workgraph` 基础上，补齐三个缺口使 workgraph 从"被动数据结构"升为"自动推进闭环"。

## 缺口 1：WorkGraph 状态注入 system prompt

**问题：** `build_system_prompt()` 只注入 AGENTS.md + catalog，不包含 workgraph 当前状态。agent 跑了一轮后不知道还有什么里程碑要做。

**解法：** `build_system_prompt()` 末尾追加 `workgraph::render_for_prompt()`。改动 4 行，无风险。

## 缺口 2：review→milestone 自动回写

**问题：** `drive_workgraph()` 推了里程碑但不管结果。即使 agent 完成任务、产出 `REVIEW VERDICT: pass`，workgraph 里里程碑仍然是 `in_progress`，下一个就绪节点不会被解锁。

**解法：** `drive_workgraph()` 在每个 `process_turn` 返回后，用 `crate::review::parse_review()` 解析最近 assistant 文本，若有 `VERDICT:` 行则自动更新里程碑状态（pass→Done，否则→NeedsFix，记录 verdict 字符串）。

## 缺口 3：Background Agent workgraph 循环推进

**问题：** `run_background()` 只跑一个 turn。如果 workgraph 有多个就绪节点，只完成第一个。

**解法：** `run_background()` 内部增加循环，无显式 task 时最多推进 3 个就绪里程碑。每个 milestone 跑完后解析 verdict 回写。

## 架构

三个缺口各自独立，互不依赖，可并行落地：

```
缺口 1: build_system_prompt() ── 追加 render_for_prompt() 输出
缺口 2: drive_workgraph() ── process_turn 后 parse_review → milestone 回写
缺口 3: run_background() ── 循环 resolve_bg_task → run_one_turn → 回写
```

## 风险

- 缺口 2 的 `parse_review` 可能误解析非 review 的普通文本 → 只检查显式 `VERDICT:` 行，和 `review.rs` 的 `parse_review` 相同逻辑（末位优先），普通文本不会误匹配
- 缺口 3 的循环可能无限推进 → 限制 MAX_AUTO=3，和 `drive_workgraph` 一致