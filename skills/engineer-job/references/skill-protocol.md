# Skill Protocol — 标准化技能间调用协议

> 定义所有 engineer-* 系列技能的标准输入/输出参数协议，
> 确保技能之间可程序化组合、自动编排。
>
> 协议版本: 1.0
> 兼容性: engineer-job >= 2.0, engineer-orchestrator >= 2.0

---

## 设计原则

1. **每个技能的输出 = 下一个技能的输入** — 不出现在同一技能同时读写同一文件
2. **结构化数据 > 自然语言** — 技能间传递 JSON 格式的 Schema，不依赖自然语言摘要
3. **文件即管道** — 技能间的数据传递通过文件系统（`project-metadata.json`），不通过对话历史
4. **向后兼容** — 新增字段不能破坏已有消费方

---

## 协议概览

```
  User (输入: requirements)
    │
    ▼
  init-project ──输出──► project-metadata.json ──► engineer-architect
                                                          │
                                                          ▼
                                                   CONTEXT.md
                                                      │
                                                      ▼
                                              engineer-orchestrator
                                                      │
                                                      ▼
                                            .agents/progress.json
                                                      │
                                                      ▼
                                              engineer-workflow × N
```

---

## 1. init-project → 其输出

init-project 完成后，必须输出 `project-metadata.json` 到项目根目录。

### 输出 Schema: `project-metadata.json`

```json
{
  "project": {
    "name": "string — 项目名",
    "description": "string — 一句话描述",
    "type": "api-server | cli-tool | library | web-app | desktop-app | mobile-app | other",
    "language": "string — 主要语言 (python | rust | go | typescript | java | ...)",
    "framework": "string — 主要框架 (fastapi | actix-web | gin | nextjs | spring-boot | ...)",
    "package_manager": "string — 包管理器 (pip | cargo | npm | go-mod | maven | ...)",
    "database": "string | null — 数据库 (sqlite | postgresql | mysql | mongodb | null)",
    "has_frontend": "boolean — 是否有前端界面",
    "frontend_framework": "string | null — 前端框架 (react | vue | nextjs | null)",
    "deployment": {
      "type": "local | docker | serverless | cloud-vm",
      "ci_cd": "github-actions | gitlab-ci | manual | none",
      "docker": "boolean — 是否需要 Docker"
    },
    "testing": {
      "framework": "string — 测试框架名",
      "types": ["unit", "integration", "e2e"]
    },
    "license": "string — MIT | Apache-2.0 | GPL-3.0 | ...",
    "features": ["string — 用户描述的核心功能列表"]
  },
  "progress_file": ".agents/job.state.json",
  "blueprint_file": "CONTEXT.md"
}
```

### 必须遵守的规则

1. init-project **必须**在文件树生成结束后写入 `project-metadata.json`
2. 下次读取时直接解析 JSON，不再从文件树推断
3. 如果用户手动修改了技术栈，需同步更新此文件

---

## 2. engineer-architect → 其输出

architect 完成后，蓝图（CONTEXT.md）已经提交。此外还必须输出或更新：

### 输出文件: `project-metadata.json`（更新）

architect 在 init-project 输出的基础上补充以下字段：

```json
{
  "project": { "...原有字段..." },
  "architect": {
    "blueprint_commit": "string — 蓝图提交的 git hash",
    "milestones": [
      {
        "id": "M1",
        "name": "string — 里程碑名",
        "description": "string — 描述",
        "dependencies": ["string — 依赖的里程碑 ID"],
        "type": "data-model | business-logic | api | cross-cutting | frontend | integration",
        "estimated_files": "number",
        "estimated_lines": "number",
        "acceptance_criteria": ["string — 验收项列表"]
      }
    ],
    "glossary_terms": [
      {
        "term_cn": "中文术语",
        "term_en": "English Term",
        "definition": "一句话定义",
        "boundary": "不属于什么"
      }
    ],
    "frontend_direction": {
      "has_frontend": "boolean",
      "design_defined": "boolean",
      "design_vibe": "string | null — 设计基调描述",
      "primary_color": "string | null",
      "typography_heading": "string | null",
      "typography_body": "string | null"
    },
    "adr_count": "number — ADR 数量"
  }
}
```

### 必须遵守的规则

1. architect 完成后必须更新 `project-metadata.json` 的 `architect` 部分
2. 里程碑的 ID 必须连续编号（M1, M2, M3...）
3. `dependencies` 只引用已定义的里程碑 ID

---

## 3. engineer-orchestrator → 输入/输出

