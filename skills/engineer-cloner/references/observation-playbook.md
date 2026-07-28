# observation-playbook — Phase 2 观测手册 / Phase 2 Observation Playbook

> engineer-cloner 的 **Phase 2（登录全站遍历）** 参考。上游读 `SKILL.md` 的逆向流水线，账本结构见 `references/coverage-ledger.schema.json`。观测能力全部复用 `agent-browser`，本手册不实现浏览器自动化。
>
> Reference for engineer-cloner **Phase 2 (Authenticated Full-Site Traversal)**. Upstream context: `SKILL.md`. Ledger shape: `references/coverage-ledger.schema.json`. All observation is driven through `agent-browser`.

---

## 目标 / Purpose

Phase 2 的任务：用 `agent-browser` 以**最高权限账号**登录目标站点，把浏览器里**看得见、点得动、抓得到**的每一个视图系统性地遍历一遍，逐节点写进 `.agents/clone.ledger.json`，直到账本"去干"（不再新增未观测节点）。

**The job of Phase 2**: log into the target with the highest-privilege authorized account via `agent-browser`, systematically walk every observable view, and write each into `.agents/clone.ledger.json` until the ledger runs *dry* (no new features discovered).

三条底线沿用 `SKILL.md`：

- **观测即黑盒** —— 只记录客户端可观测的部分（DOM、样式、交互、2xx 响应体、客户端可见契约）。每个功能条目最终归入三层保真度之一：`可观测精确 / 推断 / 不可观测`。
- **覆盖靠账本，不靠记忆** —— 覆盖是一份文件，不是对话记忆。loop-until-dry + coverage critic 保证全功能覆盖。
- **最高权限先行** —— 用能看到最多功能的角色遍历；低权限角色的差异（哪些功能对其不可见）在同一账本用 `roles_visible` 记录，无需重跑全站。

> 为什么用最高权限账号：管理员/超级用户能看到普通用户看不到的导航项、后台面板、批量操作与权限门控视图。先穷尽最大可见面，再用 `roles_visible` 标注每个功能对哪些角色可见，比逐角色重复遍历更高效、更完整。
> **Highest-privilege first**: the admin/superuser surface is a superset. Traverse it once, then annotate per-feature visibility with `roles_visible` instead of re-crawling per role.

---

## 登录与会话 / Login & Session

**首选：全自动登录。** 用 `agent-browser` 导航到登录页，填入授权凭据，提交，确认进入登录态（出现用户菜单 / 受保护路由 200 / cookie 已种）。会话（cookie / localStorage token）在后续所有遍历中复用。

**Preferred path — automated login.** Drive `agent-browser` to the login page, fill credentials, submit, and confirm the authenticated state (user menu present, protected route returns 200, session cookie set). Reuse that session for every subsequent traversal step.

```
1. agent-browser: 打开登录页 URL
2. agent-browser: 定位账号/密码字段，填入授权凭据
3. agent-browser: 提交，等待跳转
4. 断言登录态：受保护路由可达 / 用户身份元素出现
5. 记录会话来源到 ledger.target（authorized = true）
```

**降级：验证码 / 反爬阻断 → 半自动。** 若遇到验证码、二次验证、行为风控或人机校验，**不要**尝试绕过或自动化破解。切换半自动流程：

1. `agent-browser` 停在校验步骤，**请用户在同一浏览器会话里人工完成登录 / 过验证码**。
2. 用户完成后，`agent-browser` **复用其已建立的会话**继续遍历（会话共享，无需重新登录）。
3. **为保真度报告记录该降级**：在受影响节点的 `ui_states` 或功能名标注"半自动获取 / semi-auto"，并在 `ledger.target` 或 `CLONE-FIDELITY.md` 记一条"登录经半自动完成，验证码/反爬为 `不可观测`，克隆侧以占位/mock 实现，不镜像真实风控"。

