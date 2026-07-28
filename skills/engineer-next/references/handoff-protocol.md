# engineer-next 交接协议 / Handoff Protocol

`detectResumePoint` 返回的判决里有 `handoff` 字段，决定 engineer-next 怎么把控制权交给下游。本文逐种 handoff 给出确切指令。所有交接前先输出 §9 状态诊断报告；normal 模式等用户确认，auto/silent 直接交接。交接时把同一 `--mode` 透传给下游。

## 通用前置

1. 读取判决（`node skills/engineer-next/references/detect-resume.js [dir]` 或照 SKILL.md 决策表手判）。
2. 校验 `git rev-parse HEAD` 与 `job.state.json.checkpoint.last_commit`：偏差在诊断报告标 ⚠️，normal 模式询问用户。
3. 输出诊断报告，按 mode 决定是否等待确认。

## reinvoke-job-workflow（场景 1a / 1c / 6a / 7）

重调 `engineer-job` 的 Workflow，**复用它已有的阶段跳过与自愈**，不在 engineer-next 里重做。

- **1a/1c（接续型）**：判决里的 `reconstructed_args` 已从 `job.state.json` + `project-metadata.json` 重建好（mode/projectName/requirements/skip_*）。
- **6a/7（全新型）**：`reconstructed_args.requirements` 为空——**先从用户本次输入补全 requirements**；若用户没给，按 auto/silent 交由 engineer-job 问最少问题。开工前若需求文本明显是纯后端/CLI，可把 `skip_frontend`/`skip_poc` 设 true（参照 `engineer-job/references/detection-logic.js` 的 `detectComplexity`）。

调用形式：

```javascript
Workflow({
  script: "skills/engineer-job/run.wf.js",
  args: {
    requirements: "<reconstructed_args.requirements 或用户输入>",
    mode: "<reconstructed_args.mode>",
    projectName: "<reconstructed_args.projectName>",
    skip_requirements: <bool>,
    skip_frontend: <bool>,
    skip_poc: <bool>,
    stop_at_poc: false,
  }
})
```

Workflow 工具不可用时，退回 engineer-job 的"手动子代理调度"模式，按相同 args 启动。

## route-orchestrator（场景 1b / 2 / 3）

把控制权交给 `engineer-orchestrator` 的恢复流程。**不要重调 engineer-job 的 Workflow**——job 在 development 阶段会把里程碑状态初始化为全 TODO 重跑已完成里程碑，orchestrator 才有里程碑级恢复（读 `.agents/progress.json`）。

启动指令（向 orchestrator 下发）：

```markdown
继续项目 [名称] 的开发。蓝图：CONTEXT.md（当前目录）。从下一个 TODO 里程碑继续：
读取 .agents/progress.json（或 job.state.json.development.features）确定断点；
git log 验证当前 commit；运行测试基线；然后从下一个 TODO 里程碑接续 engineer-workflow。
[有 CONTEXT-MAP.md 时] 这是多模块项目，按 CONTEXT-MAP.md 的开发顺序编排。
```

## route-architect（场景 4）

仅有 `REQUIREMENTS.md`，路由 `engineer-architect` 产出 `CONTEXT.md`：

```markdown
读取当前目录的 REQUIREMENTS.md，生成完整 CONTEXT.md 蓝图（系统概览/技术栈/数据模型/API 契约/里程碑依赖树/词汇表）。
完成后交回 engineer-next（或直接由 orchestrator 接续开发）。
```

## route-architect-reverse（场景 6b）

零 engineer* 产物且代码体量大（外来项目）。遵守"无蓝图不开工"——**先逆向建模，再继续**，不在无蓝图代码上直接开干。

```markdown
这是已有大量代码但无 engineer* 产物的外来项目。执行逆向分析：
1. 扫描现有文件结构与代码，推断技术栈、数据模型、API、模块边界。
2. 生成 REQUIREMENTS.md（从代码反推的角色/功能/状态机）与 CONTEXT.md（既有代码作为地基）。
3. 诚实标注哪些是"明示/observed"、哪些是"推断/inferred"、哪些是"缺口/gap"（缺口列出来问用户）。
完成后交回 engineer-next，按场景 3 路由 orchestrator 接续开发。
```

## route-requirements（场景 5）

只有 `project-metadata.json`（脚手架已就绪），路由 `engineer-requirements` 做需求拆解（产出 REQUIREMENTS.md）。若项目明显简单（单功能/CRUD），可直接路由 `engineer-architect` 一步出 CONTEXT.md。

## report-complete（场景 1d）

`job.state.json` 全阶段 DONE。不交接——输出"项目已完成"，建议：运行 `engineer-advisor` 诊断后续方向，或追加新功能走 `engineer-architect` 更新蓝图。

## report-blocked（场景 1e）

某阶段 BLOCKED。不自动交接——输出阻塞信息（阶段名 + job.state.json 里的错误记录），建议：路由该阶段对应技能带阻塞上下文重试，或 `engineer-advisor` 诊断。normal 模式询问用户选择。

## 参数重建对照表（reconstructed_args）

| 参数 | 接续型(1a/1c)来源 | 全新型(6a/7)来源/兜底 |
|:--|:--|:--|
| mode | jobState.mode | 用户指定 / normal |
| projectName | jobState.project → metadata.project.name → cwdName | 同左兜底链 |
| requirements | metadata.project.description + features 拼接 | 用户本次输入 / 空（交 job 询问） |
| skip_requirements / skip_frontend / skip_poc | metadata 的 skip_*/has_frontend | 默认 false（交 job 自检测） |
| stop_at_poc | 固定 false | 固定 false |
