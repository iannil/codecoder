# Frontend Spec Guide — 前端代码自动生成规范

> 定义如何将"前端设计方向"转化为可自动执行的**前端组件编码规范**，
> 使得 engineer-workflow 在 `--auto` 模式下能自动生成完整的前端 UI 代码。
>
> 参见: `skill-protocol.md`（技能间调用协议）、`references/frontend-guide.md`（项目级前端配置）
>
> 规范版本: 1.0

---

## 核心思路

engineer-architect 已经能记录"前端设计方向"（设计基调、色彩、排版、布局原则）。
但光有方向不够——AI 需要**可执行的编码规范**来生成一致的前端代码。

**解决方案**: 
- 在 CONTEXT.md 的"前端设计方向"基础上，添加**组件编码规范**章节
- 定义组件结构、样式方案、状态管理、API 调用模式
- orchestrator 在碰到前端里程碑时，加载这份规范来指导 workflow 编码

---

## CONTEXT.md 前端设计方向增强

当 `project-metadata.json` 中 `has_frontend: true` 时，architect 必须在 CONTEXT.md 中编写以下内容。

### 必须包含的章节

```markdown
## 🎨 前端设计系统 / Frontend Design System

### 组件架构 / Component Architecture

**框架**: [React / Vue / Svelte / Next.js / ...]

**目录结构**:
```
src/
├── components/
│   ├── ui/           # 通用 UI 组件（按钮、输入框、卡片...）
│   ├── layout/       # 布局组件（Header, Sidebar, Main...）
│   └── features/     # 业务组件（OrderList, UserProfile...）
├── hooks/            # 自定义 Hooks
├── lib/              # 工具函数
├── pages/            # 页面组件
├── api/              # API 调用层
├── styles/           # 全局样式 / 主题变量
└── types/            # TypeScript 类型定义
```

**组件编码规则**:
1. 每个组件一个文件，文件名与组件名一致（PascalCase）
2. 组件 Props 使用 TypeScript 接口定义，放在文件顶部
3. 业务组件通过 props 接收数据，不直接在组件内调用 API
4. 通用 UI 组件放在 `components/ui/`，业务组件放在 `components/features/`

### 样式方案 / Styling Approach

**方案**: [Tailwind CSS / CSS Modules / Styled Components / ...]

**主题变量**:
```css
/* 色彩系统 */
--color-primary: #1a1a2e;
--color-secondary: #16213e;
--color-accent: #0f3460;
--color-background: #f8f9fa;
--color-text: #333333;

/* 排版系统 */
--font-heading: 'Inter', sans-serif;
--font-body: 'Inter', sans-serif;
--font-mono: 'JetBrains Mono', monospace;

/* 间距系统 (4px base) */
--space-xs: 4px;
--space-sm: 8px;
--space-md: 16px;
--space-lg: 24px;
--space-xl: 32px;
```

**设计原则**:
1. [数据优先] — 让数据和状态一目了然，装饰不喧宾夺主
2. [操作意图明确] — 每个按钮和链接明确告知后果
3. [保持克制] — 不为装饰而装饰

### API 调用模式 / API Call Pattern

**方案**: [React Query / SWR / fetch / axios]

**目录约定**:
```
src/api/
├── client.ts         # HTTP 客户端配置（baseURL, headers, interceptors）
├── queries/          # 查询 Hooks
│   ├── usePosts.ts
│   └── useUsers.ts
└── mutations/        # 写操作 Hooks
    ├── useCreatePost.ts
    └── useUpdatePost.ts
```

**错误处理标准**:
- API 调用错误统一在 `client.ts` 的 interceptor 中处理
- 业务错误显示 toast 通知
- 401 自动跳转登录页

### 页面路由 / Page Routes

| 路径 | 页面组件 | 说明 | MVP |
|:----:|---------|------|:----:|
| `/` | HomePage | 首页 | ✅ |
| `/login` | LoginPage | 登录 | ✅ |
| `/posts` | PostListPage | 文章列表 | ✅ |
| `/posts/:id` | PostDetailPage | 文章详情 | ✅ |
| `/admin` | AdminPage | 管理后台 | ❌ 后续 |
```