> **Degrade rule**: on captcha / anti-bot / MFA, drop to semi-automatic — ask the human to complete login in the shared session, then let `agent-browser` continue on that session. **Record the degrade** for the fidelity report: the challenge itself is `不可观测` and must be mocked, never mirrored.

**会话失效 / 超时**：遍历中会话过期时，按上文重新建立登录态；已 `explored` / `extracted` 的节点无需重走（见「恢复 / Resume」）。

---

## 遍历策略 / Traversal Strategy

**总体形状：先广后深。** 先对导航做**广度优先**铺开（把所有一级/二级导航目标写入账本），再对每个视图做**深度穷尽**——把该视图里的每一个入口都点开。

**Shape: breadth-first over navigation, then depth-exhaust each view.**

### 每个视图必须穷尽的入口清单 / Per-view exhaustion checklist

对账本里每一个 `discovered` 节点，`agent-browser` 打开后要逐一触发并记录：

| 入口 / Entry | 动作 | 记入账本字段 |
|:------------|:-----|:------------|
| 导航项 / nav items | 点开每个一级、二级、折叠菜单项 | 新节点 `entry_path` + `name` |
| 按钮 / buttons | 触发每个可见按钮（提交、批量、切换、危险操作） | `actions` |
| 链接 / links | 跟随站内链接，站外/`do_not_clone` 只记不进 | 新节点 / `actions` |
| 弹窗触发 / modal triggers | 打开每个对话框、抽屉、确认框、气泡 | `ui_states`（含 modal 状态） |
| 标签页 / tabs | 切换每个 tab / 分段控件，各 tab 视为子视图 | `ui_states` 或子 `id` |
| 分页 / pagination | 翻到第 2 页、末页、切换每页条数 | `ui_states` + `api_calls`（分页契约） |
| 筛选 / 排序 / filters | 逐个应用筛选器、排序、搜索框，观测结果变化 | `actions` + `api_calls`（查询参数） |
| 详情页 / detail | 从列表进入至少一条详情，记录字段与子操作 | 新节点 `entry_path` |
| 空态 / empty states | 用筛选/搜索制造无结果，或找空数据入口 | `ui_states`（`empty`） |
| 错误态 / error states | 触发校验失败、无权限、404/500 页（能触发的） | `ui_states`（`error`）+ `api_calls` |
| 多步表单 / multi-step forms | 走完向导每一步，记录每步字段、校验、上一步/下一步/保存草稿 | `forms` + `ui_states`（分步） |

> **空态与错误态是最容易漏的两类。** 空态：清空筛选结果、访问尚无数据的新账号视图。错误态：故意提交非法表单、访问越权资源。无法触发的错误态（如服务端 500）标 `推断`，在 `CLONE-FIDELITY.md` 说明"错误体为推断形状"。

### SPA 特例：以点击序列为键，而非 URL / SPA: key by click-sequence, not URL

对单页应用（SPA）——很多视图**没有独立 URL**，或 URL 不随视图变化——**不能用 URL 作为账本主键**。改用**点击序列键（click-sequence key）**：记录从一个已知锚点到达该视图所需的交互路径，写入 `entry_path`。

- 例：`entry_path = "nav:设置 > tab:成员 > btn:邀请 > modal:邀请成员"`（点击序列），而非某个固定 URL。
- `id` 用稳定、可复现的短键（如 `settings.members.invite-modal`），保证 loop-until-dry 与恢复时能精确去重、跳过。
- 同一视图不同到达路径若产生**不同状态**，各记一条；若等价，合并到同一 `id` 并在 `actions` 记录多入口。

> **SPA rule**: many views share one URL. Use a **click-sequence** as `entry_path` and a stable short `id` — that is the node identity for dedup, resume, and coverage. URL is unreliable as a key.

---

## loop-until-dry

复用 `SKILL.md` 的核心循环。**默认 `dry < 2`**：需要**连续两轮**无新增（含 critic 复查后无新增）才判定收敛，避免"critic 还没查就提前判干"。每轮结束更新账本 `loop_state`（`dry_rounds` / `pass_count` / `complete`）。

