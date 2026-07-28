# 企业架构模式参考 / Enterprise Architecture Patterns

> 适用于多端、多模块、多租户的企业级系统。
> engineer-architect 在"架构模式决策"阶段参考此文档。

## 1. BFF (Backend For Frontend) 模式

### 定义
为每个前端端（Web / 小程序 / 移动端）提供一个专属的后端 API 层。

### 适用场景
- 不同前端端有不同的数据展示需求
- 不同前端端有不同的数据聚合粒度
- 需要为特定端做 API 适配（如小程序 API 的限制）

### 推荐实现
- 每个端一个独立的 BFF 服务（或目录）
- BFF 负责：数据聚合、格式转换、裁剪字段、端特有的认证逻辑
- 不建议 BFF 包含业务逻辑 — 业务逻辑在领域服务层

### 与 API Gateway 的关系
```
客户端 → API Gateway (认证/路由/限流) → BFF (聚合/转换) → 领域服务 (业务逻辑) → 数据库
```

## 2. 事件驱动架构 (Event-Driven Architecture)

### 定义
服务间通过异步事件进行通信，不直接调用。

### 适用场景
- 跨模块长流程（如：考试申请→审核→成绩同步→证书生成）
- 需要异步处理的操作（如：批量证书打印、快递对接）
- 跨模块最终一致性需求

### 事件定义规范
- **事件名**: 过去时态的动宾短语，如 `ExamApplicationSubmitted`
- **负载**: 包含事件 ID、时间戳、业务 ID、必要数据
- **幂等性**: 消费者必须支持重复事件处理（通过事件 ID 去重）

### 推荐实现
- 事件总线: RabbitMQ / Redis Streams（轻量级）/ Kafka（大规模）
- 事件存储: 所有事件持久化到事件表，用于审计和恢复

## 3. CQRS (Command Query Responsibility Segregation) 模式

### 定义
命令（写）和查询（读）使用不同的数据模型和通道。

### 适用场景
- 读多写少的场景（报表、看板、统计）
- 查询和写入的数据形状差异大
- 需要对相同数据有不同的访问模式

### 实现级别
- **轻量级**: 同一数据库，但不同模型（ORM 的读模型 vs 写模型分离）
- **完全 CQRS**: 不同数据库（读库同步写库的事件）

### 不适用场景
- 简单的 CRUD 系统（增加不必要的复杂度）

## 4. 多租户架构 (Multi-Tenancy)

### 定义
一个应用实例服务多个租户，每个租户数据隔离。

### 隔离策略

| 策略 | 隔离级别 | 成本 | 复杂度 | 适用 |
|:----|:--------:|:----:|:------:|:----|
| 数据库级 | ⭐⭐⭐ | 高 | 低 | 安全要求高 |
| Schema 级 | ⭐⭐ | 中 | 中 | PaaS 平台 |
| 行级 (Tenant ID) | ⭐ | 低 | 高 | 大多数 SaaS |

### 推荐
- 大多数企业系统使用**行级隔离**（tenant_id 字段）
- 租户上下文通过 JWT 或请求头传递
- 所有查询自动附加 `WHERE tenant_id = :current_tenant`（通过中间件实现）

## 5. Saga 模式 (分布式事务)

### 定义
通过一系列本地事务 + 补偿事务来实现跨服务的最终一致性。

### 两种风格

| 风格 | 描述 | 适合 |
|:----|:-----|:-----|
| 编排型 (Choreography) | 每个服务发布事件，下一个服务监听并响应 | 简单线性流程 |
| 协调型 (Orchestration) | 一个协调器（Orchestrator）管理每个步骤 | 复杂分支流程 |

### 推荐
- 线性流程用编排型（基于事件驱动）
- 复杂分支用协调型（单独的状态机服务）

## 6. DDD 分层架构 (Domain-Driven Design Layered Architecture)

### 标准分层

```
Interface (Controller/API)
    → Application (Service/UseCase)
        → Domain (Entity/ValueObject/Aggregate/DomainService)
            → Infrastructure (Repository/DB/External)
```

### 依赖规则
- Interface 依赖 Application
- Application 依赖 Domain
- Infrastructure 实现 Domain 定义的接口
- Domain 层零外部依赖

### 适用场景
- 业务逻辑复杂的系统（财务规则、考试流程、证书状态机）
- 长期维护的企业应用

---

## 模式选择指南

根据项目特征推荐模式：

| 项目特征 | 推荐模式 |
|---------|---------|
| 多个前端端（Web/小程序/移动） | BFF + API Gateway |
| 异步长流程（审批链、多步骤） | 事件驱动 + Saga |
| 大量报表/统计查询 | CQRS（轻量级） |
| 一个平台管理多个客户 | 多租户（行级） |
| 核心业务逻辑复杂 | DDD 分层 |
| 以上全部（完整企业系统） | BFF + 事件驱动 + CQRS + 多租户 + DDD |
