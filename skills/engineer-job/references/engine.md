# engineer-job 执行引擎参考 / Execution Engine Reference

> `run.wf.js` 的设计原理、使用方式和扩展指南。
>
> **Updated for skill-protocol v1.0** — See `references/skill-protocol.md` for the
> standardized inter-skill communication protocol.
>
> Metadata schema: `references/project-metadata.schema.json`

---

## 设计原理 / Design Principles

`run.wf.js` 是一个 Workflow 脚本，利用 Workflow 工具的 `agent()` / `phase()` / `log()` API 实现多阶段编排。

### 四个核心原则

1. **文件即状态** — 所有进度写入 `.agents/` 目录和 `project-metadata.json`，不依赖对话上下文
2. **阶段即子代理** — 每个 Phase 是一个独立 agent() 调用，上下文干净
3. **协议即管道** — Phase 间通过 `project-metadata.json` 传递结构化数据，不依赖自然语言摘要
4. **降级优于阻塞** — 遇错优先降级/跳过，而非停下等人

### 协议数据流 / Protocol Data Flow

```
User (args.requirements)
    │
    ▼
Phase 0: init-project ──► project-metadata.json ──► Phase 1: engineer-architect
                                                           │
                                                           ▼
                                                    CONTEXT.md (blueprint)
                                                           │
                                                           ▼
Phase 2: engineer-orchestrator ──► .agents/progress.json ◄── engineer-workflow × N
                                                           │
                                                           ▼
Phase 3: Integration  ──► test report (recorded, non-blocking)
Phase 4: Deploy       ──► Dockerfile / CI config (non-blocking)
Phase 5: Report       ──► Final completion report
```

### 自动复杂度检测（Component 2）

Phase 0 读取 `args.requirements` 后，`run.wf.js` 用 `detectComplexity()` 启发式判定 `has_frontend` / `skip_requirements` / `skip_frontend`（显式 `args.skip_*` 永远优先）。检测结果写入 `project-metadata.json`，并据 `skip_requirements` / `skip_frontend` 分别门禁 Phase 1 与 Phase 3。纯函数单一真源在 `references/detection-logic.js`，由 `tests/test-detection.mjs` 守护，并内联进 `run.wf.js`（沙箱禁止 require）。

---

## 使用方式 / Usage

通过 Workflow 工具调用（**推荐**）：

```javascript
Workflow({
  script: "skills/engineer-job/run.wf.js",
  args: {
    requirements: "做一个博客系统，支持文章的CRUD，使用Python FastAPI，SQLite存数据",
    mode: "auto",           // normal (默认) | auto | silent
    projectName: "blog-system"
  }
})
```

这是 **engineer-job 的主执行路径**。不要手动逐个调用子 Agent — 在 `--auto` 和 `--silent` 模式下，
Workflow 会自动推进所有 6 个 Phase 并在完成后输出最终报告。

如果项目已开始但中途中断，重新调用 Workflow（同样的 `run.wf.js`）会自动检测 `job.state.json`
并从上一次中断处继续：

```javascript
// 恢复示例 — 自动检测 .agents/job.state.json
Workflow({
  script: "skills/engineer-job/run.wf.js",
  args: {
    requirements: "同上",
    mode: "auto",
    projectName: "blog-system"
  }
})
```

---

## 模式说明 / Mode Behavior

| 模式 | Phase 间推进 | 异常处理 |
|:----:|-------------|---------|
| normal | Phase 间自动推进（Workflow 工具自带节流） | 报告错误，降级后继续，最终报告中标明 |
| auto | 自动推进，最终报告输出降级记录 | 自动降级/跳过，记录到最终报告 |
| silent | 自动推进，无中间输出 | 静默处理，只记日志 |

---

## 6 个 Phase 详解

### Phase 0: Scaffold (init-project)

**输入**: `args.requirements` + `args.projectName`

**输出**: 
- 完整的项目文件树（通用结构 + 语言特定模板）
- `project-metadata.json` — 结构化技术栈信息（后续 Phases 的输入）

**失败处理**: 重试 1 次 → 仍然失败 → 终止（Phase 0 不可降级）

### Phase 1: Architect (engineer-architect)

**输入**: `project-metadata.json`（Phase 0 的输出）

**输出**:
- `CONTEXT.md` — 完整的项目蓝图（系统全景、词汇表、数据模型、API 契约、里程碑、部署方案）
- `docs/adr/` — 架构决策记录
- `project-metadata.json` — 追加 `architect` 节（里程碑列表、词汇表、前端设计方向）

**失败处理**: 重试 1 次 → 生成骨架蓝图（降级通过）

### Phase 2: Develop (engineer-orchestrator + engineer-workflow)

