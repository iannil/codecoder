# Spec: 树状会话(Wave 2 / roadmap #8)

对应 [[0027-pi-comparison-and-borrowing-roadmap]] Wave 2。借鉴 pi 的树状 JSONL 会话
(`id`/`parentId` + leaf 指针 + fork/clone + `/tree` + 分支摘要)。**改动持久化格式**,
故与 [[0004-session-persistence-and-migration]] 直接相关,落地时另开 ADR 0030。

本 spec 覆盖**完整树模型**,并按 Phase A→E 分期;Phase A 是唯一改动格式与核心的一期,
B/C/D/E 在其之上增量叠加,各自可单独成 spec + 实现。E 为「因果链 × 会话树 = 推理树」的
语义层(结合 `archived/skills/rc-causal-chain`)。

## 现状(必须兼容)

`Session { schema_version:1, model, token_count, messages: Vec<Message> }` —— 线性表;
每次 append 全量原子写盘;`load` 走版本化前向迁移;`latest_session` 按 mtime 选最新;
`/resume` 加载最新。compaction(`agent.rs` context_working_set)对 `session.messages` 做
**位置切片**(`summary_span` 返回 `(start,end)`,再 `messages[start..end]`、`messages[start-1].id`)。
`Message` 自带 `id: MessageId`(会话内单调 `u64`)。

## Phase A —— 树数据模型 + 迁移 + active-thread 派生

### 数据结构(v2)

```rust
pub const SCHEMA_VERSION: u32 = 2;

pub struct SessionEntry {
    pub message: Message,           // 沿用现有 Message(携带 id / role / items)
    pub parent: Option<MessageId>,  // 仅根条目为 None
}

pub struct Session {
    pub schema_version: u32,
    pub model: String,
    pub token_count: u64,
    pub entries: Vec<SessionEntry>, // 扁平存储;树由 parent 指针表达,顺序=插入序
    pub leaf: Option<MessageId>,    // 当前位置(active thread 的末端);空会话为 None
}
```

`message.id` 即条目 id(会话内唯一)。`parent` 指向另一条目的 `message.id`。**不删除**任何
条目——离开的分支仍留在 `entries` 里,这正是"树"。

### 关键派生:active thread

其余代码不再直接读 `messages`,改为读**当前活动线程**(leaf 回溯到根):

```rust
pub fn active_thread(&self) -> Vec<Message> {
    let by_id: HashMap<MessageId, &SessionEntry> =
        self.entries.iter().map(|e| (e.message.id, e)).collect();
    let mut out = Vec::new();
    let mut cur = self.leaf;
    while let Some(id) = cur {
        let Some(e) = by_id.get(&id) else { break }; // 防御:坏 parent 链
        out.push(e.message.clone());
        cur = e.parent;
    }
    out.reverse();
    out
}
```

返回临时 `Vec<Message>`,与 compaction 的位置切片**完全兼容**(它拿到的就是一条连续线性
线程)。调用点每次 `let thread = self.session.active_thread();` 绑定一次即可。

### 变更/追加 API(移进 Session)

```rust
pub fn append(&mut self, message: Message) {
    let parent = self.leaf;
    let id = message.id;
    self.entries.push(SessionEntry { message, parent });
    self.leaf = Some(id);
}
pub fn clear(&mut self) { self.entries.clear(); self.leaf = None; }
pub fn next_message_id(&self) -> MessageId {
    self.entries.iter().map(|e| e.message.id).max().map(|m| m + 1).unwrap_or(0)
}
```

`next_message_id` 取**所有条目**最大 id + 1,分支永不复用 id。

### 迁移 v1 → v2

```rust
1 => {
    // v1 线性 messages → 链式 entries(parent = 前一条 id),leaf = 最后一条 id
    let msgs = json["messages"].as_array().cloned().unwrap_or_default();
    let mut entries = Vec::new();
    let mut prev: Option<u64> = None;
    let mut leaf: Option<u64> = None;
    for m in msgs {
        let id = m["id"].as_u64();
        entries.push(json!({ "message": m, "parent": prev }));
        prev = id; leaf = id;
    }
    json["entries"] = json!(entries);
    json["leaf"] = json!(leaf);
    json.as_object_mut().remove("messages");
    json  // schema_version 由外层 while 递增
}
```

`SCHEMA_VERSION` 改 2,`migrate` 增 `1 => {…}` 臂。旧会话加载即迁移,下次保存写为 v2。
迁移失败仍**保留原文件**(沿用 ADR 0004 保证);`load_rejects_unknown_future_version`
断言仍成立(999 > 2)。

### call-site 改法(约 15 处)

| 站点 | 现 | 改 |
|---|---|---|
| `agent.rs` append | `session.messages.push(m)` | `session.append(m)` |
| `agent.rs` clear | `session.messages.clear()` | `session.clear()` |
| `agent.rs` resume 计数 | `session.messages.len()` | `session.active_thread().len()`(或 `entries.len()`) |
| `agent.rs` context_working_set | `&session.messages` | `let t = session.active_thread(); &t`(compaction 三处切片不变) |
| `agent.rs` last_assistant_text | `session.messages.iter().rev()` | 基于 `active_thread()` |
| 测试:`session.messages = vec![…]` / `.push` / `.iter()` | 直接读写字段 | 新增 `Session::linear(model, Vec<Message>)` 测试构造器(内部按链式建 entries + leaf),读断言改走 `active_thread()` |

`background.rs` 不直接触碰 `messages`(经确认);若有,同样走 `active_thread()`。

### Phase A 测试(TDD)

