# 数据库迁移策略 / Database Migration Strategy

> 定义项目中数据库迁移的规范、工具和纪律。在 architect 设计蓝图时引用此文件生成
> CONTEXT.md 的"数据库迁移策略"章节，在 workflow 开发时作为迁移文件合规性检查依据。

## 三条铁律 / Three Iron Rules

### 铁律一：可回滚 / Every Up Has a Down

每条 `up` 迁移（应用变更）**必须**提供对应的 `down` 迁移（撤销变更）。

```
✅ 正确：
  20260713000001_create_users_table.up.sql   → CREATE TABLE users ...
  20260713000001_create_users_table.down.sql → DROP TABLE users ...

❌ 错误：
  20260713000001_create_users_table.sql  ← 没有 down 文件，无法回滚
```

**例外**：仅当数据库不支持回滚（如某些 SQLite 模式）时，需在迁移文件中写明 `-- IRREVERSIBLE: <理由>`。

### 铁律二：不可改写 / Never Rewrite a Published Migration

已经合并到主分支的迁移文件**严禁修改**。

- 发现错误 → 创建新的迁移文件来修正，而不是编辑已发布的文件
- 修正迁移中写明：`-- FIX: migration 20260713000001 created wrong column type`
- 这条纪律确保了团队协作时迁移哈希一致性

### 铁律三：命名规范 / Naming Convention

```
格式: YYYYMMDDHHMMSS_description.{up|down}.{ext}
示例: 20260713000001_create_users_table.up.sql
      20260713000001_create_users_table.down.sql
```

- 时间戳确保全局有序
- description 使用 snake_case 英文
- ext 根据数据库选择（`.sql` 通用，Python 可能用 `.py`）

## 数据库类型适配表 / Per-Database Guide

| 数据库 | 推荐工具 | 迁移目录 | 回滚机制 |
|--------|---------|---------|---------|
| SQLite | 手动 SQL / `sqlx` migrate | `db/migrations/` | 手写 up/down SQL |
| PostgreSQL | `sqlx` / `goose` / `Alembic` | `db/migrations/` | 工具原生回滚命令 |
| MySQL | `golang-migrate` / Flyway | `db/migrations/` | 工具原生回滚命令 |
| MongoDB | `migrate-mongo` | `db/migrations/` | down 脚本 |
| SQLite (Rust) | `diesel` / `sqlx` | `migrations/` | 工具原生回滚 |
| PostgreSQL (Rust) | `diesel` / `sqlx` | `migrations/` | 工具原生回滚 |

## 迁移文件内容规范 / Migration Content

**up 迁移**:
```sql
-- 20260713000001_create_users_table.up.sql
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email VARCHAR(255) NOT NULL UNIQUE,
    name VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_users_email ON users(email);
```

**down 迁移**:
```sql
-- 20260713000001_create_users_table.down.sql
DROP TABLE IF EXISTS users;
```

## 验收检查清单 / Inspection Checklist

workflow 验收时对照检查：
- [ ] 本次所有迁移文件都有对应的 down 文件？
- [ ] 没有修改已发布的迁移文件？（检查 git diff 中是否包含已有的迁移文件）
- [ ] 迁移文件命名符合时间戳前缀规范？
- [ ] 如果是修正迁移，说明了修正原因？
