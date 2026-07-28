# FRONTEND-DESIGN.md 模板 / Frontend Design Document Template

> 由 engineer-frontend-architect 技能在 Phase 3 生成。
> 用作 engineer-orchestrator（Phase 4）的前端里程碑编排输入。

---

# [项目名称] — 前端设计文档 / Frontend Design Document

## 1. 前端范围 / Portal Scope

| 端 | 技术栈 | 设备 | 用户角色 | 设计系统 | 约束 |
|:--|:------|:-----|:--------|:--------|:-----|
| [端A] | [框架+UI库] | [PC] | [角色] | [共享/特有] | [约束] |
| [端B] | [框架+UI库] | [Mobile] | [角色] | [共享/特有] | [约束] |

## 2. 设计系统 Token / Design Token System

### 共享 Token

| Token | 值 | 用途 |
|:-----|:--|:------|
| color-primary | #HEX | 主色按钮、品牌标识 |
| color-accent | #HEX | 操作强调 |
| font-heading | [字体栈] | 标题 |
| font-body | [字体栈] | 正文 |
| spacing-base | 4px | 间距基准 |
| radius-md | 8px | 卡片/按钮圆角 |

### [端A] 特有 Token

| Token | 值 | 理由 |
|:-----|:--|:------|

## 3. 页面树 & 路由 / Page Tree & Routes

### [端A]

```
/page         → 页面名（一句话说明）
├── /page/sub → 子页面
└── /page/:id → 详情页
```

### [端B]

```
/page → 页面名
```

## 4. 组件树 / Component Hierarchy

### 通用 UI 组件

| 组件 | 用途 | 状态覆盖 |
|:----|:-----|:---------|
| [Component] | [用途] | [loading/empty/error] |

### 业务组件

| 组件 | 所属页面 | 依赖组件 |
|:----|:--------|:---------|

## 5. 状态管理架构 / State Architecture

| 类型 | 管理方式 | 示例 |
|:----|:--------|:-----|
| 服务端数据 | [SWR/React Query] | [数据示例] |
| 全局 UI | [Zustand/Context] | [状态示例] |
| 局部 UI | useState | [状态示例] |

## 6. UI 状态机 / UI State Machines

### [页面名]

| 状态 | 触发条件 | UI 表现 |
|:----|:---------|:--------|
| loading | [条件] | [表现] |
| empty | [条件] | [表现] |
| error | [条件] | [表现] |
| normal | [条件] | [表现] |

### [页面名]

...

## 7. API 交互模式 / API Interaction

| 页面 | 获取策略 | 变更策略 | 理由 |
|:----|:--------|:--------|:------|
| [页面] | [SSR/SWR/Static] | [optimistic/pessimistic] | [理由] |

## 8. 页面级设计总结 / Page-Level Summary

### [页面名]

**设计要点**: [一句话]
**数据需求**: [API 数据]
**状态覆盖**: [特别关注的状态]
