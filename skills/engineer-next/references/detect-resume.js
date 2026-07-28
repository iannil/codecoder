#!/usr/bin/env node
// skills/engineer-next/references/detect-resume.js
// CLI 包装：读 cwd（或传入目录）的状态指纹 -> 调 detectResumePoint -> 打印 JSON 判决。
// 这是普通 node 脚本（非 workflow 脚本），可以使用 fs。
// 用法: node detect-resume.js [projectDir]

const fs = require('node:fs')
const path = require('node:path')
const { detectResumePoint } = require('./resume-logic.js')

// 统计代码体量时忽略的目录与计数的源码扩展名
const IGNORED_DIRS = new Set([
  'node_modules', '.git', 'target', 'dist', 'build', '.venv', 'venv',
  '__pycache__', '.next', '.cache', 'coverage', '.idea', '.vscode',
])
const CODE_EXT = new Set([
  '.js', '.jsx', '.ts', '.tsx', '.mjs', '.cjs', '.py', '.rs', '.go',
  '.java', '.kt', '.rb', '.php', '.c', '.cc', '.cpp', '.h', '.hpp',
  '.cs', '.swift', '.m', '.vue', '.svelte', '.scala',
])

function readJson(file) {
  try { return JSON.parse(fs.readFileSync(file, 'utf8')) } catch { return null }
}
function exists(file) {
  try { return fs.existsSync(file) } catch { return false }
}

// 递归统计非依赖目录下的源文件数与总行数
function countCode(root) {
  let sourceFiles = 0
  let totalLoc = 0
  const walk = (dir) => {
    let entries
    try { entries = fs.readdirSync(dir, { withFileTypes: true }) } catch { return }
    for (const e of entries) {
      if (e.name.startsWith('.')) continue
      const full = path.join(dir, e.name)
      if (e.isDirectory()) {
        if (!IGNORED_DIRS.has(e.name)) walk(full)
      } else if (CODE_EXT.has(path.extname(e.name).toLowerCase())) {
        sourceFiles++
        try { totalLoc += fs.readFileSync(full, 'utf8').split('\n').length } catch { /* ignore unreadable */ }
      }
    }
  }
  walk(root)
  return { sourceFiles, totalLoc }
}

function main() {
  const root = process.argv[2] ? path.resolve(process.argv[2]) : process.cwd()
  const agents = path.join(root, '.agents')
  const state = {
    cwdName: path.basename(root) || 'unnamed-project',
    jobState: readJson(path.join(agents, 'job.state.json')),
    progress: exists(path.join(agents, 'progress.json')),
    metadata: readJson(path.join(root, 'project-metadata.json')),
    context: exists(path.join(root, 'CONTEXT.md')),
    contextMap: exists(path.join(root, 'CONTEXT-MAP.md')),
    requirements: exists(path.join(root, 'REQUIREMENTS.md')),
    frontendDesign: exists(path.join(root, 'FRONTEND-DESIGN.md')),
    pocManifest: exists(path.join(root, 'POC-MANIFEST.md')),
    codeStats: countCode(root),
  }
  const verdict = detectResumePoint(state)
  process.stdout.write(JSON.stringify(verdict, null, 2) + '\n')
}

main()
