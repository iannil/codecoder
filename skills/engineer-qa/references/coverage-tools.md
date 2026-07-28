# 覆盖率工具链 / Coverage Tools

> engineer-qa ② 单元层读取本文件确定覆盖率命令。优先用项目原生配置里的既有测试命令，
> 本表为回退。目标口径：**本轮变更(diff) 分支覆盖率 ≥ 90%**。

| 语言/框架 | 覆盖率命令（含分支） | 分支支持 |
|-----------|--------------------|:--------:|
| Python (pytest) | `pytest --cov --cov-branch --cov-report=term-missing` | ✅ |
| JS/TS (jest) | `jest --coverage --coverageReporters=text` | ✅ (branches 列) |
| JS/TS (vitest) | `vitest run --coverage` | ✅ |
| JS/TS (c8) | `c8 --branches 90 --check-coverage <cmd>` | ✅ |
| Go | `go test -covermode=count -coverprofile=cover.out ./...` | ⚠️ 行覆盖为主，分支降级告警 |
| Rust | `cargo tarpaulin --out Stdout` | ⚠️ 行覆盖为主，分支降级告警 |
| Java (JaCoCo) | `mvn test`（读 target/site/jacoco，branch counter） | ✅ |

## Diff 分支覆盖计算
1. `git diff --name-only`（回退 `--cached` / `HEAD~1..HEAD`）取变更文件。
2. 跑上表命令产出覆盖率报告。
3. 只统计变更文件/函数的分支命中率；`< 90%` → NEEDS_FIX，列出未覆盖分支 `file:line`。

## 全局 ratchet — `.agents/qa-baseline.json`
```json
{ "line_coverage": 0.0, "branch_coverage": 0.0, "updated_at": "<git-commit-hash>" }
```
- 本轮全局覆盖率 < 基线 → NEEDS_FIX。达标 → 覆盖写回（只升不降）。首次无文件 → 建立基线，仅 diff 门禁。

## 降级
工具不支持分支覆盖 → 用行覆盖按 90% 判 + 报告告警"分支覆盖不可用，已降级为行覆盖"。
