// 客户端 ↔ daemon 的可序列化线协议（newline-delimited JSON）。
// 与进程内 `AgentCommand`/`AgentEvent` 平行存在：后者携带 oneshot Sender，无法 serde，
// 故 daemon 在两者间翻译。ADR 0016 的通道拓扑不变。
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

/// 用户对交互式提示的回答（mirrors 5 种 reply 类型，无 oneshot）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptAnswer {
    Permission { grant: PermissionGrant },
    AskUser { text: String },
    Confirm { yes: bool },
    PlanApproval { approved: bool },
    Trust { decision: TrustDecisionWire },
}

/// daemon 发向客户端的提示内容（mirrors 5 种 prompt-bearing AgentEvent，
/// 但不包含不可序列化的 oneshot）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptBody {
    Permission { key: String, preview: String },
    AskUser { prompt: String },
    Confirm { prompt: String },
    PlanApproval { plan: String },
    Trust { root: String },
}

/// 用户对权限请求的授权范围（mirrors `PermissionReply` 中的 `PermScope`，但不
/// 耦合 agent 类型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionGrant {
    Once,
    AlwaysThisSession,
    AlwaysThisProject,
    Deny,
    Cancelled,
}

/// 用户对项目信任的决定（mirrors `TrustReply`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustDecisionWire {
    Always,
    Once,
    Never,
}

/// One node of the session tree, for the `cc tree` view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: u64,
    pub parent: Option<u64>,
    /// "user" | "assistant" | "system" | "tool"
    pub role: String,
    /// First non-empty line of the message, truncated.
    pub preview: String,
    /// true iff this entry is the session's current leaf.
    pub is_leaf: bool,
    /// true iff this entry is on the leaf→root active thread.
    pub on_active_path: bool,
}

/// 客户端 → daemon 的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRequest {
    SendMessage { content: String },
    NewSession,
    ListSessions,
    Resume { id: String },
    Shutdown,
    Status,
    /// 对 daemon 发来的 `ServerEvent::Prompt` 的回答。
    PromptReply { id: u64, answer: PromptAnswer },
    /// 显示活动 session 的会话树（`cc tree`）。
    TreeShow,
    /// 导航活动 session 的 leaf 到 id（`cc fork <id>`；下次 append 即分叉）。
    TreeNav { id: u64 },
    /// 复制活动 session 为新 session 文件（`cc clone`）。
    TreeClone,
}

/// daemon → 客户端的事件。一个 `SendMessage` 会产生一串事件，以 `TurnComplete` 或
/// `Error` 收尾。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    StreamDelta { text: String },
    Notice { text: String },
    Context { pct: u16 },
    ToolStarted { name: String, preview: String },
    ToolFinished { name: String, is_error: bool, output: String },
    TurnComplete,
    SessionCreated { id: String },
    Sessions { ids: Vec<String> },
    Error { message: String },
    /// 交互式提示（permission/ask/confirm/plan/trust），需客户端回答后继续。
    Prompt { id: u64, body: PromptBody },
    /// daemon 级广播通知（来自 event bus，如 workgraph/supervisor）。
    /// 与 per-turn `Notice` 区分：带 `source` 标签，客户端可不同渲染。
    BusNotice { source: String, text: String },
    /// 会话树视图（响应 `TreeShow`）。
    Tree { nodes: Vec<TreeNode> },
}

/// 从一行读一个 `ClientRequest`。`Ok(None)` 表示客户端关闭（EOF）。
pub fn read_request(r: &mut impl BufRead) -> anyhow::Result<Option<ClientRequest>> {
    let mut line = String::new();
    let n = r.read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    let req: ClientRequest = serde_json::from_str(line.trim())?;
    Ok(Some(req))
}

/// 写一个 `ServerEvent`（单行 JSON + `\n`）。
pub fn write_event(w: &mut impl Write, e: &ServerEvent) -> anyhow::Result<()> {
    let json = serde_json::to_string(e)?;
    writeln!(w, "{json}")?;
    w.flush()?;
    Ok(())
}

