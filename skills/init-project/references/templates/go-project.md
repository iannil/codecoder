# Go 项目模板 / Go Project Template

> Go 项目的标准目录结构和框架推荐。在 init-project 的 Step 3 中引用。

## 目录结构 / Directory Structure

```
{project-name}/
├── cmd/
│   └── {project-name}/
│       └── main.go              # 入口：配置加载 + 日志初始化 + 启动服务
├── internal/                     # 私有包（Go 编译器强制禁止外部导入）
│   ├── config/
│   │   └── config.go            # 声明式配置（环境变量绑定）
│   ├── handler/                 # HTTP 处理器层
│   │   └── handler.go
│   ├── middleware/              # 中间件
│   │   ├── logger.go
│   │   ├── recovery.go
│   │   └── cors.go
│   ├── model/                   # 数据模型
│   │   └── model.go
│   ├── repository/              # 数据访问层
│   │   └── repository.go
│   ├── service/                 # 业务逻辑层
│   │   └── service.go
│   └── utils/
│       ├── logger.go            # 结构化日志（JSON, trace_id/span_id）
│       └── lifecycle.go         # LifecycleTracker
├── migrations/                  # 数据库迁移
│   ├── 20260713000001_init.up.sql
│   └── 20260713000001_init.down.sql
├── tests/
│   └── integration/
│       └── api_test.go
├── docker/
│   └── Dockerfile               # 多阶段构建
├── docker-compose.yml           # 数据库等依赖服务
├── go.mod
├── go.sum
├── Makefile                     # 常用命令（build/test/lint/migrate）
├── .golangci.yml                # Linter 配置
├── .gitignore
├── README.md
├── LICENSE
├── docs/
│   ├── adr/
│   └── standards/
├── memory/
│   ├── daily/
│   └── MEMORY.md
└── release/
    └── go/
```

## 框架推荐 / Framework Recommendations

| 场景 | 推荐 | 替代方案 |
|------|:----:|:--------:|
| REST API | `gin` | `echo` / `fiber` / `chi` |
| 数据库访问 | `sqlx` | `gorm` / `ent` |
| 数据库迁移 | `golang-migrate` | `pressly/goose` / `sqlx migrate` |
| 测试 | `testing` + `testify` | `gotest` |
| 日志 | `log/slog`（标准库） | `zap` / `logrus` |
| 配置 | `envconfig` / `viper` | 标准库 `os.Getenv` |
| 构建 | `Makefile` + `go build` | `goreleaser` |
| Linter | `golangci-lint` | `staticcheck` |

## 入口文件示例 / Entry Point Example

```go
// cmd/{project}/main.go
package main

import (
    "{module}/internal/config"
    "{module}/internal/handler"
    "{module}/internal/middleware"
    "{module}/internal/repository"
    "{module}/internal/service"
    "{module}/internal/utils"
)

func main() {
    defer utils.LifecycleTracker("main.start")()

    cfg := config.Load()

    db, err := repository.Connect(cfg.DatabaseURL)
    if err != nil {
        utils.Logger.Fatal("failed to connect database", "error", err)
    }
    defer db.Close()

    svc := service.New(db)
    h := handler.New(svc)
    r := middleware.Setup(h)

    utils.Logger.Info("server starting", "addr", cfg.ServerAddr)
    r.Run(cfg.ServerAddr)
}
```

## 测试规范 / Testing Standards

| 测试类型 | 位置 | 最低标准 |
|---------|------|---------|
| 单元测试 | `*_test.go` 与源码同目录 | 每核心函数 1 happy + 2 edge |
| 集成测试 | `tests/integration/` | 每端点 1 成功 + 2 异常 |
| HTTP 测试 | 使用 `httptest` | 请求/响应断言 |

**依赖**：
- 断言使用 `testify/assert` 或 `testify/require`
- HTTP 测试使用标准库 `net/http/httptest`
- 集成测试使用真实数据库（通过 `docker-compose.yml` 启动）

## 构建与运行 / Build & Run

```bash
make build          # go build -o bin/ ./cmd/{project}
make test           # go test ./...
make lint           # golangci-lint run
make migrate-up     # 运行数据库迁移
make migrate-down   # 回滚所有迁移
make docker         # docker build
```

## Makefile 示例 / Makefile Example

```makefile
.PHONY: build test lint docker migrate-up migrate-down

APP_NAME := {project-name}

build:
	go build -o bin/$(APP_NAME) ./cmd/$(APP_NAME)

test:
	go test ./... -v -count=1

lint:
	golangci-lint run

migrate-up:
	migrate -path migrations -database "$(DATABASE_URL)" up

migrate-down:
	migrate -path migrations -database "$(DATABASE_URL)" down

docker:
	docker build -t $(APP_NAME) -f docker/Dockerfile .
```
