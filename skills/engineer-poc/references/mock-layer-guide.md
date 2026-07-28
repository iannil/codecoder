# engineer-poc — Mock 层接缝规范 / Mock Layer Seam Guide

目标：让 POC 的数据层成为**单一可替换接缝**，使 engineer-job Phase 4 只替换这一层即可接真实 API，页面与组件零改动。

## 单一接缝原则 / Single Seam

- 所有数据访问必须经过一个 adapter 模块（如 `src/mocks/adapter.*`），页面/组件**只**调用 adapter 暴露的方法，绝不直接内联假数据。
- adapter 方法签名对齐 `CONTEXT.md` 的 API 契约（路径、入参、返回形状、错误体）。契约缺失时按推断实现并在 `POC-FIDELITY.md` 标 `mock 数据`。

## 目录约定 / Layout

```
src/mocks/
├── adapter.(ts|js)     # 唯一对外接口：list/get/create/update/remove + auth
├── seed.(ts|js)        # 真实感 seed 数据
├── db.(ts|js)          # 内存存储（会话内持久化）
└── latency.(ts|js)     # 模拟网络延迟 / 随机错误（驱动 loading/error 状态）
```

## 契约对齐 / Contract Alignment

adapter 每个方法对应一个未来真实端点。示例：

```
// mock
adapter.listStudents({ page, keyword }) -> { items, total }   // 对应 GET /api/students
// 演进后（Phase 4）
fetch('/api/students?...') -> { items, total }                // 形状不变，接缝替换
```

## 驱动全状态 / Driving All UI States

- `latency` 制造可控延迟 → 页面能展示 **loading**。
- adapter 支持"强制空/错误"开关（如 query flag 或 seed 变体）→ 页面能展示 **empty / error**。
- 分页到末尾 → 页面能展示 **edge**。

## 演进映射 / Evolution Map

`POC-MANIFEST.md` 必须为每个 adapter 方法登记 `mock → 真实端点` 映射，供 Phase 4 逐个替换。

## do-not-fake

支付网关、第三方 SaaS、纯服务端算法 → adapter 返回占位结果，页面标 `占位未实现`，绝不接真实外部服务。