// ============================================================================
// PromptAnswer → agent reply-type 转换助手（由 socket.rs 在收到 PromptReply 后调用）
// ============================================================================

impl PromptAnswer {
    /// 转换为 `crate::agent::PermissionReply`（导入类型由调用方提供）。
    pub fn to_permission_reply(&self) -> crate::agent::PermissionReply {
        match self {
            PromptAnswer::Permission { grant } => match grant {
                PermissionGrant::Once => crate::agent::PermissionReply::Grant(crate::permission::PermScope::Once),
                PermissionGrant::AlwaysThisSession => crate::agent::PermissionReply::Grant(crate::permission::PermScope::AlwaysThisSession),
                PermissionGrant::AlwaysThisProject => crate::agent::PermissionReply::Grant(crate::permission::PermScope::AlwaysThisProject),
                PermissionGrant::Deny => crate::agent::PermissionReply::Deny,
                PermissionGrant::Cancelled => crate::agent::PermissionReply::Cancelled,
            },
            _ => unreachable!("to_permission_reply called on non-Permission PromptAnswer"),
        }
    }

    /// 提取 `AskUser` 的文本回答。
    pub fn into_text(self) -> String {
        match self {
            PromptAnswer::AskUser { text } => text,
            _ => unreachable!("into_text called on non-AskUser PromptAnswer"),
        }
    }

    /// 提取 `Confirm` 的布尔回答。
    pub fn yes(&self) -> bool {
        match self {
            PromptAnswer::Confirm { yes } => *yes,
            _ => unreachable!("yes called on non-Confirm PromptAnswer"),
        }
    }

    /// 提取 `PlanApproval` 的布尔回答。
    pub fn approved(&self) -> bool {
        match self {
            PromptAnswer::PlanApproval { approved } => *approved,
            _ => unreachable!("approved called on non-PlanApproval PromptAnswer"),
        }
    }

