# E2E 编排手册 / E2E Playbook (agent-browser)

> engineer-qa ④ E2E 层读取本文件。触发时机：功能/项目开发完成后负载一次。

## 标准流程
1. 探测启动命令（CONTEXT.md 运行/部署章节 → package.json scripts / Makefile / README）。
2. 后台起服务，等待就绪（探测端口/健康检查）。
3. `Skill: agent-browser` 脚本化驱动关键链路：导航 → 填表 → 点击 → 断言 DOM/文本/状态。
4. 断言来源：CONTEXT.md 用户链路/验收标准 → 缺失则从实体 CRUD 推断最小链路。
5. 截图取证 → `.agents/qa-e2e/<链路名>-<步骤>.png`。
6. flake：单链路失败自动重试 1 次，再失败判 FAIL。
7. 收尾：关闭服务，汇总通过/失败链路。

## 无 UI 项目降级 / Degradation
| 项目类型 | E2E 降级方式 |
|---------|-------------|
| API 服务 | 起服务 + HTTP 断言 CRUD 全链路（创建→读取→更新→删除）+ 错误路径 |
| CLI 工具 | 子命令 E2E：安装→运行→输出/退出码断言 |
| Library | 跳过 E2E，报告标注"库项目无 E2E，以单元+集成为终验" |

> 报告必须显式标注 E2E 是否降级及原因。E2E 环境不可用（起服务失败）→ 跳过并标注，②③层仍须过。