---

## Frontend Spec JSON 格式

当 architect 在 `--auto` 模式下运行时，除了写入 CONTEXT.md 中的设计系统章节外，
还应该生成一个机器可读的 `frontend-spec.json` 供 workflow 使用。

```json
{
  "framework": "react",
  "styling": "tailwind-css",
  "has_typescript": true,
  "component_tree": [
    {
      "path": "components/ui/Button",
      "description": "通用按钮组件，支持 variant (primary/secondary/ghost) 和 size (sm/md/lg)"
    },
    {
      "path": "components/ui/Input",
      "description": "输入框组件，支持 label、error、placeholder"
    },
    {
      "path": "components/layout/Header",
      "description": "顶部导航栏，包含 Logo + 导航链接 + 用户菜单"
    },
    {
      "path": "components/layout/MainLayout",
      "description": "主布局容器，Header + Sidebar + Content 区域"
    },
    {
      "path": "pages/HomePage",
      "description": "首页"
    },
    {
      "path": "pages/LoginPage",
      "description": "登录页，包含邮箱 + 密码表单"
    }
  ],
  "pages": [
    {
      "route": "/",
      "component": "pages/HomePage",
      "description": "首页",
      "is_mvp": true
    },
    {
      "route": "/login",
      "component": "pages/LoginPage",
      "description": "登录页",
      "is_mvp": true
    }
  ],
  "design_tokens": {
    "colors": {
      "primary": "#1a1a2e",
      "secondary": "#16213e",
      "accent": "#0f3460",
      "background": "#f8f9fa",
      "text": "#333333"
    },
    "typography": {
      "heading": "Inter",
      "body": "Inter",
      "mono": "JetBrains Mono"
    },
    "spacing": {
      "xs": "4px",
      "sm": "8px",
      "md": "16px",
      "lg": "24px",
      "xl": "32px"
    }
  },
  "api_pattern": {
    "client": "react-query",
    "base_url": "/api/v1",
    "auth_method": "jwt-token-header"
  },
  "state_management": "react-query + context",
  "routing": "react-router-dom v6"
}
```

---

## 前端里程碑编码指南

当 orchestrator 遇到 type 为 `frontend` 的里程碑时，按照以下策略执行：

### 里程碑执行顺序

```
批次 N (后端完成后):
  M{F}: 通用 UI 组件库（Button, Input, Card, Modal...）
  M{F+1}: 布局组件（Header, Sidebar, MainLayout）
  M{F+2}: API 层 + 数据 Hooks（client.ts + useXxx queries）
  M{F+3}: 页面组件（逐页实现）
  M{F+4}: 路由配置 + 导航连接
```

### 每个前端里程碑的验收重点

1. **UI 组件** — 样式是否遵循设计系统的色彩/排版？Props 是否完整？
2. **布局组件** — 是否响应式？与路由集成是否正确？
3. **API 层** — 错误处理是否统一？loading/error/empty 状态是否覆盖？
4. **页面** — 数据流是否正确？加载态/空态/错误态是否覆盖？
5. **路由** — 所有页面可访问？404 页面存在？

### 前端代码质量门禁

| 检查项 | 标准 |
|--------|------|
| 组件 Props 类型 | 必须有 TypeScript 接口定义 |
| 样式一致 | 使用设计令牌（design tokens）中的色值，不硬编码 |
| 响应式 | 至少适配 desktop + mobile |
| 加载态 | 数据加载时有 spinner/skeleton |
| 空态 | 数据为空时有友好提示 |
| 错误态 | API 错误时有 toast/alert |
| 可访问性 | 表单有 label，按钮有 aria-label（如适用） |
