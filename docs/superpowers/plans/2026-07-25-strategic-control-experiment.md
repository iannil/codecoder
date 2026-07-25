# CodeCoder 自主构建战略管控系统 — 实验计划

> **For agentic workers:** N/A — 本计划的"执行者"是 codecoder (Rust AI agent)，而非 Claude 子 agent。Claude 仅负责实验准备和监控。

**Goal:** 验证 codecoder 能否仅凭 `功能清单.md` 和恰当的 AGENTS.md 引导，自主从零构建一个完整的企业级前端应用。

**Architecture:** 这是一个双层架构：
1. **Claude 层**: 准备实验环境（编译 codecoder、创建项目目录、编写 AGENTS.md/ CONTEXT.md/ codecoder.json）
2. **CodeCoder 层** (`BG_WORKGRAPH`): 自主读取使命声明 → 拆解里程碑 → 选择技术栈 → 初始化项目 → 实现全部功能 → 自我验收

**Tech Stack:** 不固定——codecoder 自主决定。可能的选择：React/Vite/Next.js/Vue 等。

---

## Phase 1: 实验环境准备

### Task 1.1: 编译 codecoder release 二进制

**Files:** 无（仅编译）

- [ ] **Step 1: 检查当前 codecoder 目录状态**

```bash
cd /Users/rong.zhu/Code/codecoder
git status  # 确认工作区干净
```

- [ ] **Step 2: 编译 release 版本**

```bash
cargo build --release 2>&1 | tail -20
```

Expected: `Finished release [optimized] target(s) in ...`

- [ ] **Step 3: 确认二进制存在**

```bash
ls -la target/release/cc target/release/ccd
```

---

### Task 1.2: 创建目标项目目录

**Files:**
- Create: `~/Code/strategic-control/`

- [ ] **Step 1: 创建目录并初始化 git**

```bash
mkdir -p ~/Code/strategic-control
cd ~/Code/strategic-control
git init
```

- [ ] **Step 2: 复制 codecoder 二进制**

```bash
cp /Users/rong.zhu/Code/codecoder/target/release/cc ~/Code/strategic-control/
cp /Users/rong.zhu/Code/codecoder/target/release/ccd ~/Code/strategic-control/
```

---

### Task 1.3: 创建 AGENTS.md (codecoder 的使命声明)

**Files:**
- Create: `~/Code/strategic-control/AGENTS.md`

**说明:** AGENTS.md 是 codecoder 的 system prompt，是**最关键的文件**。它需要：
- 明确 codecoder 的身份（资深全栈开发者）
- 给出清晰的总体任务
- 提供技术栈决策自由
- 划定质量底线

- [ ] **Step 1: 撰写 AGENTS.md**

写入内容（如下）：

```markdown
# 身份声明

你是 CodeCoder — 一个高度自主的 AI 软件工程师。你以「文件系统即自我」的方式运作：你的身份和能力由当前项目目录下的文件定义。

---

## 任务总览

你的任务是**从零构建「战略管控系统」(Strategic Management & Control System)** —— 一个面向集团企业的管理信息系统。

你拥有完全的技术栈决策自由（React/Vue/Angular/Vite/Next.js/Tailwind 等均可），但需要遵循以下原则：
1. 构建一个**可运行、可访问**的 Web 应用
2. 完整实现 `功能清单.md` 中描述的所有**六大业务域**的功能
3. 代码清晰、有适当的注释和测试
4. 使用现代前端工程实践（组件化、状态管理、路由、类型安全）

## 项目结构要求

- 源代码存放在合理的目录结构中（如 `src/pages/`、`src/components/`、`src/store/`）
- 每个业务域应有独立模块
- 遵循 master-detail、侧栏导航等常见企业应用布局模式
- 包含必要的测试

## 功能清单索引 (`功能清单.md`)

系统中的所有页面和功能已在 `功能清单.md` 中定义。建议的工作顺序：

1. **项目初始化**（脚手架、路由框架、布局组件）
2. **共享组件**（RichTextEditor、ApprovalPanel、MasterDetailLayout 等）
3. **战略驾驶舱**（首页 Dashboard）
4. **战略规划管理 (PLG-01)** — 最复杂的域，含 8 个子分组
5. **机构管理 (INST-02)**
6. **组织绩效管理 (PERF-03)**
7. **制度管理 (POL-04)**
8. **智能与数据中枢 (INT-05)**
9. **系统管理 (SYS-06)**

优先实现核心功能（页面渲染 + 路由 + 基础交互），再补充 AI/集成等示意性功能。

## 工作方式

- 使用 `milestone` 工具管理工作进度图
- 每个功能模块创建独立的里程碑
- 频繁提交 git，提交信息使用常规提交规范（feat/fix/docs 等）
- 使用 `plan` 工具对复杂模块进行前置设计
- 遇到不清楚的需求时，使用 `search_web` 查询最佳实践
- 对于 `功能清单.md` 中标记为"(示意)"的功能，实现轻量占位页面即可
- 对于"侧栏暂隐藏"的页面，注册路由但不加入主导航

## 自我验收

每完成一个里程碑，均应：
1. 确认应用可以成功构建 (`npm run build` 或等效命令)
2. 运行测试 (`npm test` 或等效命令)
3. 如有必要，启动开发服务器验证页面可渲染
4. git add + git commit 将该里程碑的成果固化
```

