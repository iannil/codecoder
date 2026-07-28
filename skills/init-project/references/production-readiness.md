# 生产就绪基准 / Production Readiness Baseline

> 按项目类型定义生产环境的安全、监控、错误处理最低标准。
> 在 engineer-job 的集成阶段（Phase 3）作为可选检查步骤执行。

## 服务端应用 / Server Application

### 安全 / Security
- [ ] 敏感信息通过环境变量注入（无硬编码密钥、Token、连接字符串）
- [ ] 健康检查端点：`/health`（存活检查）和 `/ready`（就绪检查）
- [ ] CORS 配置为白名单模式（无 `Access-Control-Allow-Origin: *`）
- [ ] 输入验证层（请求体 schema 校验，拒绝格式错误的数据）
- [ ] Rate Limiting（防止单 IP 高频请求压垮服务）

### 可观测性 / Observability
- [ ] 结构化日志（JSON 格式，每行包含 `timestamp`, `level`, `message`, `trace_id`）
- [ ] 统一错误响应格式（`{ error: { code, message, details? } }`）
- [ ] 全局异常捕获（未 catch 的异常返回 500 + 日志记录，不暴露堆栈给客户端）

### 依赖安全 / Dependency Security
- [ ] 运行依赖漏洞扫描：
  - Node.js: `npm audit`
  - Python: `pip-audit` 或 `safety check`
  - Rust: `cargo audit`
  - Go: `govulncheck`

## Web 应用 / Web Application

在服务端应用全部检查项之上，额外检查：
- [ ] CSP（Content-Security-Policy）头已配置
- [ ] X-Frame-Options: `DENY` 或 `SAMEORIGIN`
- [ ] X-Content-Type-Options: `nosniff`
- [ ] HSTS 头（`Strict-Transport-Security`）已配置
- [ ] 输出编码（HTML 实体编码防止 XSS）
- [ ] 构建产物体积优化（Tree Shaking / 代码分割 / 懒加载）

## CLI 工具 / CLI Tool

- [ ] 友好的错误输出（使用 stderr 输出错误信息，不是 panic/crash）
- [ ] 退出码语义：0=成功, 1=运行时错误, 2=参数错误
- [ ] `--help` 输出完整且格式整齐
- [ ] 所有错误都有用户可读的消息（无笼统的 "Error: something went wrong"）

## 库 / Library

- [ ] 公共 API 有完整的 JSDoc / doc comment
- [ ] 所有 panic/unwrap 已消除（Rust: 返回 Result，Go: 返回 error）
- [ ] 无硬编码的全局状态或单例（影响库的多次实例化）

## 执行方式

在 engineer-job 的 Phase 3（集成阶段）作为可选子步骤：
1. 读取本文件，按项目类型找到对应的检查清单
2. 逐项检查。通过则标记 ✅，不通过则记录到"建议改进"清单
3. 对安全类失败（硬编码密钥、无输入验证、CORS 全开）标记为 **必须修复**
4. 将检查结果合并到最终报告中
