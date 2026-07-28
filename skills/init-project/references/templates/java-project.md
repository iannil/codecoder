# Java 项目模板 / Java Project Template

> Java/Kotlin 项目的标准目录结构和框架推荐。在 init-project 的 Step 3 中引用。

## 目录结构 / Directory Structure

```
{project-name}/
├── src/
│   ├── main/
│   │   ├── java/
│   │   │   └── com/{project-name}/
│   │   │       ├── {ProjectApplication}.java       # 入口（Spring Boot）
│   │   │       ├── config/
│   │   │       │   ├── AppConfig.java               # 应用配置
│   │   │       │   └── LogConfig.java               # 日志配置
│   │   │       ├── controller/                      # API 控制器层
│   │   │       │   └── HealthController.java
│   │   │       ├── service/                         # 业务逻辑层
│   │   │       │   └── Service.java
│   │   │       ├── repository/                      # 数据访问层
│   │   │       │   └── Repository.java
│   │   │       ├── model/                           # 数据模型/实体
│   │   │       │   └── Entity.java
│   │   │       ├── dto/                             # 数据传输对象
│   │   │       │   ├── Request.java
│   │   │       │   └── Response.java
│   │   │       ├── exception/                       # 全局异常处理
│   │   │       │   ├── AppException.java
│   │   │       │   └── GlobalExceptionHandler.java
│   │   │       └── utils/
│   │   │           ├── Logger.java                  # 结构化日志工具
│   │   │           └── LifecycleTracker.java        # 生命周期追踪
│   │   └── resources/
│   │       ├── application.yml                      # 声明式配置
│   │       ├── application-dev.yml
│   │       └── db/migration/                        # Flyway 迁移
│   │           ├── V1__init.sql
│   │           └── V1__init.rollback.sql
│   └── test/
│       └── java/
│           └── com/{project-name}/
│               ├── controller/
│               │   └── HealthControllerTest.java
│               └── service/
│                   └── ServiceTest.java
├── docker/
│   └── Dockerfile               # 多阶段构建
├── docker-compose.yml           # 数据库等依赖服务
├── build.gradle.kts              # Gradle Kotlin DSL（推荐）
├── settings.gradle.kts
├── gradlew                       # Gradle Wrapper
├── gradlew.bat
├── gradle/
│   └── wrapper/
├── Makefile
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
    └── java/
```

## 框架推荐 / Framework Recommendations

| 场景 | 推荐（Spring Boot 3.x） | 轻量替代 |
|------|:-----------------------:|:--------:|
| 框架 | Spring Boot 3.x | Quarkus / Micronaut |
| 数据库 | Spring Data JPA + Flyway | MyBatis + Liquibase |
| API 文档 | SpringDoc OpenAPI | — |
| 测试 | JUnit 5 + Mockito + Testcontainers | — |
| 构建 | Gradle（Kotlin DSL） | Maven |
| 配置 | application.yml + @ConfigurationProperties | — |
| 日志 | Logback（JSON encoder） | Log4j2 |

## 入口文件示例 / Entry Point Example

```java
// src/main/java/com/{project}/{ProjectApplication}.java
package com.{project};

import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import com.{project}.utils.Logger;

@SpringBootApplication
public class {ProjectApplication} {

    public static void main(String[] args) {
        Logger.info("Application starting...");
        SpringApplication.run({ProjectApplication}.class, args);
        Logger.info("Application started");
    }
}
```

## 测试规范 / Testing Standards

| 测试类型 | 位置 | 最低标准 |
|---------|------|---------|
| 单元测试 | `src/test/java/`（镜像 main 结构） | 每核心方法 1 happy + 2 edge |
| 集成测试 | `@SpringBootTest` + Testcontainers | 覆盖核心 API 链路 |
| 控制器测试 | `@WebMvcTest` | 请求/响应/状态码断言 |

**依赖**：
- 单元测试使用 JUnit 5 + Mockito
- 集成测试使用 Testcontainers（数据库容器）
- 控制器测试使用 `@WebMvcTest` 切片测试

## 构建与运行 / Build & Run

```bash
./gradlew build           # 编译 + 测试 + 打包
./gradlew bootRun         # 开发运行
./gradlew test            # 运行所有测试
./gradlew clean           # 清理构建产物
make docker               # Docker 构建
```
