---
name: init-project
description: >
  Complete project initialization workflow that scaffolds a new project's file tree
  following strict conventions. Use when the user wants to create a new project,
  initialize a repository, scaffold a project structure, set up development conventions,
  or start a new codebase from scratch. ALWAYS use this skill whenever the user says
  "init project", "new project", "scaffold", "project init", "create project",
  "initialize repo", "项目初始化", "新项目", "创建项目", "脚手架", or any similar
  phrase about starting a new project — even if the request sounds informal or incomplete.
  ROUTING RULE: 若用户要"从零做一个完整项目做完"，请改触发 engineer-job；本技能仅做脚手架，不负责架构与开发。
  The skill handles both the universal project conventions (docs, memory, release structure)
  and project-type-specific scaffolding (Python, Rust, Node.js, Go, web apps, CLI tools, etc.).
  It asks 5-8 clarifying questions before generating the full file tree.
source: "https://zhurongshuo.com"
compatibility: "bash, write, edit"
---

# init-project — 项目初始化工作流

> **来源声明**: 本 skill 来源于 [zhurongshuo.com](https://zhurongshuo.com)。如需引用或了解更多，请访问原著。
>
> **Source**: This skill originates from [zhurongshuo.com](https://zhurongshuo.com). Visit the source for more information.

## ⚙️ 模式选择 / Mode Selection

通过 `--mode` 参数控制自动确认程度（默认 normal）：

| 模式 | 行为 |
|:----:|------|
| normal | 问 5-8 个问题 → 展示文件树 → 等待确认后生成 |
| auto | 从需求推断默认值，只问推断置信度低于 60% 的问题（最多 2 个） |
| silent | 全部使用默认值，不提问，直接生成 |

### auto 模式默认决策

| 决策点 | 默认行为 |
|--------|---------|
| 技术栈选择 | 从需求推断默认技术栈（预定义模板匹配） |
| 项目元信息 | 从目录名/已有配置推断 |
| 许可证 | 默认 MIT（或项目已有许可证） |
| CI/CD | 默认 GitHub Actions（如适用） |
| Docker | 根据项目类型默认决定（服务端=需要，库=不需要） |
| 文件树确认 | 直接生成 |

### silent 模式附加行为

- 无进度条输出，仅记录日志
- 连低置信度推断也不提问——使用最保守的默认值
- 直接在 `pwd` 或指定目录生成项目结构

## 核心工作流

当你检测到用户要初始化一个新项目时，EXACTLY 执行以下步骤：

### Step 1: 推断 + 提问（交互阶段）

基于用户给出的描述（可能是简短的"帮我把 X 项目初始化了"或详细的需求），推断项目类型，然后**根据 --mode 模式**确定问题数量：

- `normal`：一次性问 5-8 个问题覆盖所有未知信息
- `auto`：先推断默认值，只问推断置信度 < 60% 的问题（最多问 2 个）
- `silent`：全部使用默认值，跳过提问阶段

需要覆盖的问题领域（并非每个问题都必问，根据已掌握的信息裁剪）：

| # | 问题领域 | 常见问题 | 何时跳过 |
|---|---------|---------|---------|
| 1 | **项目元信息** | 项目名？中文名/英文名？一句话描述？ | 用户已提供则跳过 |
| 2 | **技术栈确认** | 主要语言？框架？包管理器？ | 用户已说明则直接确认 |
| 3 | **构建与运行** | 构建工具？运行方式？入口文件？ | 对解释型语言可跳过 |
| 4 | **测试策略** | 使用什么测试框架？是否需要集成测试？ | — |
| 5 | **部署目标** | 部署到哪里？（服务器/SaaS/桌面/CLI/库/移动端） | 用户已说明则跳过 |
| 6 | **容器化** | 需要 Docker 吗？需要 docker-compose 吗？ | 纯库项目可跳过 |
| 7 | **CI/CD** | 需要配置 CI/CD 吗？哪个平台？（GitHub Actions / GitLab CI / 其他） | — |
| 8 | **许可证** | 选择什么开源许可证？ | 非开源项目可跳过 |

> **原则**：先思考用户已提供了多少信息。如果用户已经写了项目名和技术栈，就不要问已经知道的问题。确认语气胜过从零询问。对于完全模糊的描述（如"帮我搭个新项目"），则所有问题都需要问。

### Step 2: 生成通用文件树（始终生成）

无论项目类型，必须生成以下结构：

```
{project-name}/
├── .agents/                    # Agents 配置
│   └── settings.json
├── docs/                       # 项目文档
│   ├── standards/              # 规范文档
│   ├── templates/              # 文档模板
│   ├── adr/                    # 架构决策记录（Architecture Decision Records）
│   ├── progress/               # 未完成的修改进度
│   │   └── {YYYY-MM-DD}.md
│   └── reports/                # 已完成修改和验收报告
│       ├── completed/
│       └── index.md
├── memory/                     # 记忆系统（双层架构）
│   ├── daily/                  # 第一层：每日笔记
│   │   └── {YYYY-MM-DD}.md
│   └── MEMORY.md               # 第二层：长期记忆
├── release/                    # 发布产物
│   └── {language}/             # 按语言分类（如 rust, python）
├── .gitignore
├── README.md                   # 项目介绍（中英双语或英文）
├── IMPLEMENTATION_PLAN.md      # 实现计划
└── LICENSE                     # 许可证文件
```

**README.md 模板要点**：
- 项目名称和一句话描述（顶部首屏）
- 快速开始（安装 + 运行）
- 项目结构说明
- 贡献指南
- 许可证信息

请确保 `.agents/settings.json` 的内容包含基本的权限配置（如允许 Bash、Read、Write、Edit、Glob、Grep）。

### Step 3: 生成类型特定文件树

根据推断出的项目类型，在通用结构之上补充生成。常见模式如下 —— 但不仅限于此，根据实际情况灵活判断：

#### 模式 A：Python 项目
```
{project-name}/
├── src/{project-name}/
│   ├── __init__.py
│   ├── app.py                   # 应用入口
│   ├── config.py                # 声明式配置
│   ├── services/                # 服务层
│   │   └── __init__.py
│   └── utils/                   # 工具函数
│       ├── __init__.py
│       ├── logger.py            # 结构化日志（JSON格式，含 trace_id/span_id）
│       └── lifecycle.py         # LifecycleTracker 装饰器/上下文管理器
├── tests/
│   ├── __init__.py
│   ├── conftest.py              # pytest fixtures
│   ├── test_app.py
│   └── test_services/
│       └── __init__.py
├── docker/
│   └── Dockerfile
├── docker-compose.yml           # 数据库、消息队列等依赖
├── pyproject.toml               # 项目元数据与依赖（推荐）
├── requirements.txt             # 或 requirements
├── .venv/                       # venv 虚拟环境（在 README 中说明）
└── Makefile                     # 常用命令快捷方式
```

#### 模式 B：Rust 项目
```
{project-name}/
├── src/
│   ├── main.rs                   # 入口
│   ├── config.rs                 # 配置
│   ├── services/                 # 服务层
│   │   └── mod.rs
│   ├── utils/
│   │   ├── mod.rs
│   │   ├── logger.rs             # 结构化日志
│   │   └── lifecycle.rs          # LifecycleTracker 结构
│   └── lib.rs                    # 如果同时是库
├── tests/
│   └── integration/
│       └── test_main.rs
├── release/                      # 发布路径
│   └── rust/
├── docker/
│   └── Dockerfile
├── docker-compose.yml
├── Cargo.toml
├── Cargo.lock
├── .rustfmt.toml
├── .clippy.toml                  # 或 clippy 配置
└── Makefile                      # 或 Justfile
```

#### 模式 C：Node.js / TypeScript 项目
```
{project-name}/
├── src/
│   ├── index.ts                  # 入口
│   ├── config.ts                 # 声明式配置
│   ├── services/                 # 服务层
│   │   └── index.ts
│   ├── utils/
│   │   ├── logger.ts             # 结构化日志
│   │   └── lifecycle.ts          # LifecycleTracker
│   └── types/                    # 类型定义
│       └── index.ts
├── tests/
│   ├── setup.ts
│   ├── test_index.ts
│   └── services/
├── docker/
│   └── Dockerfile
├── docker-compose.yml
├── package.json
├── tsconfig.json
├── .eslintrc.cjs
├── .prettierrc
├── vitest.config.ts              # 或 jest.config.ts
└── Makefile
```

#### 模式 D：前端 Web 项目（React/Vue/Next.js 等）
在通用结构 + Node.js 结构之上，额外生成：
```
{project-name}/
├── src/
│   ├── app/                      # Next.js App Router / 页面
│   ├── components/               # 组件
│   │   ├── ui/                   # 通用 UI 组件
│   │   └── features/             # 业务组件
│   ├── hooks/                    # 自定义 Hooks
│   ├── lib/                      # 工具库
│   ├── styles/                   # 样式
│   └── types/                    # TypeScript 类型
├── public/                       # 静态资源
├── tailwind.config.ts            # 或其他 CSS 框架配置
└── next.config.ts                # 或其他框架配置
```

#### 模式 E：CLI 工具
在对应语言结构基础上，额外包含：
```
{project-name}/
├── src/
│   └── cli/
│       ├── mod.rs / __init__.py  # CLI 入口
│       ├── commands/             # 子命令
│       └── args.rs / args.py     # 参数解析
├── tests/
│   └── test_cli.rs / test_cli.py
└── Makefile                      # 含 install / build / publish 目标
```

#### Go 项目

参考 `references/templates/go-project.md` 生成。

#### Java/Kotlin 项目

参考 `references/templates/java-project.md` 生成。

#### 其他项目类型

对于未在上述列表中列出的项目类型（C++、Elixir、Swift 等），遵循以下原则自行推断生成：
- 该语言/框架社区的标准目录结构
- 对应的包管理器配置文件
- 测试目录结构
- 构建/运行配置
- 如果 Docker 化有意义，加入 Dockerfile

### Step 4: 注入项目指南约定

在生成的每个关键文件中，**注入以下约定精神**（具体内容参考 `references/conventions-guide.md`）：

#### 4.1 代码与文档语言
- 代码中的注释、文档使用**中文**
- 代码本身（变量名、函数名、类型名）使用**英文**
- README 等面向外部文档使用英文或中英双语

#### 4.2 可观测性（ODD）
为实现全链路可观测性，在项目中安装以下基础设施：

**结构化日志格式（JSON）：**
```json
{
  "timestamp": "2026-07-11T21:30:00.000Z",
  "trace_id": "uuid-or-snowflake",
  "span_id": "step-identifier",
  "event_type": "Function_Start | Function_End | Branch | Error",
  "payload": { "args": {}, "result": {} }
}
```

**LifecycleTracker 装饰器/上下文管理器**（在对应语言中实现）：
- 函数进入时记录输入参数
- 函数退出时记录返回值和耗时
- 函数异常时记录完整堆栈

**关键节点埋点**：
- 在 if/else 分支、for/while 循环、外部 API 调用前后添加 Point 埋点

**注意事项**：埋点代码必须与业务逻辑解耦（使用装饰器/AOP），不能让日志代码淹没业务逻辑。

#### 4.3 面向大模型的可改写性（LLM Friendly）
在生成代码时遵循：
- **一致的分层与目录**：相同功能在不同包中遵循相同结构
- **明确边界与单一职责**：函数/类保持单一职责，公共模块暴露极少稳定接口
- **显式类型与契约优先**：导出 API 均有显式类型，运行时与编译时契约一致
- **声明式配置**：重要行为转为数据驱动（配置对象 + `as const`/`satisfies`）
- **可搜索性**：统一命名模式（`parseXxx`、`assertNever`、`safeJsonParse`、`createXxxService`）
- **变更安全策略**：批量程序性改动前先将原文件备份至 `backup/` 相对路径

#### 4.4 环境约定
在生成 `docker-compose.yml` 和配置时遵循：
- 数据库、消息队列、缓存等尽量使用 Docker 部署
- Python 项目使用 venv 虚拟环境（在 README 和 Makefile 中体现）
- 为项目配置独立的 Docker 网络，避免与其他项目网络冲突

#### 4.5 记忆系统
始终生成 `memory/` 双层架构：
- `memory/daily/{YYYY-MM-DD}.md` — 仅追加的每日笔记，按时间顺序记录
- `memory/MEMORY.md` — 经过整理的长期记忆（用户偏好、关键决策、经验教训）

### Step 5: 填充关键文件的初始内容

对于以下文件，不仅要创建，还要写入有意义的初始内容：

#### `.gitignore`
根据项目语言/工具链生成对应的 gitignore 规则（可参考 github/gitignore 模板），必须包含：
- 依赖目录（node_modules/, __pycache__/, target/ 等）
- 环境文件（.env, .venv/ 等）
- 系统文件（.DS_Store, Thumbs.db 等）
- IDE 目录（.idea/, .vscode/ 等，但如果项目本身要求保留则例外）

#### `README.md`
包含：
- 首屏：项目名 + 一句话描述 + 徽章（CI、License 等占位）
- 项目简介（1-2 段）
- 快速开始（前置依赖 → 安装 → 运行 → 测试）
- 项目结构示意
- 贡献指南
- 许可证

#### `IMPLEMENTATION_PLAN.md`
生成一个初始的 `IMPLEMENTATION_PLAN.md`，包含：
- 项目愿景与目标
- 当前阶段（Phase 0：项目初始化）
- 待办里程碑列表（占位，待细化）
- 架构决策记录入口

#### `docs/reports/index.md`
生成一个初始首页，列出功能完成度状态。

#### 入口文件
在入口文件中给出一个可运行的 "Hello World" 级别最小示例，包含：
- 基本配置加载
- 日志初始化
- 生命周期追踪示例（LifecycleTracker）

### Step 6: 通知用户完成

生成结束后，**用以下格式向用户汇报**：

```
✅ 项目 {project-name} 初始化完成！

生成文件树：
{project-name}/
├── src/...
├── docs/...
├── memory/...
└── ...

已注入的约定：
- ✅ 文档体系（docs/ + memory/）
- ✅ 发布结构（release/{lang}/）
- ✅ 可观测性（LifecycleTracker + 结构化日志）
- ✅ 环境配置（Docker / venv / 独立网络）
- ✅ LLM 友好（统一命名 + 声明式配置 + 单一职责）
- ✅ CI/CD 配置（{platform}）
- ✅ 测试框架（{framework}）
- ✅ {license} 许可证
- ✅ {docker|docker-compose|none}

你可以执行以下命令开始开发：
  cd {project-name}
  {run-command}
```

## 重要原则

### 1. 问得聪明，不要多问已经知道的事
用户如果已经说"创建一个 Python CLI 工具叫 crawler"，你就只需要确认测试框架、CI/CD 等补充信息，不要反问"项目是什么类型"。

### 2. 生成的是初始化，不是完成品
每个文件的初始内容应该是**可运行的最小示例**，而不是完整的业务代码。重点是让用户能立即 `cd` 进去并运行，而不是生成一个空壳。

### 3. 约定优先，但保持灵活性
如果用户明确说不需要某些结构（"不要 Docker"、"不要 memory 系统"），则尊重用户意愿跳过。约定是默认值，不是强迫。

### 4. 语言混合约定
代码用英文写（变量、函数、类名、注释内技术术语），面向开发者的文档/README 用英文或中英双语，面向项目内部团队的注释和文档可以用中文。遵循这一约定。

### 5. 渐进式深度
如果用户只是简单说"帮我初始化一个项目"，不需要一次生成所有文件的完整内容。首屏 README + 入口文件可运行 + 目录结构完整 就足够。复杂的实现留在后续开发。

## 参考文件

- `references/conventions-guide.md` — 完整的项目指南全文（项目目标、语言约定、发布约定、环境约定、文档约定、LLM Friendly 模式、ODD 可观测性开发、记忆系统）
- `references/templates/` — 按项目类型的文件模板
