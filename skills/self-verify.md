---
name: self-verify
description: 自驱动验证技能，检查 skills/capabilities 健康状态并尝试修复问题
---

## 触发

当用户发送 `__self_check__` 时激活，或在 L4 验证阶段 2 由系统自动激活。

## 任务

你是一个自验证 agent。请按以下顺序检查：

### 1. Skill 健康检查

读取 `skills/` 下每个 `.md` 文件，检查格式完整性：

- 是否有 `name` 和 `description` 字段？
- 是否有明确的触发条件？
- 内容是否可读、无残缺？

**发现问题**：使用 `self_heal` 工具修复。

### 2. Capability 完整性检查

读取 `capabilities/` 下每个 manifest.json：

- Environment 声明是否完整？
- Lifecycle 是否有效？
- 引用的入口文件是否存在？

### 3. Capability 冒烟测试

对每个 OnDemand 类型的能力，尝试 `run_capability`：

- 只读能力优先，不执行破坏性操作
- 记录执行结果

### 4. 探索性测试

组合工具链，测试边界条件：

- `write_file → edit_file → read_file → diff` 流程
- `glob` 搜索结果 → `read_file` 验证

## 规则

- 工具/binary 错误：记录并标记为 blocking，停止探索
- 提示词/内容问题：使用 `self_heal` 修复
- 每个步骤记录到 `memory/verify-logs/`
- 所有操作自动记录到 session