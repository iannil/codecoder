# kanflow — 多用户实时 Kanban 看板 / 用 codecoder 自主建成设计

- 日期:2026-07-24
- 目标目录:`~/Code/kanflow`
- 驱动器:codecoder(编译自 `~/Code/codecoder`,二进制复制进 `kanflow/bin/`)
- 模型:`deepseek-v4-flash`(deepseek.com)
- 首要目的:**两者兼顾、偏压测** —— 以 codecoder 能力覆盖为主线,但每个里程碑要真实可用、有验收门。

---

## 0. 背景与意图

这是一次**用 codecoder 自主建成一个与 codecoder 无关的全栈 web 项目**的练习,同时作为对
codecoder 前端(设计/排版/交互)与后端(数据库/接口/CRUD)能力的一次压力测试。

分工原则:**我(Claude)只搭控制平面并监督;所有项目代码由 codecoder 自主写。**

栈:**Rust API(axum + sqlx/SQLite + argon2 + tokio)+ React/Vite/TS SPA**。
后端验收走 `cargo test`(bg_gate 原生识别);前端 `web/` 由 `make web-check` 跑 `tsc --noEmit && vitest run && vite build`。

---

## 1. 产品:kanflow

**一句话**:Trello 式看板。用户注册登录后创建看板,看板内有列(list)、卡片(card),支持跨列拖拽与
列内重排;看板可邀请成员协作,任何人的改动经 **SSE 实时广播**给其他在线成员。

### 硬轴(压测点)
- **前端**:指针拖拽(卡片跨列 + 列内重排 + 列整体重排)、乐观更新 + SSE 对账、卡片弹窗、
  标签/成员筛选、一套克制但成型的设计系统。
- **后端**:关系型 schema + 迁移、会话认证(argon2id + httpOnly cookie)、逐操作鉴权
  (看板成员校验,防越权)、fractional-index 排序算法(避免重排整列)、SSE 广播中枢
  (每看板一个 tokio broadcast 频道)、6+ 张表的 CRUD、活动流。

### YAGNI(明确不做)
不做 OAuth / 邮箱验证 / 找回密码;不做后端全文检索(筛选放前端);不做附件/图片上传;
不做移动端专门适配;实时用 SSE 不用 WebSocket(写走 REST)。

---

## 2. 架构

```
kanflow/
  Cargo.toml            # 后端 axum + sqlx(SQLite) + argon2 + tokio
  migrations/           # sqlx 迁移
  src/                  # handlers / auth / order / sse / db / models
  tests/                # cargo test 集成测试(tower oneshot + in-mem SQLite)
  web/                  # React + Vite + TS SPA(vitest + tsc + build)
  Makefile              # make api-test / make web-check / make check / make scaffold-check
  codecoder.json        # 权限 allowlist(我预写)
  AGENTS.md CONTEXT.md  # codecoder 的"自我"(我预写)
  .ccd.env              # 白名单调参(我预写)
  bin/                  # 我编译并复制进来的 codecoder + cc 二进制
```

后端单一 crate → `cargo test` 原生验收。生产由 axum 直接托管 `web/dist` 静态资源。

**技术约定(写进 CONTEXT.md / AGENTS.md,降低 flash 出错面):**
- sqlx 用**运行时** `sqlx::query` / `query_as`,**不用编译期宏**,规避 `sqlx prepare` 摩擦。
- `position` 用 fractional-index **字符串键**(两键之间生成中点键),拖拽只更新单条记录。
- 错误统一 `AppError` → `IntoResponse`;handler 返回 `Result<Json<T>, AppError>`。

---

## 3. 数据模型(6+ 表)

- `users`(id, username 唯一, password_hash, created_at)
- `sessions`(token, user_id, expires_at)
- `boards`(id, name, owner_id, created_at)
- `board_members`(board_id, user_id, role: owner|member)
- `lists`(id, board_id, name, position TEXT)
- `cards`(id, list_id, title, description, position TEXT, due_date?)
- `labels`(id, board_id, name, color) + `card_labels`(card_id, label_id)
- `activity`(id, board_id, user_id, kind, payload_json, created_at)

---

## 4. API(REST + SSE)

