// L2 — pty smoke (opt-in). Drives the *real* compiled binary under a
// pseudo-terminal, with a scripted provider injected via CODECODER_SCRIPT so no
// network or API key is involved. Gated behind RUN_PTY_SMOKE=1 and `#[ignore]`d
// so it never runs (or fails) in the default `cargo test`. Even an explicit
// `-- --ignored` run early-returns when the flag is unset.
//
// This layer exercises the third observable face indirectly: it confirms the
// full TUI+agent process boots, ingests a user line, renders the scripted
// assistant turn, and exits cleanly on `/exit`. It is deliberately a MINIMAL,
// GATED skeleton — pty timing across terminals is inherently flaky, so the
// contract here is "compiles + runs when opted-in", not "bulletproof".

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// The scripted assistant text we expect to see echoed into the rendered TUI.
const SCRIPT_TEXT: &str = "SCRIPTED_HELLO_L2";

#[test]
#[ignore = "pty smoke: RUN_PTY_SMOKE=1"]
fn tui_boots_and_renders_turn() {
    if std::env::var("RUN_PTY_SMOKE").is_err() {
        return;
    }

    // 1) Throwaway project root + a CODECODER_SCRIPT json holding one
    //    assistant_text turn. The script format is a serialized Vec<Message>
    //    (see src/provider/stub.rs::ScriptFileProvider + src/message.rs serde:
    //    Message = { id, role, items:[{ item:"text", text }] }).
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let script_path = root.join("script.json");
    let script = format!(
        r#"[{{"id":0,"role":"assistant","items":[{{"item":"text","text":"{SCRIPT_TEXT}"}}]}}]"#
    );
    std::fs::write(&script_path, script).expect("write script");

    // 2) Spawn the built binary under a pty. `CARGO_BIN_EXE_codecoder` is set by
    //    Cargo for integration tests and points at the freshly built binary, so
    //    we launch it directly (no `cargo run` recompile inside the pty).
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let exe = env!("CARGO_BIN_EXE_codecoder");
    let mut cmd = CommandBuilder::new(exe);
    cmd.env("CODECODER_ROOT", &root);
    cmd.env("CODECODER_SCRIPT", &script_path);
    // Force the scripted path regardless of the ambient environment.
    cmd.env_remove("CODECODER_API_KEY");

    let mut child = pty.slave.spawn_command(cmd).expect("spawn under pty");
    // Drop the slave so only the child holds it; closing our handle lets the
    // child see EOF cleanly when it exits.
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().expect("pty reader");
    let mut writer = pty.master.take_writer().expect("pty writer");

    // 3) Give the TUI a moment to enter its alternate screen + first render,
    //    then type a user line and submit it with a separate Enter (some terminals
    //    drop a CR batched onto the same write as the text).
    std::thread::sleep(Duration::from_millis(800));
    let _ = writer.write_all(b"hello there");
    let _ = writer.flush();
    std::thread::sleep(Duration::from_millis(300));
    let _ = writer.write_all(b"\r");
    let _ = writer.flush();
    std::thread::sleep(Duration::from_millis(400));

    // Drain output for a bounded window, watching for the scripted text. A reader
    // thread pumps bytes into a channel so the main loop enforces a REAL wall-clock
    // deadline via recv_timeout — portable_pty's reader.read() blocks, so polling it
    // directly would ignore the deadline and hang if the child goes quiet.
    let (byte_tx, byte_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break, // EOF or error: child gone
                Ok(n) => {
                    if byte_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut sent_exit = false;
    while Instant::now() < deadline {
        match byte_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                seen.push_str(&String::from_utf8_lossy(&chunk));
                if seen.contains(SCRIPT_TEXT) && !sent_exit {
                    sent_exit = true;
                    let _ = writer.write_all(b"/exit\r");
                    let _ = writer.flush();
                }
            }
            // Quiet tick: stop early once we've seen the text; otherwise keep
            // waiting until the deadline.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if seen.contains(SCRIPT_TEXT) {
                    break;
                }
            }
            // Reader thread ended (child EOF) — nothing more will arrive.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Always terminate the child — never block on wait(): the TUI may not exit on
    // its own, so send /exit, then kill() unconditionally and reap. This is what
    // makes the test incapable of hanging.
    if !sent_exit {
        let _ = writer.write_all(b"/exit\r");
        let _ = writer.flush();
    }
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        seen.contains(SCRIPT_TEXT),
        "scripted assistant text {SCRIPT_TEXT:?} was not rendered by the TUI; \
         captured output (may be sparse under a pty):\n{seen}"
    );
}
