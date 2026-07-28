// skills/engineer-next/references/resume-logic.js
// 纯函数单一真源：项目接续点检测。无 fs / 无 I/O。
// 由 tests/test-resume.test.js 守护。与 engineer-job/references/detection-logic.js 同构。

// 外来项目代码体量评估（场景 6a/6b 分界）。
// 统计时由 CLI 忽略 node_modules/.git/target/dist/build/.venv 等依赖目录。
function assessCodeVolume(stats) {
  const files = (stats && stats.sourceFiles) || 0
  const loc = (stats && stats.totalLoc) || 0
  return (files >= 10 || loc >= 500) ? 'substantial' : 'near-empty'
}

// job.state.json 阶段规范顺序
const PHASE_ORDER = ['init', 'requirements', 'architect', 'frontend', 'poc', 'development', 'run_gate', 'finalize', 'deploy', 'report']
const REINVOKE_PHASES = ['init', 'requirements', 'architect', 'frontend', 'poc']
const CLOSURE_PHASES = ['run_gate', 'finalize', 'deploy', 'report']

function phaseStatus(jobState, name) {
  const p = jobState && jobState.phases && jobState.phases[name]
  return p ? p.status : 'TODO'
}

function featuresHaveMixed(features) {
  if (!features) return false
  const vals = Object.values(features).map((f) => f && f.status)
  const hasDone = vals.includes('DONE')
  const hasPending = vals.some((v) => v === 'TODO' || v === 'IN_PROGRESS')
  return hasDone && hasPending
}

function nextTodoFeature(features) {
  if (!features) return null
  for (const id of Object.keys(features)) {
    const st = features[id] && features[id].status
    if (st === 'TODO' || st === 'IN_PROGRESS') return id
  }
  return null
}

// 重建重调/新建 engineer-job 的参数。
// fresh=true：全新型（6a/7），requirements 留空交由用户/job 补。
// fresh=false：接续型（1a/1c），从 job.state.json + project-metadata.json 重建。
function buildReconstructedArgs(state, fresh) {
  const js = state.jobState
  const meta = state.metadata
  const projectName = (js && js.project) || (meta && meta.project && meta.project.name) || state.cwdName || 'unnamed-project'
  const mode = (js && js.mode) || 'normal'
  let requirements = ''
  let skipRequirements = false
  let skipFrontend = false
  if (!fresh) {
    const desc = meta && meta.project && meta.project.description
    const feats = meta && meta.project && meta.project.features
    requirements = [desc, feats && feats.length ? feats.join('、') : null].filter(Boolean).join('；') || ''
    skipRequirements = !!(meta && meta.project && meta.project.skip_requirements)
    skipFrontend = !!(meta && meta.project && meta.project.skip_frontend)
  }
  return {
    mode,
    projectName,
    requirements,
    skip_requirements: skipRequirements,
    skip_frontend: skipFrontend,
    // 无前端即跳过 POC；project-metadata.json 不单独携带 skip_poc，
    // job 恢复后会按 detectComplexity 自行重导，故此处与 skip_frontend 绑定即可。
    skip_poc: skipFrontend,
    stop_at_poc: false,
  }
}