- 认证:`POST /api/register`、`POST /api/login`、`POST /api/logout`、`GET /api/me`
- 看板:`GET/POST /api/boards`、`GET/PATCH/DELETE /api/boards/:id`、`POST /api/boards/:id/members`
- 列:`POST /api/boards/:id/lists`、`PATCH/DELETE /api/lists/:id`(含 move=重定 position)
- 卡片:`POST /api/lists/:id/cards`、`PATCH/DELETE /api/cards/:id`(含跨列 move)
- 标签:`GET/POST /api/boards/:id/labels`;卡片贴/撤标签
- 实时:`GET /api/boards/:id/events`(SSE,带 `Last-Event-ID` 断线重连);每次成功变更向该看板频道广播。
- 统一鉴权中间件:登录态 + 该用户是该看板成员;每个变更前校验(防越权)。

---

## 5. 前端

- **设计系统**:一组 tokens(色板/间距/字号/圆角/阴影),浅色为主 + 单一强调色;
  卡片/列/弹窗/按钮/输入统一样式。目标"看着是有人设计过的",不追求花哨。
- **视图**:登录/注册页;看板列表页;看板详情页(水平滚动的列 + 卡片);
  卡片弹窗(标题/描述/标签/成员/删除);成员邀请弹窗。
- **交互**:指针拖拽卡片(跨列/列内)+ 拖拽列重排,拖拽乐观更新,落库后以服务端返回校正;
  SSE 到达的他人变更实时合并;按标签/文本客户端筛选;Esc 关弹窗、Enter 快速加卡。

---

## 6. 引导(bootstrap)

1. `cd ~/Code/codecoder && cargo build --release`(编译 codecoder)。
2. `mkdir ~/Code/kanflow && git init`;复制 `target/release/{codecoder,cc}` 进 `~/Code/kanflow/bin/`。
3. 我预写**控制平面文件**(非项目代码):`AGENTS.md`(项目身份+纪律+禁止过度探索)、
   `CONTEXT.md`(术语表)、`codecoder.json`(allowlist)、`.ccd.env`
   (`MODEL=deepseek-v4-flash`、`MAX_TOKENS=8192`、`BG_MAX_FIX_ATTEMPTS=3`)、`mission_state`(workgraph)。
4. **信任/权限**:`~/.codecoder/trust.json` 加入 `kanflow`;启动带 `CODECODER_DEFAULT_TRUST=always` 兜底。
   allowlist 键:`write_file, edit_file, commit, generate_skill, generate_capability, run_capability,
   run_command:cargo, run_command:make, run_command:npm, run_command:sqlx, run_command:git`
   (grep/search/reason 是 `Permission::None`,免授权)。
5. **密钥**:`API_KEY/API_BASE` 被安全策略拒绝从 `.ccd.env` 注入 → 启动时真实 shell `export`。

---

## 7. 里程碑工作图

每个节点验收门都是**独占一行的裸命令**(走 Makefile 目标,门命令永远是裸 `make X`,
一次授权 `run_command:make` 全覆盖,避开复合命令逐串授权)。

```
M0 scaffold ─┬─ M1 db+migrations ── M2 auth ─┐
             │                                ├─ M4 CRUD+鉴权 ── M5 SSE hub ─┐
             ├─ M3 ordering(fractional-index)─┘                              ├─ M9 集成+托管+smoke
             └─ M6 设计系统+认证页 ── M7 看板视图+卡片弹窗 ── M8 拖拽+乐观+SSE客户端 ┘
```

| 里程碑 | 内容 | 验收门(裸命令) |
|---|---|---|
| **M0** | Cargo(lib+bin, axum/tokio/sqlx/argon2)、Makefile、`web/` vite+react+ts 骨架、`migrations/`、.gitignore | `make scaffold-check` |
| **M1** | 6+ 表 sqlx 迁移、db 模块、models(运行时 `sqlx::query`) | `make api-test` |
| **M2** | register/login/logout/me、argon2id、sessions、鉴权中间件 | `make api-test` |
| **M3** | fractional-index `key_between` 纯模块 + 边界单测(首/尾/相邻取中点) | `make api-test` |
| **M4** | board/list/card handlers、成员鉴权(防越权)、move 端点用 M3、活动流 | `make api-test` |
| **M5** | 每看板 tokio broadcast、`GET events` SSE、变更广播、Last-Event-ID | `make api-test` |
| **M6** | 设计 tokens + 基础组件、登录/注册页接 API、会话态 | `make web-check` |
| **M7** | 看板列表页、看板详情(列/卡片)、卡片弹窗、标签/成员 | `make web-check` |
| **M8** | 指针拖拽(跨列/列内/列重排)、乐观更新+对账、SSE 合并、筛选、键盘 | `make web-check` |
| **M9** | axum 托管 `web/dist`、smoke Capability 跑通登录→建看板→拖卡流程 | `make check` |

