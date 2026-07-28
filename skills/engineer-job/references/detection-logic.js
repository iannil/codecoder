// skills/engineer-job/references/detection-logic.js
// 纯函数单一真源。run.wf.js 内联同名函数（见 // mirrored 注释），保持同步。
// 由 tests/test-detection.mjs 守护。

const FRONTEND_SIGNALS = [
  '前端', '界面', '页面', 'ui', 'web app', 'web-app', '小程序', '移动端',
  'dashboard', '仪表盘', '后台管理', '管理后台', 'react', 'vue', 'next.js',
  'nextjs', 'frontend', '网页', '网站',
]
const NO_FRONTEND_SIGNALS = [
  'cli', '命令行', 'library', '库', 'sdk', '脚本', '纯后端', 'api only',
  '纯 api', 'backend only', 'fastapi', 'flask', 'django', 'express', 'spring',
  'gin', 'echo', 'actix', 'rest api', 'api 服务', 'api-server', 'api server',
  '后端接口',
]
const COMPLEX_SIGNALS = [
  '多端', '多模块', '多个角色', '审批', '工作流', 'saas', '多租户',
  '事件驱动', 'bff', '微服务', '跨模块', '多机构', '分销', '权限体系',
]
const SIMPLE_SIGNALS = [
  'crud', '简单', '单个', '工具', '计算器', 'todo', '待办', '脚本',
  '小工具', '单功能', 'single', 'simple', 'library', '库',
]

function detectComplexity(req) {
  const text = String(req || '').toLowerCase()
  const hit = (arr) => arr.some((k) => text.includes(k))

  let has_frontend, fe_reason
  if (hit(NO_FRONTEND_SIGNALS)) {
    has_frontend = false; fe_reason = '命中无前端信号词'
  } else if (hit(FRONTEND_SIGNALS)) {
    has_frontend = true; fe_reason = '命中前端信号词'
  } else {
    has_frontend = true; fe_reason = '无明确信号，保守按含前端处理'
  }

  let skip_requirements, req_reason
  if (hit(COMPLEX_SIGNALS)) {
    skip_requirements = false; req_reason = '命中复杂信号词，保留需求分析'
  } else if (hit(SIMPLE_SIGNALS)) {
    skip_requirements = true; req_reason = '命中简单信号词，跳过需求分析'
  } else {
    skip_requirements = false; req_reason = '无明确信号，保留需求分析（安全默认）'
  }

  const skip_frontend = !has_frontend

  let detected_complexity
  if (hit(COMPLEX_SIGNALS)) detected_complexity = 'complex'
  else if (hit(SIMPLE_SIGNALS)) detected_complexity = 'simple'
  else detected_complexity = 'moderate'

  return {
    detected_complexity,
    has_frontend,
    skip_requirements,
    skip_frontend,
    complexity_reasoning: `${fe_reason}; ${req_reason}`,
  }
}

function topoSort(milestones) {
  const ids = milestones.map((m) => m.id)
  const idSet = new Set(ids)
  for (const m of milestones) {
    for (const d of m.deps || []) {
      if (!idSet.has(d)) throw new Error(`milestone ${m.id} depends on unknown ${d}`)
    }
  }
  const indeg = {}
  const adj = {}
  ids.forEach((id) => { indeg[id] = 0; adj[id] = [] })
  for (const m of milestones) {
    for (const d of m.deps || []) {
      adj[d].push(m.id)
      indeg[m.id]++
    }
  }
  const queue = ids.filter((id) => indeg[id] === 0)
  const order = []
  while (queue.length) {
    const id = queue.shift()
    order.push(id)
    for (const next of adj[id]) {
      indeg[next]--
      if (indeg[next] === 0) queue.push(next)
    }
  }
  if (order.length !== ids.length) throw new Error('cycle detected in milestone dependency graph')
  return order
}

module.exports = { detectComplexity, topoSort }