Reproduces the `SKILL.md` loop. **Default `dry < 2`** — require **two consecutive** no-growth rounds before declaring the ledger dry. Each round updates `loop_state` in the ledger.

```
# Phase 2 的核心循环：反复遍历，直到账本"去干"（不再新增未观测节点）
seed_frontier ← Phase 1 发现的路由种子
将 seed_frontier 全部写入 ledger（coverage_status = "discovered"）

repeat:
    new_nodes ← 0
    for each node in ledger where coverage_status == "discovered":
        用 agent-browser 打开该 node（登录态）
        截图 + 抓 HAR + 记录 DOM 关键结构 → .agents/clone.observations/
        标注保真度层级（可观测精确 / 推断 / 不可观测）
        发现的子路由 / 交互 / 状态 / 表单 / 空态与错误态:
            if 未在 ledger 中:
                加入 ledger（coverage_status = "discovered"）
                new_nodes += 1
        将该 node 置为 coverage_status = "explored"

    # coverage critic 复查
    critic 追问：还有哪些入口 / 状态机分支 / 分页 / 权限视图 / 空态没走到？
    critic 发现的遗漏节点写入 ledger（coverage_status = "discovered"）
    new_nodes += critic 新增数

    # 每轮结束更新 loop_state
    loop_state.pass_count += 1
    if new_nodes == 0:
        loop_state.dry_rounds += 1
    else:
        loop_state.dry_rounds ← 0

until loop_state.dry_rounds >= 2      # dry < 2 默认：连续两轮无新增即收敛

loop_state.complete ← true
assert 每个 node 都有 coverage_status ∈ {"explored","extracted"} 且带保真度层级
```

> `coverage_status` 枚举沿用 schema：`discovered`（已发现未展开）→ `explored`（已遍历状态）→ `extracted`（契约与设计已提取，Phase 3/4 推进）。Phase 2 负责把节点从 `discovered` 推到 `explored`；`extracted` 由后续阶段置位。

---

## 覆盖 critic / Coverage Critic

每轮遍历结束后，coverage critic 对着账本逐条追问下面的清单；任何"没走到"的入口都要作为新 `discovered` 节点写回账本，让循环再转一轮。critic 的存在就是为了对抗长会话的遗漏。

At the end of every round, the coverage critic interrogates the ledger with the checklist below. Any "not yet opened" entry is written back as a new `discovered` node, forcing another round.

**Critic 清单（逐条问）/ Critic checklist:**

- 哪个**导航项**还没点开？折叠菜单、页脚链接、用户头像下拉里的项都算吗？
- 哪个**弹窗 / 抽屉 / 确认框**的触发按钮还没点？危险操作（删除/停用）的二次确认弹窗打开了吗？
- 哪个**空态**还没制造？有没有清空筛选、访问零数据视图看到过空占位？
- 哪个**错误态**还没触发？校验失败、无权限（403）、找不到（404）、超限提示见过吗？
- 哪个**分页**没翻到底？每页条数切换、"加载更多"、无限滚动的下一批请求抓了吗？
- 哪个**筛选 / 排序 / 搜索**组合没试？筛选后的查询参数与响应形状记了吗？
- 哪个**详情页**只看了列表没进去？详情里的子操作、关联对象跳转走到了吗？
- 哪个**多步表单**只走了第一步？向导的每一步、上一步回退、草稿保存、最终提交都覆盖了吗？
- 哪个**角色可见性**没确认？当前功能对低权限角色是否可见，`roles_visible` 标了吗？
- 哪个 **SPA 分支**只有一条到达路径被记录？同一入口的不同点击序列会到不同状态吗？

> critic 判为"全部走到"且当轮 `new_nodes == 0`，才计入一次 dry 轮；连续两轮 dry 方可收敛（见 loop-until-dry）。

