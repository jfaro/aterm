//! Client side of the socket API, shared by the CLI and the TUI.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::paths;
use crate::protocol::Event;
use crate::protocol::Request;
use crate::protocol::Response;
use crate::protocol::ServerMessage;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>;

pub struct Connection {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Pending,
    next_id: AtomicU64,
}

impl Connection {
    /// Connects to a running server. Events arrive on the returned receiver
    /// once `events.subscribe` has been called; it closes when the server goes away.
    pub async fn connect() -> Result<(Arc<Self>, mpsc::UnboundedReceiver<Event>)> {
        let socket = paths::socket_path();
        let stream = UnixStream::connect(&socket)
            .await
            .with_context(|| format!("no server running on {}", socket.display()))?;
        let (read_half, write_half) = stream.into_split();
        let pending: Pending = Arc::default();
        let (events_tx, events_rx) = mpsc::unbounded_channel();

        let reader_pending = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut lines = BufReader::new(read_half).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str(&line) {
                    Ok(ServerMessage::Response(response)) => {
                        if let Some(waiter) = reader_pending.lock().unwrap().remove(&response.id) {
                            let _ = waiter.send(response);
                        }
                    }
                    Ok(ServerMessage::Event(event)) => {
                        let _ = events_tx.send(event);
                    }
                    Err(err) => eprintln!("aterm: ignoring malformed server message: {err}"),
                }
            }
            // Dropping the waiters fails every in-flight call.
            reader_pending.lock().unwrap().clear();
        });

        let connection = Self {
            writer: tokio::sync::Mutex::new(write_half),
            pending,
            next_id: AtomicU64::new(1),
        };
        Ok((Arc::new(connection), events_rx))
    }

    /// Connects, starting a background server first if none is running.
    pub async fn connect_or_spawn() -> Result<(Arc<Self>, mpsc::UnboundedReceiver<Event>)> {
        if let Ok(connection) = Self::connect().await {
            return Ok(connection);
        }
        spawn_server_daemon()?;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(30)).await;
            if let Ok(connection) = Self::connect().await {
                return Ok(connection);
            }
        }
        bail!("server did not start; see {}", paths::log_path().display())
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let request = Request {
            id,
            method: method.to_owned(),
            params,
        };
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        if let Err(err) = self.writer.lock().await.write_all(line.as_bytes()).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(err).context("lost connection to server");
        }

        let response = rx.await.map_err(|_| anyhow!("lost connection to server"))?;
        match response.error {
            Some(error) => Err(anyhow!(error)),
            None => Ok(response.result.unwrap_or(Value::Null)),
        }
    }

    pub async fn call_as<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).with_context(|| format!("unexpected response to {method}"))
    }
}

/// Starts `aterm server` detached from this terminal, logging to the log file.
fn spawn_server_daemon() -> Result<()> {
    let log_path = paths::log_path();
    if let Some(dir) = log_path.parent() {
        paths::ensure_private_dir(dir)?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("server")
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe, as required between fork and exec.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn().context("failed to start server")?;
    Ok(())
}