- `make api-test` → `cargo test`
- `make web-check` → `cd web && tsc --noEmit && vitest run && vite build`
- `make check` → 前两者全跑
- `make scaffold-check` → `cargo build && cd web && npm ci && tsc --noEmit && vite build`

里程碑"完成"以**我独立复跑**该门为准,不信 flash 自述。

---

## 8. 能力覆盖计划(把"尽量用所有能力"落到具体节点)

- **原生 Tool**(全程自然用):read/list/write/edit_file、run_command、glob/grep(AST 查自身代码)、
  diff、commit(每里程碑)、plan、milestone、memory、reason。
- **search_web / search_github**:M3/M5 指令要求先检索 axum SSE 与 `generateKeyBetween` 参考实现。
- **agent(子 agent)**:M3 排序模块交给派生子 agent 独立建+测(隔离、可单测)。
- **review**:M4、M8 提交前对 diff 跑 `review` 自审。
- **generate_prompt → promote_prompt → use_skill**:M0/M1 把"handler/错误/鉴权约定"起草为
  prompt→晋升为 skill `kanflow-api-conventions`,后续里程碑 `use_skill` 激活。
- **generate_capability + run_capability**(覆盖 Environment×Lifecycle):
  - `seed-demo`(Shell/OneShot)灌演示数据;
  - `dev-server`(Shell/**Persistent**)常驻跑 axum,压 supervisor;
  - `smoke`(Shell/OnDemand)curl 全流程,M9 验收用。

---

## 9. 监督环(混合驱动:workgraph 为主 + 我监督)

1. bootstrap 后用 `cc` **交互式**跑通 M0(亲眼确认脚手架能 build),再交给 headless。
2. `CODECODER_BG_WORKGRAPH=1` 逐里程碑推进,`BG_MAX_FIX_ATTEMPTS=3` 自动重试,`MAX_TOKENS=8192`。
   监控退出码:0 完成就绪 / 2 卡住需人工 / 3 熔断 / 4 错误。
3. 每里程碑我**独立**跑门验证;卡住(exit 2)时读失败输出 → 要么带更精确修复提示把节点重置
   `pending`,要么我手修 footgun,要么改用 `cc` 续推。
4. **切忌向同一常驻 daemon 并发发消息**(共享 session 历史会文件竞争)→ 串行,等每 turn 完再发。
5. 收尾用 **agent-browser** 打开跑起来的应用,截图 + 实操拖拽,**实证验收前端设计/排版/交互**
   (补足 flash 自述不可信)。

---

## 10. 风险与对策

| 风险 | 对策 |
|---|---|
| flash 弱、易谎报/过度探索 | 每里程碑精确指令 + 内联类型签名 + "禁止过度探索" + 小步写;测试我自己复跑 |
| sqlx 编译期宏需 prepare | 强制运行时 `sqlx::query`,零 prepare |
| `npm ci` 联网 | 授权 `run_command:npm`,scaffold 阶段我先手动 `npm install` 探路 |
| 复合命令逐串授权 prompt | 验收门全走裸 `make X` |
| SSE 测试 flaky | hub 单测同一测试内订阅+触发+轮询,确定性 |
| max_tokens 截断大文件 | `CODECODER_MAX_TOKENS=8192` + 按模块小步写 |

---

## 11. 成功标准

- M0–M9 全部里程碑门在**我独立复跑**下通过(`make check` 绿)。
- agent-browser 实操:注册→登录→建看板→建列/卡→拖拽跨列/列内/列重排→第二浏览器上下文见 SSE 实时更新。
- codecoder 能力覆盖:上述 §8 列出的 Tool/Skill/Capability/agent/review/workgraph 均有真实调用记录。
- 越权防护:非成员访问他人看板 API 返回 403/404(集成测试覆盖)。
