// tests/testkit/driver.rs — spawns the real AgentLoop, drives one turn to
// completion, and answers blocking permission/ask/confirm/plan round-trips.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use codecoder::{AgentCommand, AgentEvent, AgentLoop, PermScope, PermissionReply, Provider};

use super::scripted_provider::{RecordedRequest, Recorder};

#[derive(Clone, Copy)]
pub enum PermPolicy {
    GrantOnce,
    GrantSession,
    GrantProject,
    Deny,
}

/// Map a policy to the concrete reply. Kept private so later tasks (`run_steps`)
/// reuse it instead of duplicating the match.
fn reply_for(perm: PermPolicy) -> PermissionReply {
    match perm {
        PermPolicy::GrantOnce => PermissionReply::Grant(PermScope::Once),
        PermPolicy::GrantSession => PermissionReply::Grant(PermScope::AlwaysThisSession),
        PermPolicy::GrantProject => PermissionReply::Grant(PermScope::AlwaysThisProject),
        PermPolicy::Deny => PermissionReply::Deny,
    }
}

pub struct RunOutcome {
    pub events: Vec<AgentEvent>,
    pub requests: Vec<RecordedRequest>,
}

impl RunOutcome {
    pub fn stream_text(&self) -> String {
        self.events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::StreamDelta(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }
    pub fn permission_keys(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::PermissionRequest { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect()
    }
    pub fn tool_outputs(&self, name: &str) -> Vec<(bool, String)> {
        self.events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolFinished {
                    name: n,
                    is_error,
                    output,
                } if n == name => Some((*is_error, output.clone())),
                _ => None,
            })
            .collect()
    }
}

/// Drive one turn to completion. Answers blocking round-trips per policy/answers.
pub fn run_turn(
    root: PathBuf,
    provider: Arc<dyn Provider>,
    recorder: Recorder,
    msg: &str,
    perm: PermPolicy,
    mut answers: Vec<String>,
) -> RunOutcome {
    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>();
    let agent = AgentLoop::new(provider, "test-model", 4096, 0.0, root);
    let handle = thread::spawn(move || agent.run(cmd_rx, event_tx));
    cmd_tx
        .send(AgentCommand::ProcessMessage(msg.into()))
        .unwrap();

    let mut events = Vec::new();
    let deadline = Duration::from_secs(5);
    loop {
        match event_rx.recv_timeout(deadline) {
            Ok(AgentEvent::TurnComplete) => {
                events.push(AgentEvent::TurnComplete);
                break;
            }
            Ok(AgentEvent::PermissionRequest {
                key,
                preview,
                reply_tx,
            }) => {
                let _ = reply_tx.send(reply_for(perm));
                // Record the occurrence with a dummy tx (only key/preview are inspected).
                events.push(AgentEvent::PermissionRequest {
                    key,
                    preview,
                    reply_tx: mpsc::channel().0,
                });
            }
            Ok(AgentEvent::AskUser { prompt, reply_tx }) => {
                let a = if answers.is_empty() {
                    String::new()
                } else {
                    answers.remove(0)
                };
                let _ = reply_tx.send(a);
                events.push(AgentEvent::AskUser {
                    prompt,
                    reply_tx: mpsc::channel().0,
                });
            }
            Ok(AgentEvent::Confirm { prompt, reply_tx }) => {
                let yes = answers.first().map(|s| s == "yes").unwrap_or(true);
                if !answers.is_empty() {
                    answers.remove(0);
                }
                let _ = reply_tx.send(yes);
                events.push(AgentEvent::Confirm {
                    prompt,
                    reply_tx: mpsc::channel().0,
                });
            }
            Ok(AgentEvent::PlanApproval { plan, reply_tx }) => {
                let _ = reply_tx.send(true);
                events.push(AgentEvent::PlanApproval {
                    plan,
                    reply_tx: mpsc::channel().0,
                });
            }
            Ok(other) => events.push(other),
            Err(_) => break, // timeout — return what we have
        }
    }
    let _ = cmd_tx.send(AgentCommand::Shutdown);
    let _ = handle.join();
    let requests = recorder.lock().unwrap().clone();
    RunOutcome { events, requests }
}