---

## 记录规范 / Recording Convention

**账本文件 / Ledger.** 每个发现的功能写入 `.agents/clone.ledger.json`，严格遵循 `references/coverage-ledger.schema.json`。功能条目字段（按 schema 命名）：

- `id` —— 稳定主键（SPA 用可复现短键）。
- `entry_path` —— URL，或 SPA 的点击序列。
- `name` —— 功能名。
- `roles_visible` —— 可见此功能的角色（最高权限遍历时逐条标注）。
- `ui_states` —— 观测到的 UI 状态：默认 / 空态 / 错误态 / 加载 / modal / 分步。
- `actions` —— 可执行操作（按钮、链接、批量操作）。
- `forms` —— 表单字段与校验规则。
- `api_calls` —— 观测到的 API 调用引用（指向 observations 目录里的 HAR 条目）。
- `screenshots` —— `.agents/clone.observations/` 下的截图路径。
- `coverage_status` —— `discovered` / `explored` / `extracted`（见下）。

顶层 `target`（`url` / `authorized` / `inferred_stack` / `roles` / `do_not_clone`）与 `loop_state`（`dry_rounds` / `pass_count` / `complete`）每轮维护。

**取证目录 / Observations.** 截图与网络抓取（HAR / DOM 快照）写入 `.agents/clone.observations/`，用与功能 `id` 对应的稳定命名（如 `settings.members.invite-modal.png` / `.har`），供 Phase 6 视觉比对与 Phase 3 契约提取引用。

```
.agents/
├── clone.ledger.json            # 覆盖账本（唯一真相源）
└── clone.observations/          # 取证目录
    ├── <id>.png                 # 截图
    ├── <id>.har                 # 网络抓取（请求/响应）
    └── <id>.dom.html            # DOM 关键结构快照（按需）
```

**状态转移 / coverage_status transitions.** 每个功能沿单向路径推进，Phase 2 负责前两跳：

```
discovered ──(agent-browser 打开该视图，穷尽入口、截图、抓 HAR)──▶ explored
explored   ──(Phase 3/4 提取客户端可见契约与设计令牌)──────────────▶ extracted
```

- `discovered`：已知存在、尚未展开（种子节点、或从其他视图新发现的入口）。
- `explored`：Phase 2 已遍历完其状态（空/错/分页/表单各步），截图与 HAR 已落盘。
- `extracted`：Phase 3（契约/数据模型）与 Phase 4（设计令牌）已从该节点提取完毕。

> **只在真正做完对应工作后才推进状态。** 未穷尽入口的节点不得置 `explored`；未提取契约/设计的节点不得置 `extracted`。状态谎报会让恢复时错误跳过未观测面。

---

## 恢复 / Resume

会话重启（超时、掉线、上下文丢失、跨天续做）时，**不重头遍历**，从账本恢复：

On session restart, do **not** re-crawl. Resume from the ledger:

1. **读账本** —— 加载 `.agents/clone.ledger.json`，读 `loop_state`（还差几轮 dry）与全部功能条目。
2. **重建登录态** —— 按「登录与会话」重新登录；若原为半自动，仍走半自动。
3. **跳过已覆盖功能** —— `coverage_status == "explored"` 或 `"extracted"` 的节点视为已覆盖，**不重走**（截图/HAR 已在 observations 目录）。
4. **续做未覆盖功能** —— 从 `coverage_status == "discovered"` 的节点继续 loop-until-dry，沿用 `loop_state.dry_rounds` 计数，跑到连续两轮无新增。
5. **critic 补位** —— 恢复后先跑一轮 coverage critic，防止中断前遗漏的入口被 dry 计数误判为已收敛。

> 关键前提：状态如实记录（见「记录规范」）。只要 `explored`/`extracted` 从不谎报，恢复就能安全跳过已覆盖面、只补未观测的部分——**从账本 + observations 目录即可完全恢复 Phase 2 进度**。
