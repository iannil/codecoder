# QA 验收报告模板 / Report Template → `.agents/qa-latest.md`

```markdown
# 🔍 QA 验收报告 / QA Acceptance Report

## 摘要 / Summary
**结论**: [ ✅ PASS / ⚠️ NEEDS_FIX / 🛑 REBUILD ]
**项目类型**: [API/CLI/Web/Library]　**E2E**: [执行 / 降级(原因) / 跳过(原因)]
**本轮范围**: [git diff 摘要]

## 一、测试金字塔 / Test Pyramid
| 层 | 命令 | 通过/总数 | 覆盖率 | 说明 |
|----|------|:--------:|:------:|------|
| 单元 | [cmd] | N/N | diff分支 X% | [未覆盖分支 file:line] |
| 集成 | [cmd] | N/N | — | [CRUD/错误路径] |
| E2E | agent-browser/[降级] | N/N | — | [链路名 + 截图路径] |

## 二、覆盖率门禁 / Coverage Gate
| 指标 | 本轮 | 门禁 | 基线 | 判定 |
|------|:----:|:----:|:----:|:----:|
| diff 分支覆盖率 | X% | ≥90% | — | ✅/⚠️ |
| 全局分支覆盖率 | X% | ≥基线 | Y% | ✅/⚠️ |

## 三、问题明细 / Issues
- [严重/建议]：[file:line] — [描述]

## 四、决策建议 / Recommendation
[结论 + 理由 + 后续步骤：提交 / 升维修一次 / 重建]
```
