// 客户端 ↔ daemon 的可序列化线协议（newline-delimited JSON）。
// 与进程内 `AgentCommand`/`AgentEvent` 平行存在：后者携带 oneshot Sender，无法 serde，
// 故 daemon 在两者间翻译。ADR 0016 的通道拓扑不变。
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{BufRead, Write};

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
}
