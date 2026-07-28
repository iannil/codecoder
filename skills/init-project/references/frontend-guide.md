# 前端实现指引 / Frontend Implementation Guide

> 在 architect 的前端设计方向和 workflow 的编码之间搭桥。
> 帮助 workflow 将蓝图中的设计基调、色彩、排版转化为实际的组件代码。

## 组件目录规范 / Component Directory Convention

```
src/
├── components/
│   ├── ui/           # 通用 UI 组件（按钮、输入框、卡片…）
│   │   ├── Button.tsx
│   │   ├── Card.tsx
│   │   └── Input.tsx
│   └── features/     # 业务组件（UserCard、OrderTable…）
│       ├── UserCard.tsx
│       └── OrderTable.tsx
├── layouts/          # 布局组件
│   ├── MainLayout.tsx
│   └── AuthLayout.tsx
├── pages/            # 页面组件
│   └── HomePage.tsx
├── hooks/            # 自定义 Hooks
│   └── useUser.ts
├── styles/           # 样式
│   └── globals.css
├── lib/              # 工具函数
│   └── api.ts
└── types/            # TypeScript 类型
    └── index.ts
```

## 框架生态推荐 / Framework Recommendations

| 场景 | 推荐 | 替代方案 |
|------|------|---------|
| React SPA | Next.js + shadcn/ui + Tailwind CSS | Vite + React Router |
| Vue SPA | Nuxt + shadcn-vue + Tailwind CSS | Vite + Vue Router |
| 静态站点 | Next.js (SSG) / Astro | Hugo / Jekyll |
| 移动端 | React Native / Expo | Flutter |
| 组件库 | shadcn/ui（默认推荐） | Radix UI + Tailwind |

**原则**：用成熟组件库处理 80% 的通用 UI（按钮、表格、弹窗），自定义 20% 的业务独特样式。

## 设计方向 → 代码映射 / Design-to-Code Mapping

当 architect 在 CONTEXT.md 中定义了前端设计方向后，workflow 编码时按以下规则映射：

| 蓝图定义 | 代码实现 |
|---------|---------|
| 主色 `#1a1a2e` | `tailwind.config.ts`: `theme.extend.colors.primary: '#1a1a2e'` → CSS 类 `bg-primary` / `text-primary` |
| 标题字体 Inter | `tailwind.config.ts`: `theme.extend.fontFamily.heading: ['Inter', 'sans-serif']` |
| "数据优先"原则 | 页面布局按"核心数据 → 操作 → 辅助信息"顺序排列 |

## 设计原则检查清单 / Design Principles Checklist

每个前端里程碑编码时对照检查：
- [ ] 色彩使用仅限蓝图色板？超出色板的新颜色是否必要？
- [ ] 字体使用是否 = 蓝图定义的字体系列？
- [ ] 排版层次（标题/正文/辅助文字）是否清晰？
- [ ] 交互反馈（悬停/点击/加载/错误状态）是否完整？
- [ ] 移动端响应式是否适配？

## 与 frontend-design 技能的联动

对于需要深度 UI 设计的项目，可以在 architect 阶段完成后调用 `frontend-design` 技能：
1. frontend-design 输出设计系统 Token（色板/排版/间距的精确值）
2. 将这些 Token 回写到 `CONTEXT.md` 的设计方向章节
3. workflow 编码时直接使用 Token 定义的 CSS 变量
