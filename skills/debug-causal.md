---
name: debug-causal
description: Root-cause analysis using the inference tree — dig layer by layer to the high-leverage cause
---

# Root-Cause Analysis with the Inference Tree

Use the `reason` tool to build a causal tree when debugging a persistent problem ("为什么老是失败").

## Workflow

1. **锚定初始问题**: `reason add question="Why is <problem> happening?"`
2. **逐节点展开**: For each candidate direct cause, `reason add question="<direct cause>?" parent=<parent_id>`
3. **验证后锁定**: `reason status id=<id> status=locked` (only after you have evidence from `reason list`)
4. **标注余量/杠杆**: `reason margin id=<id> margin="<description>" leverage=high|medium|low terminal=excluded|natural_law|boundary`
5. **追溯路径**: `reason trace id=<id>` 查看从根到该节点的完整链
6. **收敛到行动**: 找到高余量×高杠杆的关键节点后，用 `milestone add title="<行动>"` 把诊断转为 Plan 里程碑

## Principles

- **一次一个节点** — 不要一次性摊开整棵树。深挖一条分支后再开新分支（反过早共识）。
- **验证才锁定** — 节点默认为 `hypothesis`，有证据才 `status=locked`。没证据的节点是"猜测"，不是"事实"。
- **末端纪律** — `terminal=excluded`(已验证排除)、`natural_law`(物理/数学约束，无余量)、`boundary`(余量在你的权力边界外)。
- **关键节点 = 高余量 × 高杠杆** — 这才是你应该行动的地方。
- **余量是你可改变的范围**，杠杆是改变它后对初始问题的影响程度。两者都高→值得做。

## Managing Inference Trees

When the agent has multiple ongoing causal trees (different problems being debugged), use `reason add` under the appropriate root or simply start fresh — `reason list` always shows the full tree so you know what exists. Each `causal_tree.json` is per-project, so different projects don't interfere.