function detectResumePoint(state) {
  const s = state || {}
  const js = s.jobState

  // 场景 1a–1e：job.state.json 存在
  if (js) {
    const statuses = PHASE_ORDER.map((name) => ({ name, status: phaseStatus(js, name) }))

    // 1e：任一阶段 BLOCKED
    const blocked = statuses.find((p) => p.status === 'BLOCKED')
    if (blocked) {
      return { scenario: '1e', resume_point: `阶段 ${blocked.name} 阻塞`, target_skill: null, handoff: 'report-blocked', reconstructed_args: null, reasoning: `job.state.json 中阶段 ${blocked.name} 为 BLOCKED` }
    }

    // 1d：全阶段 DONE
    if (statuses.every((p) => p.status === 'DONE')) {
      return { scenario: '1d', resume_point: '已完成', target_skill: null, handoff: 'report-complete', reconstructed_args: null, reasoning: 'job.state.json 全部阶段 DONE' }
    }

    const firstNonDone = statuses.find((p) => p.status !== 'DONE')
    const devStatus = phaseStatus(js, 'development')
    const devFeatures = js.phases && js.phases.development && js.phases.development.features

    // 1b：development 为首个未完成阶段（TODO 起步或 IN_PROGRESS）——走 orchestrator 里程碑级恢复，
    // 避免重调 job 在 development 阶段把里程碑状态初始化为全 TODO 重跑。
    if (firstNonDone.name === 'development') {
      const resumeId = nextTodoFeature(devFeatures)
      return {
        scenario: '1b',
        resume_point: resumeId ? `里程碑 ${resumeId}` : '首个里程碑',
        target_skill: 'engineer-orchestrator',
        handoff: 'route-orchestrator',
        reconstructed_args: null,
        reasoning: `development 为 ${devStatus}${featuresHaveMixed(devFeatures) ? '（部分里程碑已完成）' : ''}；走 orchestrator 里程碑级恢复，避免重调 job 重跑已完成里程碑`,
      }
    }

    // 1a：首个未完成阶段在 init/设计期——重调 job Workflow 自动跳过 DONE 阶段
    if (REINVOKE_PHASES.includes(firstNonDone.name)) {
      return { scenario: '1a', resume_point: `阶段 ${firstNonDone.name}`, target_skill: 'engineer-job', handoff: 'reinvoke-job-workflow', reconstructed_args: buildReconstructedArgs(s, false), reasoning: `下一个未完成阶段为 ${firstNonDone.name}；重调 job Workflow 自动跳过 DONE 阶段` }
    }

    // 1c：development 已 DONE，收尾阶段未完成——重调 job 跑 run-gate/integrate/deploy/report（不重跑里程碑）
    if (CLOSURE_PHASES.includes(firstNonDone.name)) {
      return { scenario: '1c', resume_point: `收尾阶段 ${firstNonDone.name}`, target_skill: 'engineer-job', handoff: 'reinvoke-job-workflow', reconstructed_args: buildReconstructedArgs(s, false), reasoning: `development 已 DONE，下一个未完成为收尾 ${firstNonDone.name}；重调 job 跑 run-gate/integrate/deploy/report` }
    }
  }

  // 场景 2：仅有 progress.json
  if (s.progress) {
    return { scenario: '2', resume_point: '下一个 TODO 里程碑', target_skill: 'engineer-orchestrator', handoff: 'route-orchestrator', reconstructed_args: null, reasoning: '无 job.state.json 但存在 progress.json；走 orchestrator 恢复流程' }
  }

  // 场景 3：蓝图就绪未开发
  if (s.context) {
    const multi = !!s.contextMap
    return { scenario: '3', resume_point: multi ? '首个模块' : '首个里程碑', target_skill: 'engineer-orchestrator', handoff: 'route-orchestrator', reconstructed_args: null, reasoning: `蓝图 CONTEXT.md 就绪${multi ? '（多模块，有 CONTEXT-MAP.md）' : ''}；走 orchestrator 开始里程碑` }
  }

  // 场景 4：仅有需求文档
  if (s.requirements) {
    return { scenario: '4', resume_point: '架构设计', target_skill: 'engineer-architect', handoff: 'route-architect', reconstructed_args: null, reasoning: '仅有 REQUIREMENTS.md；路由 architect 生成 CONTEXT.md' }
  }

  // 场景 5：仅脚手架元数据
  if (s.metadata) {
    return { scenario: '5', resume_point: '需求/架构', target_skill: 'engineer-requirements', handoff: 'route-requirements', reconstructed_args: null, reasoning: '仅有 project-metadata.json（脚手架）；路由 requirements（简单项目可由其转 architect）' }
  }

  // 场景 6a/6b/7：零 engineer* 产物（外来项目 / 空目录）
  const stats = s.codeStats || null
  if (stats && assessCodeVolume(stats) === 'substantial') {
    return { scenario: '6b', resume_point: '逆向建模', target_skill: 'engineer-architect', handoff: 'route-architect-reverse', reconstructed_args: null, reasoning: `零 engineer* 产物且代码体量大（${stats.sourceFiles} 文件 / ${stats.totalLoc} LOC）；路由 architect 逆向建模出蓝图` }
  }
  const isEmpty = stats && stats.sourceFiles === 0 && stats.totalLoc === 0
  if (isEmpty) {
    return { scenario: '7', resume_point: '从零开始', target_skill: 'engineer-job', handoff: 'reinvoke-job-workflow', reconstructed_args: buildReconstructedArgs(s, true), reasoning: '完全空目录；路由 engineer-job 全新（无需求则先问最少问题）' }
  }
  return { scenario: '6a', resume_point: '从零开始', target_skill: 'engineer-job', handoff: 'reinvoke-job-workflow', reconstructed_args: buildReconstructedArgs(s, true), reasoning: '零 engineer* 产物且代码近乎为空；路由 engineer-job 全新，既有零散文件当作脚手架' }
}

module.exports = { detectResumePoint, assessCodeVolume }