**输入**: `CONTEXT.md` + `project-metadata.json`

**输出**:
- `.agents/progress.json` — 里程碑级进度跟踪
- 完成的项目代码（按里程碑依赖顺序生成）
- 更新后的 `CONTEXT.md`（里程碑状态标记为完成）

**失败处理**: 里程碑级自愈（重建 ≤ 2 次 → 降级 → 跳过）

### Phase 2.5: Run Gate (hard gate)

**新增（Component 4）** — build + test 必须通过。

- agent 读 `project-metadata.json` 的 language/framework，优先用项目原生配置（Makefile / package.json / pyproject.toml / Cargo.toml / go.mod），回退查 `references/build-commands.json`。
- 真跑 build + test（通过 agent 的 Bash 工具）。
- 失败 → 强制修复循环（normal=2 / auto=1 / silent=1 次）。
- 修不动 → 标 `DOES_NOT_RUN`，最终报告头条如实标注，**不宣称完成**。
- 覆盖率门禁：本轮 diff 分支覆盖率 ≥90%，全局 ratchet 不回退（委托 engineer-qa ②③层，见其 references/coverage-tools.md）。

### Phase 3: Integrate

**非阻塞阶段** — 记录结果但不阻塞后续 Phase

**检查项**:
1. 编译/构建检查
2. 全部测试运行
3. 生产就绪检查（仅服务端/Web 应用）
4. agent-browser E2E 关键用户链路负载一次（无 UI 降级为黑盒，委托 engineer-qa ④层）。

### Phase 4: Deploy

**非阻塞阶段** — 生成但不验证部署配置

**生成项**:
- Dockerfile（如适用）
- docker-compose.yml（如多服务）
- .github/workflows/（如 CI/CD 配置）
- .env.example（环境变量模板）

### Phase 5: Report

输出包含以下内容的最终报告：
1. 每阶段完成状态
2. 里程碑完成表
3. 文件变更摘要
4. 测试结果
5. 部署配置
6. 词汇表审计
7. 降级日志
8. 后续建议

---

## 恢复流程 / Recovery

重启时自动检测：

```
Check .agents/job.state.json
  ├── exists → read current phase
  │   └── for each phase:
  │       ├── DONE → skip
  │       ├── BLOCKED → report, ask what to do
  │       └── TODO/IN_PROGRESS → execute
  └── not exists → check project-metadata.json / CONTEXT.md
       └── nothing → fresh start
```

### 恢复操作

1. 读取 `job.state.json` — 确定当前阶段和下一个动作
2. 验证 git 状态 — `git log --oneline -3`，与 `checkpoint.last_commit` 比对
3. 运行测试 — 确认现有代码状态正常
4. 读取 `project-metadata.json` — 获取最新的项目元信息
5. 生成恢复报告 — 展示当前进度和下一个动作

---

## 扩展指南 / Extending

### 新增 Phase

在 `run.wf.js` 的 main loop 末尾追加：

```javascript
// New Phase: Security Audit
phase('Security')
if (!isDone(state, 'security')) {
  const result = await agent(
    `Security audit prompt...`,
    { schema: PHASE_RESULT, label: 'security-audit', phase: 'Security' }
  )
  markPhase(state, 'security', result?.status || 'DONE', result)
  state.checkpoint.next_action = 'next phase'
  writeState(state)
}
```

不要忘记在 `meta.phases` 数组中添加对应的 phase title。

### 新增 phase 到 meta

```javascript
phases: [
  ...
  { title: 'Security', detail: 'security audit scanning' },
]
```

### 修改 Phase 逻辑

编辑对应 phase 的 `agent()` prompt。注意保持 `PHASE_RESULT` schema 的兼容性。

### 自定义降级策略

在每个 phase 的 `if (result?.status === 'BLOCKED')` 分支中修改处理逻辑：
- 可修复错误 → 重试一次
- 不可修复但可降级 → 降级通过
- 不可修复且不可降级 → 抛出异常

---

## 兼容性 / Compatibility

| 组件 | 版本 | 备注 |
|------|:----:|------|
| `run.wf.js` | 2.0 | skill-protocol v1.0 实现 |
| `job.state.json` | 2.0 | 与 `engineer-orchestrator` 兼容 |
| `project-metadata.json` | 1.0 | 新增标准协议文件 |
| Workflow 工具 | 任意 | 需要 `agent()` / `phase()` / `log()` API |
| Node.js | ≥18 | 仅用于文件 I/O，非运行时依赖 |

### 与 engineer-orchestrator 的兼容

`job.state.json` 的 `development.features` 结构与 `engineer-orchestrator` 的 `progress.json` 完全兼容。
检测优先级：`job.state.json` → `progress.json` → `CONTEXT.md` → 从用户问起。