- [ ] **Step 2: 验证 AGENTS.md 已正确写入**

---

### Task 1.4: 创建 CONTEXT.md (领域术语表)

**Files:**
- Create: `~/Code/strategic-control/CONTEXT.md`

- [ ] **Step 1: 撰写 CONTEXT.md**

写入内容：

```markdown
# 战略管控系统 — 领域术语表

## 业务域

| 编号 | 业务域 | 模块编号 | 说明 |
|------|--------|----------|------|
| 一 | 战略驾驶舱 (Strategy Cockpit) | — | 集团决策层仪表盘，系统落地页 |
| 二 | 战略规划管理 (Strategic Planning) | PLG-01 | 产业研究、规划编制、审批、分解、投资、评估、推演 |
| 三 | 机构管理 (Institution Management) | INST-02 | 法人注册、变更、注销、台账 |
| 四 | 组织绩效管理 (Organizational Performance) | PERF-03 | 考核方案、过程考核、指标管理、年度/半年度考核 |
| 五 | 制度管理 (Policy Management) | POL-04 | 制度分类、标签、全文检索 |
| 六 | 智能与数据中枢 (Intelligence & Data Hub) | INT-05 | AI能力中心、数据集成治理、通知预警 |
| 七 | 系统管理 (System Management) | SYS-06 | 用户权限、流程引擎、统一门户 |

## 页面路由前缀

| 域 | 路由前缀 |
|----|---------|
| 战略驾驶舱 | `/dashboard` |
| 战略规划 | `/planning/*` |
| 机构管理 | `/institution/*` |
| 组织绩效 | `/performance/*` |
| 制度管理 | `/policy/*` |
| 智能与数据中枢 | `/intelligence/*` |
| 系统管理 | `/system/*` |
| 个人工作台 | `/workbench` |

## 公共组件

- RichTextEditor: 基于 TipTap 的富文本编辑器
- AiCopilotRail: AI 辅助侧栏
- ApprovalPanel: 通用审批操作面板
- SectionConsole: 二级分组/子域控制台侧栏导航
- MasterDetailLayout: 主列表-详情通用布局
- VersionDrawer: 版本历史抽屉
- CommentsDrawer: 文档批注面板
- CitationPanel: 引用来源管理面板

## 权威来源

- 功能清单: `功能清单.md` — 唯一权威的功能定义
- 本术语表补充功能清单中未明确定义的术语
```

- [ ] **Step 2: 验证 CONTEXT.md 已正确写入**

---

### Task 1.5: 复制功能清单 + 创建 codecoder.json + .ccd.env

**Files:**
- Copy to: `~/Code/strategic-control/功能清单.md`
- Create: `~/Code/strategic-control/codecoder.json`
- Create: `~/Code/strategic-control/.ccd.env`

- [ ] **Step 1: 复制功能清单**

```bash
cp /Users/rong.zhu/Code/codecoder/docs/proof/功能清单.md ~/Code/strategic-control/
```

- [ ] **Step 2: 创建 codecoder.json（预授权开发工具）**

```json
{
  "allowlist": [
    "write_file",
    "edit_file",
    "read_file",
    "run_command:npm",
    "run_command:npx",
    "run_command:node",
    "run_command:git",
    "run_command:ls",
    "run_command:mkdir",
    "run_command:cp",
    "run_command:mv",
    "run_command:cat",
    "run_command:cd",
    "run_command:pwd",
    "run_command:vite",
    "run_command:tsc",
    "glob",
    "grep",
    "commit",
    "search_web",
    "generate_skill",
    "use_skill",
    "plan",
    "agent"
  ]
}
```

- [ ] **Step 3: 创建 .ccd.env**

```
# 注意: API_KEY 和 API_BASE 需要从 shell 环境变量传入
# .ccd.env 不会注入这两项（安全限制）
CODECODER_MODEL=deepseek-v4-flash
CODECODER_MAX_TOKENS=8192
```

- [ ] **Step 4: 创建 .gitignore**

```
node_modules/
dist/
target/
*.log
.DS_Store
cc
ccd
```

- [ ] **Step 5: 初始 git 提交**

```bash
cd ~/Code/strategic-control
git add -A
git commit -m "feat: init experiment scaffold

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Phase 2: 启动 codecoder 自主构建

### Task 2.1: 环境变量确认

- [ ] **Step 1: 确认 API Key 在 shell 环境中可用**

```bash
echo "API_KEY set: $([ -n \"$CODECODER_API_KEY\" ] && echo YES || echo NO)"
echo "API_BASE: ${CODECODER_API_BASE:-not set}"
```

如果未设置，需要先导出：
```bash
export CODECODER_API_KEY=sk-...  # 从 .ccd.env 读取
export CODECODER_API_BASE=https://api.deepseek.com
```

- [ ] **Step 2: 记录启动前快照**

```bash
cd ~/Code/strategic-control
echo "=== Starting CodeCoder BG_WORKGRAPH ==="
date -u "+%Y-%m-%dT%H:%M:%SZ"
echo "Project: ~/Code/strategic-control"
echo "Model: deepseek-v4-flash"
```

---

### Task 2.2: 运行 BG_WORKGRAPH

- [ ] **Step 1: 启动 codecoder 自主构建**

```bash
cd ~/Code/strategic-control
export CODECODER_DEFAULT_TRUST=always
CODECODER_BG_WORKGRAPH=1 \
  CODECODER_BG_MAX_FIX_ATTEMPTS=3 \
  ./ccd 2>&1 | tee ccd-output.log
```

**预期行为:**
1. ccd 启动 → 读取 AGENTS.md → 了解使命
2. 创建初始里程碑（项目初始化、各域功能实现等）
3. 自动推进第一个就绪里程碑
4. 持续输出日志到 stdout 和 ccd-output.log

**运行时长:** 无法预测——取决于 codecoder 的实现能力和项目复杂度。可能是数十分钟到数小时。

- [ ] **Step 2: 监控退出码**

```bash
# 上一步完成后查看退出码
echo "Exit code: $?"
# 0=全部完成  2=卡住  3=熔断  4=错误
```

---

### Task 2.3: 监控与记录

- [ ] **Step 1: 实时观察日志**

观察重点：
- 里程碑创建和推进情况
- 技术栈选择（codecoder 选了什么框架？）
- 项目初始化命令（npm create vite? npx create-react-app?）
- 功能实现进度（完成了哪些域？）
- 错误和重试情况（retry 原因）
- 是否出现「过度探索」（整轮只读不写）

- [ ] **Step 2: 记录关键事件**

记录格式：
```
[时间戳] 里程碑: <名称> → <状态>
[时间戳] 技术栈: <codecoder 选择的框架>
[时间戳] 错误: <错误描述>
[时间戳] 重试 #N: <原因>
```

---

## Phase 3: 结果评估

### Task 3.1: 检查完成状态

- [ ] **Step 1: 检查退出码和最后状态**

```bash
cat ~/Code/strategic-control/ccd-output.log | tail -50
```

- [ ] **Step 2: 检查项目文件**

```bash
cd ~/Code/strategic-control
ls -la
# 检查项目结构
find . -not -path './node_modules/*' -not -path './.git/*' -not -path './target/*' -type f | head -50
```

- [ ] **Step 3: 验证项目可构建**

```bash
cd ~/Code/strategic-control
# 如果 codecoder 用了 Node.js:
npm run build 2>&1 || echo "Build failed"
# 如果 codecoder 用了其他框架:
# 自行判断构建命令
```

### Task 3.2: 撰写实验报告

- [ ] **Step 1: 汇总结果**

包含：
- 退出码
- 完成里程碑数
- 未完成的功能
- 技术栈选择
- 代码行数统计
- 构建/测试结果

- [ ] **Step 2: 问题清单**

列出 codecoder 在本次实验中暴露的所有问题：
- 卡住的位置和原因
- 选择不合理的技术决策
- 代码质量问题
- 功能遗漏
- 稳定性问题

- [ ] **Step 3: 提交到 docs/proof/ 作为反馈**

报告写入 `docs/proof/experiment-report.md`

---

## 全局约束

1. Claude 不参与任何代码开发——仅负责环境准备和结果记录
2. codecoder 拥有完全自主的技术栈决策权
3. 如果 codecoder 无法完成（stuck/crash/loop），终止实验并记录问题
4. 实验报告须包含具体、可复现的问题描述，供 codecoder 项目修复参考
5. 所有日志保留在 `ccd-output.log` 中