- **迁移**:喂一段 v1 JSON(3 条线性),断言迁移后 `entries` 链式、`leaf`=末条,`active_thread()` == 原 messages。
- **派生**:手建带**分叉**的 entries(一个 parent 两个子),`leaf` 指其一 → `active_thread()` 只含该分支路径。
- **分支追加**:`leaf` 指向祖先后 `append` → 新条目 parent=该祖先,`active_thread()` 反映新分支,旧分支条目仍在 `entries`。
- save/load v2 往返;`next_message_id` 跨分支不复用;拒绝未来版本(沿用)。

## Phase B —— `/tree` 导航 + `/fork` + `/clone`

- `navigate_to(id)`:`leaf = Some(id)`;下次 append 从该点分叉(in-place 时间旅行)。
- `/tree`:TUI 列出 entries 树(缩进 + 折叠),选中即 `navigate_to`。新 `AgentCommand::Navigate(id)`。
- `/fork`:等价于 navigate 到选定点继续(分支在下次 append 形成)。
- `/clone`:整会话复制为新文件(新 `session-<stamp>.json`),继续独立演进。
- 渲染:TUI 需能显示"当前在某历史点、后面还有别的分支"。属 Phase B 细化。

## Phase C —— 离开分支时摘要

`navigate_to` 若使某分支被"抛弃"(新 leaf 不在其路径上),复用
`AgentLoop::summarize_span`([[0023-context-compaction]] tier-2)对被弃分支生成摘要,作为
一条合成条目挂到新位置(或先发 `Confirm` 询问)。细化留 Phase C。

## Phase D —— 配置即条目(可选、最小)

pi 把 model/thinking/active-tools 变更做成 transcript 条目、按重放派生状态。codecoder 目前
只有 **model** 一个配置维度(无 thinking level / active-tools 切换)。若要做,把 `SessionEntry`
从"总是 Message"升级为**带类型**的枚举(`Message(Message) | ModelChange(String) | …`),需
v2→v3 迁移。**Phase A 先不引入枚举**(YAGNI);Phase D 真要时再迁移,避免 A 期复杂化。

## Phase E —— 推理树语义层(因果链 × 会话树)

结合 `archived/skills/rc-causal-chain`(因果链 / 观察收敛方法论):把会话树从"泛泛的对话
分支"升级为**一等公民的推理/根因树**,专治 coding agent 的系统化调试与根因分析("为什么
老是失败")。**机制来自 pi(树 + 离开分支摘要),语义来自因果链。**

映射:

| 因果链概念 | 落到本 spec 的树上 |
|---|---|
| 一个候选原因 = 一个节点,一次只深挖一条(反过早共识) | 一个分支 = 探索一个假设;`navigate_to` 就地开分支(Phase B) |
| 每节点 `观察锁定` / `[假设]` | 条目带 `status: locked \| hypothesis`(见下,依赖 Phase D 的带类型条目) |
| 判定末端 / 排除某原因 | 离开分支 → Phase C 的 branch-summary 记"此因已排除,因为…" |
| 关键节点 = 可用余量最大 × 杠杆最高 | 树视图高亮"最该修的那一环" |

落地形态(不改 Phase A 地基):

- **元数据而非新格式**:节点的 `status`/`margin`/`leverage` 走 Phase D 的带类型条目
  (或先塞进一个 `SessionEntry.meta: Option<serde_json::Value>` 旁路字段),**不冲击 Phase A**。
- **方法论进 `skills/`**:把因果链纪律写成一个 codecoder Skill(`skills/debug-causal.md`),
  用 `use_skill` 激活时注入——agent 用会话树当基底、按因果链纪律逐节点展开。这是"文件系统即
  自我"的正道:**机制在内核(树),方法在磁盘(skill)**。
- 与 Phase C 天然配对:排除一个原因 = 离开该分支 = 自动摘要"为何排除",避免重复挖同一条死路。

> **该用例反向锁定了"完整树 vs 文件级 fork"的选择**:推理树需要*就地分支 + 离开即摘要*,
> 文件级 fork(换文件、丢上下文)给不了。故 Wave 2 应走本 spec 的完整树。

## 相关但独立(不属本 spec):inspector rubric 快胜

`archived/skills/engineer-inspector` 的三个"架构偏移"信号(篡改地基 / 过度设计 / 体积失控)
可直接做成 codecoder `review` 工具的结构化 rubric(对 `git diff` × `CONTEXT.md`/ADR 比对)。
零架构改动、独立于会话树,可作为并行快胜——**另开 skill/prompt,不阻塞本 spec**。

## 风险

- compaction 位置切片假设线性连续线程 —— `active_thread()` 恰好返回这样一条,无影响。
- id 唯一性:`next_message_id` 跨所有条目取 max,分支不复用。
- 格式变更:旧会话自动迁移;失败保留原文件。
- 坏 parent 链(手工损坏文件):`active_thread` 遇缺失 parent 即止,不 panic。

## 分期建议

Phase A 是地基(改格式 + 核心),自身**无用户可见功能**但风险集中在此一期;B 才带来
`/tree`/fork 的可见价值;C 摘要、D 配置即条目、**E 推理树语义层**为增值。

顺序建议:
1. **A**(完整树地基)单独一批:spec + ADR 0030 + TDD。①推理树用例已背书走完整树。
2. **B**(`/tree` 最小跳转 + `/fork`/`/clone`)。
3. **C**(离开分支摘要)、**D**(带类型条目,为 E 的 `status` 铺路)。
4. **E**(推理树):元数据 + `skills/debug-causal.md` 方法论,机制在内核、方法在磁盘。
5. **并行快胜(独立)**:engineer-inspector 三信号 → `review` rubric,不阻塞以上任何一期。
