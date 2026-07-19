# Spec: 树状会话(Wave 2 / roadmap #8)

对应 [[0027-pi-comparison-and-borrowing-roadmap]] Wave 2。借鉴 pi 的树状 JSONL 会话
(`id`/`parentId` + leaf 指针 + fork/clone + `/tree` + 分支摘要)。**改动持久化格式**,
故与 [[0004-session-persistence-and-migration]] 直接相关,落地时另开 ADR 0030。

本 spec 覆盖**完整树模型**,并按 Phase A→D 分期;Phase A 是唯一改动格式与核心的一期,
B/C/D 在其之上增量叠加,各自可单独成 spec + 实现。

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

## 风险

- compaction 位置切片假设线性连续线程 —— `active_thread()` 恰好返回这样一条,无影响。
- id 唯一性:`next_message_id` 跨所有条目取 max,分支不复用。
- 格式变更:旧会话自动迁移;失败保留原文件。
- 坏 parent 链(手工损坏文件):`active_thread` 遇缺失 parent 即止,不 panic。

## 分期建议

Phase A 是地基(改格式 + 核心),自身**无用户可见功能**但风险集中在此一期;B 才带来
`/tree`/fork 的可见价值;C/D 为增值。建议:A 单独一批(spec+ADR 0030+TDD),过了再排 B。