orchestrator 读取 `CONTEXT.md` + `project-metadata.json`，写入 `progress.json`。

### 输入文件

| 文件 | 用途 |
|------|------|
| `CONTEXT.md` | 蓝图定义（里程碑、词汇表、API 契约） |
| `project-metadata.json` | 技术栈、里程碑列表、验收标准 |

### 输出文件: `.agents/progress.json`

```json
{
  "project": "string — 项目名",
  "blueprint_version": "string — 蓝图版本",
  "progress": {
    "total_features": "number",
    "completed": "number",
    "blocked": "number",
    "skipped": "number"
  },
  "checkpoint": {
    "last_git_commit_hash": "string",
    "last_git_commit_message": "string",
    "last_verified_commit_hash": "string",
    "last_session_summary": "string — 上次对话摘要"
  },
  "features": {
    "M1": {
      "name": "string",
      "status": "todo | in_progress | done | blocked | skipped",
      "dependencies": ["string"],
      "rebuild_count": "number",
      "degraded": "boolean",
      "integration_issues": ["string"],
      "completed_at": "string | null — ISO 8601",
      "summary": "string | null",
      "commit_hash": "string | null",
      "files_changed": "number",
      "tests_passed": "number"
    }
  }
}
```

---

## 4. engineer-workflow → 返回值协议

当 engineer-workflow 被 orchestrator 调用时，其**返回值**必须遵循以下协议：

### 返回值 Schema

```json
{
  "status": "done | done_with_concerns | blocked",
  "summary": "string — 功能完成的简短摘要",
  "changes": {
    "files_created": "number",
    "files_modified": "number",
    "tests_passed": "number",
    "tests_total": "number",
    "commit_hash": "string | null",
    "rebuild_count": "number",
    "degraded": "boolean"
  },
  "issues": [
    {
      "severity": "warning | error",
      "description": "string",
      "file": "string | null",
      "resolved": "boolean"
    }
  ],
  "glossary_changes": {
    "new_terms": ["string — 新增术语"],
    "updated_terms": ["string — 更新术语"],
    "issues": ["string — 术语问题"]
  }
}
```

### 必须遵守的规则

1. **退出码必须精确**：`done` = 完全成功；`done_with_concerns` = 有降级但有可用产出；`blocked` = 无法继续
2. **所有字段必须填充**：即使为 0 / null / empty array
3. **commit_hash** 在 `done` 状态下不能为 null（必须提交了代码）

---

## 5. engineer-inspector → 返回值协议

inspector 验收后的返回值遵循以下协议：

### 返回值 Schema

```json
{
  "verdict": "pass | needs_fix | rebuild",
  "summary": "string — 验收结论摘要",
  "signals": {
    "foundation_tampering": "clean | warning | critical",
    "over_engineering": "clean | warning | critical",
    "volume_control": "clean | warning | critical",
    "terminology_drift": "clean | warning | critical",
    "frontend_compliance": "clean | warning | skipped",
    "test_compliance": "compliant | insufficient | failed"
  },
  "acceptance_check": {
    "total": "number",
    "passed": "number",
    "failed": "number"
  },
  "issues": [
    {
      "severity": "critical | warning | info",
      "signal": "foundation | overengineer | volume | terminology | frontend | test",
      "file": "string | null",
      "line": "number | null",
      "description": "string",
      "recommendation": "string"
    }
  ],
  "changes_summary": {
    "total_files": "number",
    "total_additions": "number",
    "total_deletions": "number",
    "test_count": "number",
    "test_pass_count": "number"
  }
}
```

---

## 6. 通用执行上下文传递

每个 agent() 调用（无论是 Workflow 脚本还是手动调用）的标准入参结构：

```json
{
  "blueprint": "CONTEXT.md (by reading the file)",
  "project_metadata": "project-metadata.json (by reading the file)",
  "mode": "normal | auto | silent",
  "current_milestone": {
    "id": "M1",
    "name": "milestone name"
  }
}
```

这些信息通过**文件读取**（而非对话参数）传递给子代理。每个子代理启动时自己读取所需文件。

---

## 7. 协议合规检查清单

发布新技能或修改现有技能时，对照以下清单检查协议合规性：

- [ ] 技能的输出有标准 Schema（JSON 格式）
- [ ] 技能的输入有标准 Schema
- [ ] 输出文件写入磁盘（不通过对话传递）
- [ ] 消费方能通过文件名定位所需数据
- [ ] 所有状态码使用协议定义的标准值
- [ ] 新增字段不会破坏已有消费方
