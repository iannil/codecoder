/// SocketClient — connect to daemon via cc protocol (Unix socket, JSON-line).
///
/// Reads `ServerEvent` lines from the daemon and dispatches them to a callback.
/// Writes `ClientRequest` lines to the daemon.
use crate::daemon::proto::{self, ClientRequest, ServerEvent};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub type EventCallback = Box<dyn Fn(&ServerEvent) + Send + 'static>;

pub struct SocketClient {
    stream: Option<UnixStream>,
    daemon_path: String,
    cb: Arc<Mutex<Option<EventCallback>>>,
    cmd_tx: Sender<ClientRequest>,
    stop_flag: Arc<Mutex<bool>>,
    thread_handle: Option<JoinHandle<()>>,
}

impl SocketClient {
    /// Connect to daemon at the given Unix socket path.
    pub fn connect(daemon_path: &Path) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(daemon_path)?;
        stream.set_read_timeout(Some(std::time::Duration::from_millis(500)))?;
        let (cmd_tx, _cmd_rx) = mpsc::channel();
        Ok(Self {
            stream: Some(stream),
            daemon_path: daemon_path.to_string_lossy().into_owned(),
            cb: Arc::new(Mutex::new(None)),
            cmd_tx,
            stop_flag: Arc::new(Mutex::new(false)),
            thread_handle: None,
        })
    }

    /// For testing: wrap an already-connected stream (pair).
    #[doc(hidden)]
    pub fn connect_unchecked(stream: UnixStream) -> Self {
        let (cmd_tx, _cmd_rx) = mpsc::channel();
        Self {
            stream: Some(stream),
            daemon_path: String::new(),
            cb: Arc::new(Mutex::new(None)),
            cmd_tx,
            stop_flag: Arc::new(Mutex::new(false)),
            thread_handle: None,
        }
    }

    pub fn set_event_callback(&mut self, cb: EventCallback) {
        *self.cb.lock().unwrap() = Some(cb);
    }

    /// Write a `ClientRequest` as a JSON line to the daemon socket.
    pub fn send(&self, req: ClientRequest) -> anyhow::Result<()> {
        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("stream closed"))?;
        let mut writer = stream.try_clone()?;
        let json = serde_json::to_string(&req)?;
        writeln!(writer, "{json}")?;
        writer.flush()?;
        Ok(())
    }

    /// Start the receive loop in a background thread.
    /// Reads `ServerEvent` lines from the daemon and dispatches to the callback.
    pub fn start(&mut self) {
        let mut stream = self.stream.take().expect("SocketClient already started");
        let cb = self.cb.clone();
        let stop = self.stop_flag.clone();

        let handle = std::thread::spawn(move || {
            let mut reader = BufReader::new(&mut stream);
            loop {
                if *stop.lock().unwrap() {
                    break;
                }
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        // EOF (daemon closed)
                        if let Some(ref cb) = *cb.lock().unwrap() {
                            cb(&ServerEvent::Notice {
                                text: "daemon disconnected".into(),
                            });
                        }
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<ServerEvent>(trimmed) {
                            Ok(ev) => {
                                if let Some(ref cb) = *cb.lock().unwrap() {
                                    cb(&ev);
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "cc-web: failed to parse ServerEvent: {e} (line: {trimmed:?})"
                                );
                            }
                        }
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue;
                    }
                    Err(e) => {
                        eprintln!("cc-web: read error: {e}");
                        if let Some(ref cb) = *cb.lock().unwrap() {
                            cb(&ServerEvent::Notice {
                                text: format!("read error: {e}"),
                            });
                        }
                        break;
                    }
                }
            }
        });
        self.thread_handle = Some(handle);
    }

    pub fn stop(&mut self) {
        *self.stop_flag.lock().unwrap() = true;
    }
}

impl Drop for SocketClient {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::proto::{ClientRequest, ServerEvent};
    use std::sync::mpsc;

    #[test]
    fn socket_client_send_and_recv_event() {
        let (client, mut daemon) = UnixStream::pair().unwrap();
        let mut daemon_w = daemon.try_clone().unwrap();

        // Spawn a fake daemon thread
        std::thread::spawn(move || {
            // Read a request
            let mut daemon_reader = std::io::BufReader::new(&mut daemon);
            let req = proto::read_request(&mut daemon_reader).unwrap();
            assert!(matches!(req, Some(ClientRequest::ListSessions)));
            // Respond with an event
            proto::write_event(
                &mut daemon_w,
                &ServerEvent::Sessions {
                    ids: vec!["s1".into()],
                },
            )
            .unwrap();
        });

        let (tx, rx) = mpsc::channel();
        let cb: EventCallback = Box::new(move |ev| {
            tx.send(ev.clone()).unwrap();
        });

        let mut sc = SocketClient::connect_unchecked(client);
        sc.set_event_callback(cb);
        sc.send(ClientRequest::ListSessions).unwrap();
        sc.start();

        let received = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        match received {
            ServerEvent::Sessions { ids } => assert_eq!(ids, vec!["s1"]),
            other => panic!("expected Sessions, got {other:?}"),
        }
        sc.stop();
    }
}