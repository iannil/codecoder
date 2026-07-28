# engineer-poc — 产物模板 / Output Templates

两份产物：`POC-MANIFEST.md`（给 engineer-job Phase 4 消费）与 `POC-FIDELITY.md`（诚实保真度报告）。

---

## POC-MANIFEST.md 模板

```markdown
# POC Manifest — <项目名>

> 演进式 POC 清单。engineer-job Phase 4 据此把 mock 层演进为真实实现，而非重建页面。

## 技术栈 / Tech Stack
- 框架: <framework> ｜ 构建: <bundler> ｜ 状态: <state lib>

## 页面 / Pages
| 路由 | 页面 | 覆盖状态 | UI 状态 | 保真度 |
|:--|:--|:--|:--|:--|
| /students | 学员列表 | implemented | loading/empty/error/normal/edge | 真实交互 |

## 组件 / Components
| 组件 | 所属页面 | 保真度 |
|:--|:--|:--|

## Mock → 真实端点演进映射 / Evolution Map
| adapter 方法 | mock 行为 | 未来真实端点 | 契约来源 |
|:--|:--|:--|:--|
| listStudents | 内存分页 | GET /api/students | CONTEXT.md §API |

## 占位清单 / Placeholders（正式实现阶段接入）
| 资产 | 原因 | 处理 |
|:--|:--|:--|
| 支付页 | 第三方网关 | 占位未实现 |
```

---

## POC-FIDELITY.md 模板

```markdown
# POC Fidelity Report — <项目名>

逐资产诚实标注。三层级：`真实交互` / `mock 数据` / `占位未实现`。

## 汇总 / Summary
- 真实交互: N ｜ mock 数据: M ｜ 占位未实现: K
- 输入降级: <是/否，若是说明缺哪些文档>

## 逐资产 / Per-Asset
| 资产 | 类型 | 保真度 | 说明 |
|:--|:--|:--|:--|
| 学员列表 | page | 真实交互 | 全状态可点，数据来自 mock adapter |
| 排序算法 | logic | mock 数据 | 前端按 mock 结果展示，真实排序在服务端 |
| 支付流程 | flow | 占位未实现 | 第三方网关，留待正式实现 |
```
