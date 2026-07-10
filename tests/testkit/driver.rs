// tests/testkit/driver.rs — spawns the real AgentLoop, drives one turn to
// completion, and answers blocking permission/ask/confirm/plan round-trips.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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
    /// Wall-clock duration the driver observed for the turn. Only meaningful for
    /// `run_turn_with_cancel`; `run_turn` reports the full drive time.
    pub elapsed: Duration,
    /// True iff the drive loop exited because it observed `AgentEvent::TurnComplete`
    /// (the turn ran to completion); false if it exited via the 5s `recv_timeout`
    /// (a hung/mis-dispatched turn). Negative "absence" tests read this to prove a
    /// tool was actually reached and guarded — not merely never run. For
    /// `run_steps` this reflects whether the LAST step completed; for
    /// `run_turn_with_cancel` it is left `false` (that path intentionally may not
    /// complete).
    pub completed: bool,
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
    let start = Instant::now();
    let mut completed = false;
    loop {
        match event_rx.recv_timeout(deadline) {
            Ok(AgentEvent::TurnComplete) => {
                events.push(AgentEvent::TurnComplete);
                completed = true;
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
    RunOutcome {
        events,
        requests,
        elapsed: start.elapsed(),
        completed,
    }
}

/// A single step in a multi-step drive of one long-lived agent thread.
pub enum Step {
    /// Send `ProcessMessage(_)` and drain to `TurnComplete`.
    Msg(String),
    /// Send `AgentCommand::Reload` (rescan skills/prompts/capabilities + rebuild
    /// system prompt) and drain to its `TurnComplete`.
    Reload,
    /// Send `AgentCommand::Resume` (load + forward-migrate the latest session on
    /// disk and adopt it as the live session, ADR 0004). Terminates with a
    /// `Notice` + `TurnComplete`, drained by the same loop as the other steps.
    Resume,
}

/// Drive several steps over ONE agent thread, so state authored in an early step
/// (e.g. a skill written by `generate_skill`) is visible to a later step after a
/// `Reload`. Reuses `reply_for` for permission round-trips (no duplicated match).
/// Ends with `Shutdown` + join and returns the accumulated `RunOutcome` (all
/// events across all steps, plus every recorded provider request).
pub fn run_steps(
    root: PathBuf,
    provider: Arc<dyn Provider>,
    recorder: Recorder,
    steps: Vec<Step>,
    perm: PermPolicy,
) -> RunOutcome {
    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>();
    let agent = AgentLoop::new(provider, "test-model", 4096, 0.0, root);
    let handle = thread::spawn(move || agent.run(cmd_rx, event_tx));

    let mut events = Vec::new();
    let start = Instant::now();
    let deadline = Duration::from_secs(5);
    // Tracks whether the CURRENT step reached `TurnComplete`; after the loop it
    // reflects the LAST step's outcome (true = completed, false = timed out).
    let mut completed = false;

    for step in steps {
        completed = false;
        match step {
            Step::Msg(m) => {
                cmd_tx.send(AgentCommand::ProcessMessage(m)).unwrap();
            }
            Step::Reload => {
                cmd_tx.send(AgentCommand::Reload).unwrap();
            }
            Step::Resume => {
                cmd_tx.send(AgentCommand::Resume).unwrap();
            }
        }
        // Both ProcessMessage and Reload terminate with a `TurnComplete`; drain
        // (answering any blocking round-trip) until it arrives or we time out.
        loop {
            match event_rx.recv_timeout(deadline) {
                Ok(AgentEvent::TurnComplete) => {
                    events.push(AgentEvent::TurnComplete);
                    completed = true;
                    break;
                }
                Ok(AgentEvent::PermissionRequest {
                    key,
                    preview,
                    reply_tx,
                }) => {
                    let _ = reply_tx.send(reply_for(perm));
                    events.push(AgentEvent::PermissionRequest {
                        key,
                        preview,
                        reply_tx: mpsc::channel().0,
                    });
                }
                Ok(AgentEvent::AskUser { prompt, reply_tx }) => {
                    let _ = reply_tx.send(String::new());
                    events.push(AgentEvent::AskUser {
                        prompt,
                        reply_tx: mpsc::channel().0,
                    });
                }
                Ok(AgentEvent::Confirm { prompt, reply_tx }) => {
                    let _ = reply_tx.send(true);
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
                Err(_) => break, // timeout — move on to the next step
            }
        }
    }

    let _ = cmd_tx.send(AgentCommand::Shutdown);
    let _ = handle.join();
    let requests = recorder.lock().unwrap().clone();
    RunOutcome {
        events,
        requests,
        elapsed: start.elapsed(),
        completed,
    }
}

/// Cooperative-cancellation variant. Drives one turn, but the instant the first
/// `ToolStarted` event arrives it trips the agent's real `CancelToken` (the same
/// mechanism the TUI uses — a shared atomic flag, ADR 0016) and, belt-and-braces,
/// also sends `AgentCommand::Cancel`. Returns as soon as `TurnComplete` arrives
/// or a hard 3s wall-clock deadline elapses, whichever comes first.
///
/// Answers permission round-trips per `perm` so the long command actually starts.
pub fn run_turn_with_cancel(
    root: PathBuf,
    provider: Arc<dyn Provider>,
    recorder: Recorder,
    msg: &str,
    perm: PermPolicy,
) -> RunOutcome {
    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>();
    let agent = AgentLoop::new(provider, "test-model", 4096, 0.0, root);
    // Grab the real cancel token BEFORE moving the agent into its thread. The run
    // loop blocks on `process_turn` for the duration of the turn, so a `Cancel`
    // command on `cmd_tx` cannot be serviced mid-turn — flipping the shared token
    // is the only in-turn cancellation path (the TUI holds such a clone).
    let cancel = agent.cancel_token();
    let handle = thread::spawn(move || agent.run(cmd_rx, event_tx));
    cmd_tx
        .send(AgentCommand::ProcessMessage(msg.into()))
        .unwrap();

    let start = Instant::now();
    let hard_deadline = Duration::from_secs(3);
    let mut events = Vec::new();
    let mut cancelled = false;
    loop {
        if start.elapsed() >= hard_deadline {
            break;
        }
        let remaining = hard_deadline.saturating_sub(start.elapsed());
        match event_rx.recv_timeout(remaining) {
            Ok(AgentEvent::TurnComplete) => {
                events.push(AgentEvent::TurnComplete);
                break;
            }
            Ok(ev @ AgentEvent::ToolStarted { .. }) => {
                events.push(ev);
                if !cancelled {
                    cancelled = true;
                    cancel.cancel();
                    let _ = cmd_tx.send(AgentCommand::Cancel);
                }
            }
            Ok(AgentEvent::PermissionRequest {
                key,
                preview,
                reply_tx,
            }) => {
                let _ = reply_tx.send(reply_for(perm));
                events.push(AgentEvent::PermissionRequest {
                    key,
                    preview,
                    reply_tx: mpsc::channel().0,
                });
            }
            Ok(other) => events.push(other),
            Err(_) => break, // timeout — return what we have
        }
    }
    let elapsed = start.elapsed();
    let _ = cmd_tx.send(AgentCommand::Shutdown);
    // Do not block the test on join(): if the tool is uncancellable it may still
    // be running. Detach and let it finish on its own.
    drop(handle);
    let requests = recorder.lock().unwrap().clone();
    RunOutcome {
        events,
        requests,
        elapsed,
        // This path intentionally may not run to completion (it trips cancel
        // mid-turn); leave `completed` false. Its callers assert on cancellation
        // signals, not on `completed`.
        completed: false,
    }
}