    /// 转换为 `crate::agent::TrustReply`（导入类型由调用方提供）。
    pub fn to_trust_reply(&self) -> crate::agent::TrustReply {
        match self {
            PromptAnswer::Trust { decision } => match decision {
                TrustDecisionWire::Always => crate::agent::TrustReply::Always,
                TrustDecisionWire::Once => crate::agent::TrustReply::Once,
                TrustDecisionWire::Never => crate::agent::TrustReply::Never,
            },
            _ => unreachable!("to_trust_reply called on non-Trust PromptAnswer"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn client_request_roundtrips() {
        let cases = vec![
            ClientRequest::SendMessage { content: "hi".into() },
            ClientRequest::NewSession,
            ClientRequest::ListSessions,
            ClientRequest::Resume { id: "abc123".into() },
            ClientRequest::Shutdown,
            ClientRequest::Status,
        ];
        for req in cases {
            let json = serde_json::to_string(&req).unwrap();
            let back: ClientRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(req, back, "round-trip failed for {json}");
            // tag 用 snake_case
            assert!(json.contains("\"type\":"));
        }
    }

    #[test]
    fn server_event_writes_one_line_and_reads_back() {
        let ev = ServerEvent::StreamDelta { text: "hello\nworld".into() };
        let mut buf: Vec<u8> = Vec::new();
        write_event(&mut buf, &ev).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // 恰好一行（内容里没有真实换行被保留为 JSON 转义）
        assert_eq!(s.matches('\n').count(), 1);
        assert!(s.ends_with("\n"));
        let back: ServerEvent = serde_json::from_str(s.trim()).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn read_request_returns_none_on_eof() {
        let mut r = Cursor::new("");
        assert!(read_request(&mut r).unwrap().is_none());
    }

    // ===== Task 9a: 新增协议类型的 serde round-trip 测试 =====

    #[test]
    fn prompt_answer_serde_roundtrips() {
        let cases = vec![
            PromptAnswer::Permission { grant: PermissionGrant::Once },
            PromptAnswer::Permission { grant: PermissionGrant::AlwaysThisSession },
            PromptAnswer::Permission { grant: PermissionGrant::AlwaysThisProject },
            PromptAnswer::Permission { grant: PermissionGrant::Deny },
            PromptAnswer::Permission { grant: PermissionGrant::Cancelled },
            PromptAnswer::AskUser { text: "my answer".into() },
            PromptAnswer::Confirm { yes: true },
            PromptAnswer::Confirm { yes: false },
            PromptAnswer::PlanApproval { approved: true },
            PromptAnswer::PlanApproval { approved: false },
            PromptAnswer::Trust { decision: TrustDecisionWire::Always },
            PromptAnswer::Trust { decision: TrustDecisionWire::Once },
            PromptAnswer::Trust { decision: TrustDecisionWire::Never },
        ];
        for ans in cases {
            let json = serde_json::to_string(&ans).unwrap();
            let back: PromptAnswer = serde_json::from_str(&json).unwrap();
            assert_eq!(ans, back, "round-trip failed for {json}");
        }
    }

    #[test]
    fn prompt_body_serde_roundtrips() {
        let cases = vec![
            PromptBody::Permission { key: "run_command:rm".into(), preview: "rm -rf /".into() },
            PromptBody::AskUser { prompt: "what color?".into() },
            PromptBody::Confirm { prompt: "proceed?".into() },
            PromptBody::PlanApproval { plan: "step 1, step 2".into() },
            PromptBody::Trust { root: "/tmp/project".into() },
        ];
        for body in cases {
            let json = serde_json::to_string(&body).unwrap();
            let back: PromptBody = serde_json::from_str(&json).unwrap();
            assert_eq!(body, back, "round-trip failed for {json}");
        }
    }

    #[test]
    fn server_event_prompt_serde_roundtrips() {
        let ev = ServerEvent::Prompt {
            id: 42,
            body: PromptBody::AskUser { prompt: "favorite number?".into() },
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: ServerEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn client_request_prompt_reply_serde_roundtrips() {
        let req = ClientRequest::PromptReply {
            id: 7,
            answer: PromptAnswer::Confirm { yes: true },
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ClientRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn permission_grant_all_variants_serialize() {
        let variants = vec![
            PermissionGrant::Once,
            PermissionGrant::AlwaysThisSession,
            PermissionGrant::AlwaysThisProject,
            PermissionGrant::Deny,
            PermissionGrant::Cancelled,
        ];
        for grant in variants {
            let json = serde_json::to_string(&grant).unwrap();
            let back: PermissionGrant = serde_json::from_str(&json).unwrap();
            assert_eq!(grant, back);
            // 确保用的是 snake_case（serde(rename_all)）
            assert!(json.contains("once") || json.contains("always_this_session") || json.contains("always_this_project") || json.contains("deny") || json.contains("cancelled"));
        }
    }

    #[test]
    fn trust_decision_wire_all_variants_serialize() {
        let variants = vec![
            TrustDecisionWire::Always,
            TrustDecisionWire::Once,
            TrustDecisionWire::Never,
        ];
        for decision in variants {
            let json = serde_json::to_string(&decision).unwrap();
            let back: TrustDecisionWire = serde_json::from_str(&json).unwrap();
            assert_eq!(decision, back);
        }
    }

    #[test]
    fn tree_node_and_variants_serde_roundtrip() {
        let n = TreeNode {
            id: 5, parent: Some(2), role: "assistant".into(), preview: "hi".into(),
            is_leaf: true, on_active_path: true,
        };
        let j = serde_json::to_string(&n).unwrap();
        let back: TreeNode = serde_json::from_str(&j).unwrap();
        assert_eq!(n, back);

        let reqs = vec![
            ClientRequest::TreeShow,
            ClientRequest::TreeNav { id: 7 },
            ClientRequest::TreeClone,
        ];
        for r in reqs {
            let j = serde_json::to_string(&r).unwrap();
            assert_eq!(r, serde_json::from_str::<ClientRequest>(&j).unwrap());
        }
        let ev = ServerEvent::Tree { nodes: vec![n.clone()] };
        let j = serde_json::to_string(&ev).unwrap();
        let back: ServerEvent = serde_json::from_str(&j).unwrap();
        assert_eq!(ev, back);
        assert!(j.contains("\"type\":\"tree\""));
    }
}
