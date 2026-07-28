# 级联失败处理 / Cascade Failure Handling

> 当编排器中某个里程碑失败（BLOCKED / SKIPPED）时，定义其依赖链的传播规则。
> 防止"一个功能失败导致整个编排计划混乱"。

## 依赖类型 / Dependency Type

在里程碑定义时，每个依赖标注类型：

| 类型 | 标识 | 含义 | 示例 |
|:----:|:----:|------|------|
| **hard** | 默认 | 必须完成才能开始下游。下游无法在上游缺失时独立工作 | M1 创建了 User 表，M2 的 API 需要 User 表才能工作 |
| **soft** | `(soft)` | 建议但非必需。下游可以独立启动，功能可能部分受限 | M3 添加通知功能，M4 的 UI 可以没有通知也能渲染 |

## 级联传播规则 / Cascade Propagation Rules

| 上游状态 | 直接下游默认行为 | 间接下游默认行为 | 用户可覆盖 |
|:--------:|:---------------:|:---------------:|:---------:|
| `DONE` | 正常执行 | 正常执行 | — |
| `DEGRADED` | 继续执行（标记"上游降级"） | 继续执行 | 可暂停审查 |
| `BLOCKED` | hard 依赖 → 自动标记 `BLOCKED (cascade)` | 同左递归 | 可强制继续 |
| `BLOCKED` | soft 依赖 → 保持 `TODO`，标记"上游阻塞" | 保持 `TODO` | — |
| `SKIPPED` | hard 依赖 → `BLOCKED (cascade)` | 同左递归 | 可改 soft |
| `SKIPPED` | soft 依赖 → 保持 `TODO`，标记"上游跳过" | 保持 `TODO` | — |
| `IN_PROGRESS` | 排队等待 | 排队等待 | 可调整顺序 |

## 级联影响报告 / Cascade Impact Report

当发生级联阻塞时，在最终报告中输出依赖树截断摘要：

```markdown
## ⛓️ 级联影响 / Cascade Impact

M2 (BLOCKED: 编译失败) ─hard──→ M3 (BLOCKED by cascade)
                                 └─soft──→ M5 (TODO, unaffected)

M4 (SKIPPED: 用户要求跳过) ─hard──→ M6 (BLOCKED by cascade)

**摘要**: 6 个里程碑中，因级联影响额外阻塞 2 个。
**建议**: 修复 M2 后级联阻塞自动解除。
```

## 恢复策略 / Recovery Strategy

1. **修复根因** — 解决 BLOCKED 的根因里程碑后，级联 BLOCKED 的里程碑自动恢复为 TODO
2. **强制继续** — 用户可选择将 hard 依赖临时降级为 soft，继续执行下游
3. **重排计划** — 如果大量里程碑级联阻塞，建议重新审视蓝图并重排里程碑
