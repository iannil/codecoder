# 战略管控系统 — CodeCoder 自主构建实验设计

> 实验日期: 2026-07-25
> 实验目的: 验证 codecoder (Rust AI agent) 能否仅凭自身能力，从零自主构建一个完整的企业级前端应用

---

## 1. 背景

### 1.1 项目来源

`docs/proof/功能清单.md` 描述了一个完整的 **战略管控系统** (Strategic Management & Control System)，覆盖六大业务域：

| 编号 | 业务域 | 模块编号 | 二级分组 |
|------|--------|----------|----------|
| 一 | 战略驾驶舱 | — | 1 个 |
| 二 | 战略规划管理 | PLG-01 | 8 个 |
| 三 | 机构管理 | INST-02 | 4 个 |
| 四 | 组织绩效管理 | PERF-03 | 6 个 |
| 五 | 制度管理 | POL-04 | 1 个 |
| 六 | 智能与数据中枢 | INT-05 | 3 个 |
| 七 | 系统管理 | SYS-06 | 3 个 |

合计约 50+ 个页面/路由，大量共享组件（富文本编辑器、AI 助手、审批面板等）。

### 1.2 实验动机

验证 codecoder 的 **自主项目构建能力** 边界，发现其在真实项目开发中的短板和缺陷，为后续 codecoder 自身改进提供输入。

### 1.3 关键约束

- **自驱**: codecoder 完全自主推进，不允许 Claude 参与代码开发
- **技术栈自由**: codecoder 自行决定使用什么框架/语言/工具
- **全量实现**: 目标是一次性实现所有六大域的功能
- **驱动模式**: BG_WORKGRAPH（headless 逐里程碑模式）

---

## 2. 实施方案

### 2.1 角色分工

| 角色 | 谁 | 职责 |
|------|-----|------|
| 实验设计者 | Claude | 设计方案、准备环境、监控执行、记录问题 |
| 执行者 | CodeCoder (ccd/cc) | 根据 AGENTS.md 使命声明，自主完成全部开发工作 |
| 验收者 | 用户 (rong.zhu) | 最终确认结果 |

### 2.2 前期准备步骤

1. **编译 codecoder**:
   ```bash
   cd ~/Code/codecoder && cargo build --release
   ```

2. **创建目标项目目录**:
   ```bash
   mkdir ~/Code/strategic-control && cd ~/Code/strategic-control
   ```

3. **复制二进制**:
   ```bash
   cp ~/Code/codecoder/target/release/cc ~/Code/strategic-control/
   cp ~/Code/codecoder/target/release/ccd ~/Code/strategic-control/
   ```

4. **创建 AGENTS.md** — 使命声明，包含:
   - codecoder 的角色身份（资深全栈开发者）
   - 总体任务描述（基于功能清单构建完整系统）
   - 技术栈自主决策权
   - 质量要求（可用、可测试）

5. **创建 CONTEXT.md** — 领域术语表:
   - 六大业务域的定义
   - 关键术语的准确定义
   - 模块编号与路由规划

6. **复制功能清单**:
   ```bash
   cp ~/Code/codecoder/docs/proof/功能清单.md ~/Code/strategic-control/
   ```

7. **创建 codecoder.json** — 预授权开发所需工具:
   - `write_file`, `edit_file`, `read_file`
   - `run_command` (npm, cargo, git, npx 等)
   - `commit`, `glob`, `grep`
   - `search_web`, `generate_skill`, `plan`, `milestone`

8. **创建 .ccd.env** — 环境配置:
   - `CODECODER_MAX_TOKENS=8192`
   - `CODECODER_MODEL=gpt-4o`（或其他可用模型）
   - `CODECODER_BG_MAX_FIX_ATTEMPTS=3`

9. **初始化 git 仓库**:
   ```bash
   git init && git add -A && git commit -m "feat: init project scaffold"
   ```

### 2.3 运行模式

```bash
cd ~/Code/strategic-control
export CODECODER_DEFAULT_TRUST=always
CODECODER_BG_WORKGRAPH=1 CODECODER_BG_MAX_FIX_ATTEMPTS=3 ./ccd 2>&1 | tee ccd-output.log
```

**预期行为** (基于 ADR 0026/0033):
1. ccd 读取 AGENTS.md 了解使命
2. 自动拆解任务 → 创建里程碑图
3. 自动推进就绪里程碑（项目初始化 → 功能实现 → 验收）
4. 里程碑失败自动重试（最多 3 次）
5. 全部完成 → 退出码 0
6. 卡住/依赖未就绪 → 退出码 2

### 2.4 监控与终止条件

**监控指标**:
- 里程碑推进速度
- 每次 retry 的原因
- 单里程碑运行时长
- 日志中的报错信息

**终止条件**（任一满足即终止）:
1. 同一里程碑连续 retry 3 次仍 `needs_fix` → StuckNeedsFix
2. codecoder 进入死循环（同一模式重复超过 10 轮）
3. 过度探索：整轮输出只有 read_file，不写任何代码
4. 退出码 3 (CircuitBreaker) 或 4 (Error)
5. 超过 2 小时无有效进展

**终止后处理**:
1. 记录退出码和最后状态
2. 收集 ccd-output.log
3. 分析卡住的原因
4. 撰写问题清单 → 提交到 codecoder 项目作为改进依据

---

## 3. 已知风险与应对

| 风险 | 可能性 | 影响 | 应对 |
|------|--------|------|------|
| 项目规模过大，Context 超限 | 高 | codecoder 丢失早期上下文 | 依赖 codecoder 的 tier-1/tier-2 compaction |
| npm install 耗时 | 中 | 长时间无进展 | 设置合理超时 |
| 框架选择不当无法推进 | 中 | 半途换框架 | 记录问题，终止实验 |
| API key 限额耗尽 | 低 | 中断运行 | 提前确认可用额度 |
| codecoder 自身 bug 导致卡住 | 中 | 项目停滞 | 记录 bug 位置 |
| 单 turn 12 工具上限限制 | 中 | 指令执行不完整 | 依赖 codecoder 的续接能力 |

---

## 4. 成功标准

| 级别 | 标准 | 说明 |
|------|------|------|
| ✅ 完全成功 | 退出码 0，所有里程碑 done | 完整实现了系统，可运行 |
| 🔶 部分成功 | 部分里程碑完成但中途卡住 | 验证了 codecoder 的编码能力，暴露了边界问题 |
| ❌ 失败 | 早期就卡住或完全无法推进 | 暴露了严重的功能缺陷或设计问题 |

---

## 5. 附录

### 5.1 已知经验（从 kanflow/coedit 项目总结）

- trust 门: headless 模式需设 `CODECODER_DEFAULT_TRUST=always`
- codecoder.json allowlist key 格式: `write_file`, `run_command:cargo`, `commit` 等
- 复合命令按整串 keying (ADR 0036)
- 里程碑 acceptance 命令必须是独占一行的裸命令
- 验证不能信 agent 的 "all pass"，需独立验证
- 退出码 0 也可能有隐藏问题（如文件未 git add）

### 5.2 相关 ADR

- ADR 0026: Background Agent Headless Runner
- ADR 0028: Project Trust Load Gate
- ADR 0030: BG Objective Acceptance Gate
- ADR 0033: BG Ledger and Exit Codes
- ADR 0036: Compound Command Keying
