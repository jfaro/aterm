//! The aterm server: owns every pane and serves the JSON socket API.

mod pane;
mod state;

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::UnixListener;
use tokio::net::UnixStream;
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

use crate::detect::AgentState;
use crate::detect::Detection;
use crate::detect::Detectors;
use crate::detect::Snapshot;
use crate::detect::process;
use crate::keys;
use crate::paths;
use crate::protocol::Event;
use crate::protocol::PaneId;
use crate::protocol::PaneInfo;
use crate::protocol::Request;
use crate::protocol::Response;
use crate::protocol::ServerMessage;
use crate::protocol::events;
use crate::protocol::unix_millis;
use pane::PaneRuntime;
use pane::SpawnSpec;
use pane::Terminal;
use state::AppState;

const OUTPUT_TICK: Duration = Duration::from_millis(16);
/// Detection runs every this many output ticks (~250ms).
const DETECT_EVERY: u64 = 15;
/// Detection is forced even without new output every this many ticks (~2s),
/// so agents started in a quiet pane are still noticed.
const FORCE_DETECT_EVERY: u64 = 120;
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

pub async fn run() -> Result<()> {
    let socket = paths::socket_path();
    prepare_socket(&socket)?;
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    eprintln!("aterm server listening on {}", socket.display());

    let server = Arc::new(Server::new(socket.clone()));
    let monitor_server = Arc::clone(&server);
    std::thread::Builder::new()
        .name("monitor".into())
        .spawn(move || monitor(&monitor_server))?;

    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(handle_client(Arc::clone(&server), stream));
                }
                Err(err) => eprintln!("accept failed: {err}"),
            },
            _ = server.shutdown.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    // Let in-flight responses (e.g. to `server.stop`) reach their clients.
    tokio::time::sleep(Duration::from_millis(50)).await;
    server.stopping.store(true, Ordering::Release);
    server.inner.lock().unwrap().runtimes.clear();
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

/// Ensures the socket directory exists privately and no live server owns the socket.
fn prepare_socket(socket: &Path) -> Result<()> {
    if let Some(dir) = socket.parent() {
        paths::ensure_private_dir(dir)?;
    }
    if socket.exists() {
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            bail!("a server is already running on {}", socket.display());
        }
        std::fs::remove_file(socket).context("failed to remove stale socket")?;
    }
    Ok(())
}

struct Server {
    inner: Mutex<Inner>,
    events: broadcast::Sender<Event>,
    detectors: Detectors,
    socket: PathBuf,
    shutdown: Notify,
    stopping: AtomicBool,
}

#[derive(Default)]
struct Inner {
    state: AppState,
    runtimes: HashMap<PaneId, PaneRuntime>,
}

impl Server {
    fn new(socket: PathBuf) -> Self {
        Self {
            inner: Mutex::default(),
            events: broadcast::channel(1024).0,
            detectors: Detectors::builtin(),
            socket,
            shutdown: Notify::new(),
            stopping: AtomicBool::new(false),
        }
    }

    fn emit(&self, event: &str, data: Value) {
        // An error only means nobody is subscribed.
        let _ = self.events.send(Event {
            event: event.to_owned(),
            data,
        });
    }

    fn pane_info(&self, id: PaneId) -> Result<PaneInfo> {
        let inner = self.inner.lock().unwrap();
        inner.state.get(id).cloned().ok_or_else(|| no_such_pane(id))
    }

    fn terminal(&self, id: PaneId) -> Result<Arc<Terminal>> {
        let inner = self.inner.lock().unwrap();
        let runtime = inner.runtimes.get(&id).ok_or_else(|| no_such_pane(id))?;
        Ok(Arc::clone(&runtime.terminal))
    }

    fn create_pane(&self, params: CreateParams) -> Result<PaneInfo> {
        let command = params.command.filter(|c| !c.trim().is_empty());
        let cwd = params
            .cwd
            .map(PathBuf::from)
            .filter(|dir| dir.is_dir())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/"));
        let rows = params.rows.unwrap_or(DEFAULT_ROWS).max(2);
        let cols = params.cols.unwrap_or(DEFAULT_COLS).max(10);

        let mut inner = self.inner.lock().unwrap();
        let id = inner.state.allocate_id();
        let runtime = PaneRuntime::spawn(SpawnSpec {
            id,
            command: command.as_deref(),
            cwd: &cwd,
            rows,
            cols,
            socket: self.socket.clone(),
        })?;
        let command = command.unwrap_or_else(|| "shell".into());
        let name = params.name.filter(|name| !name.trim().is_empty());
        let info = PaneInfo {
            id,
            auto_named: name.is_none(),
            name: name.unwrap_or_else(|| state::program_name(&command)),
            command,
            cwd: cwd.display().to_string(),
            rows,
            cols,
            agent: None,
            status: AgentState::Unknown,
            status_since: unix_millis(SystemTime::now()),
            rule: None,
        };
        inner.state.insert(info.clone());
        inner.runtimes.insert(id, runtime);
        drop(inner);

        self.emit(events::PANE_CREATED, json!({ "pane": id, "info": info }));
        Ok(info)
    }

    fn close_pane(&self, id: PaneId) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.state.remove(id).ok_or_else(|| no_such_pane(id))?;
        let runtime = inner.runtimes.remove(&id);
        drop(inner);
        drop(runtime);
        self.emit(events::PANE_CLOSED, json!({ "pane": id }));
        Ok(())
    }

    fn resize_pane(&self, params: ResizeParams) -> Result<PaneInfo> {
        let (rows, cols) = (params.rows.max(2), params.cols.max(10));
        let mut inner = self.inner.lock().unwrap();
        let runtime = inner
            .runtimes
            .get(&params.pane)
            .ok_or_else(|| no_such_pane(params.pane))?;
        runtime.resize(rows, cols)?;
        let info = inner
            .state
            .get_mut(params.pane)
            .ok_or_else(|| no_such_pane(params.pane))?;
        info.rows = rows;
        info.cols = cols;
        let info = info.clone();
        drop(inner);
        self.emit(
            events::PANE_UPDATED,
            json!({ "pane": info.id, "info": info }),
        );
        Ok(info)
    }

    /// Renames a pane; a missing or blank name returns it to automatic naming.
    fn rename_pane(&self, params: RenameParams) -> Result<PaneInfo> {
        let name = params.name.filter(|name| !name.trim().is_empty());
        let mut inner = self.inner.lock().unwrap();
        let title = inner
            .runtimes
            .get(&params.pane)
            .map(|runtime| runtime.terminal.title())
            .unwrap_or_default();
        let info = inner
            .state
            .rename(params.pane, name, &title)
            .ok_or_else(|| no_such_pane(params.pane))?
            .clone();
        drop(inner);
        self.emit(
            events::PANE_UPDATED,
            json!({ "pane": info.id, "info": info }),
        );
        Ok(info)
    }

    fn move_pane(&self, params: MoveParams) -> Result<usize> {
        // Reorder the pane in the shared state.
        let index = self
            .inner
            .lock()
            .unwrap()
            .state
            .move_to(params.pane, params.index)
            .ok_or_else(|| no_such_pane(params.pane))?;

        // Tell clients to refresh their pane lists.
        self.emit(
            events::PANE_MOVED,
            json!({ "pane": params.pane, "index": index }),
        );

        Ok(index)
    }

    /// Writes to a pane off the async runtime: a child that stops reading its
    /// input must not stall the server.
    async fn write_to_pane(&self, id: PaneId, bytes: Vec<u8>) -> Result<()> {
        let writer = {
            let inner = self.inner.lock().unwrap();
            inner
                .runtimes
                .get(&id)
                .ok_or_else(|| no_such_pane(id))?
                .writer()
        };
        tokio::task::spawn_blocking(move || {
            let mut writer = writer.lock().unwrap();
            writer.write_all(&bytes)?;
            writer.flush()
        })
        .await?
        .context("failed to write to pane")
    }

    /// Removes panes whose process has exited.
    fn reap_exited(&self) {
        let exited: Vec<(PaneId, u32)> = {
            let mut inner = self.inner.lock().unwrap();
            let exited: Vec<_> = inner
                .runtimes
                .iter_mut()
                .filter_map(|(id, runtime)| runtime.try_exit_code().map(|code| (*id, code)))
                .collect();
            for (id, _) in &exited {
                inner.state.remove(*id);
                inner.runtimes.remove(id);
            }
            exited
        };
        for (id, code) in exited {
            self.emit(events::PANE_EXITED, json!({ "pane": id, "code": code }));
        }
    }

    /// Re-detects the agent and status of panes with new output (or all panes
    /// when `force`), emitting an event for each change.
    fn detect(&self, last_seq: &mut HashMap<PaneId, u64>, force: bool) {
        let candidates: Vec<(PaneId, Arc<Terminal>, Option<i32>)> = {
            let inner = self.inner.lock().unwrap();
            inner
                .runtimes
                .iter()
                .filter(|(id, runtime)| {
                    force || last_seq.get(*id) != Some(&runtime.terminal.output_seq())
                })
                .map(|(id, runtime)| {
                    (
                        *id,
                        Arc::clone(&runtime.terminal),
                        runtime.foreground_pgid(),
                    )
                })
                .collect()
        };
        if candidates.is_empty() {
            return;
        }

        let groups = process::process_groups();
        for (id, terminal, pgid) in candidates {
            last_seq.insert(id, terminal.output_seq());
            let manifest = pgid
                .and_then(|pgid| groups.get(&pgid))
                .and_then(|names| self.detectors.identify(names.iter().map(String::as_str)));
            let detection = match manifest {
                Some(manifest) => {
                    let (title, lines) = terminal.detection_snapshot();
                    manifest.detect(&Snapshot {
                        title: &title,
                        lines: &lines,
                    })
                }
                None => Detection {
                    state: AgentState::Unknown,
                    rule: None,
                },
            };
            let agent = manifest.map(|m| m.id.clone());
            let title = terminal.title();

            let (status_changed, renamed) = {
                let mut inner = self.inner.lock().unwrap();
                let status_changed = inner.state.apply_detection(
                    id,
                    agent.clone(),
                    detection.clone(),
                    unix_millis(SystemTime::now()),
                );
                let renamed = inner
                    .state
                    .refresh_auto_name(id, &title)
                    .then(|| inner.state.get(id).cloned())
                    .flatten();
                (status_changed, renamed)
            };
            if status_changed {
                self.emit(
                    events::PANE_AGENT_STATUS_CHANGED,
                    json!({ "pane": id, "agent": agent, "status": detection.state }),
                );
            }
            if let Some(info) = renamed {
                self.emit(events::PANE_UPDATED, json!({ "pane": id, "info": info }));
            }
        }
    }
}

/// Background loop: throttled output notifications, detection and reaping.
fn monitor(server: &Server) {
    let mut emitted_seq: HashMap<PaneId, u64> = HashMap::new();
    let mut detected_seq: HashMap<PaneId, u64> = HashMap::new();
    let mut tick: u64 = 0;

    while !server.stopping.load(Ordering::Acquire) {
        std::thread::sleep(OUTPUT_TICK);
        tick += 1;

        let terminals: Vec<(PaneId, Arc<Terminal>)> = {
            let inner = server.inner.lock().unwrap();
            inner
                .runtimes
                .iter()
                .map(|(id, rt)| (*id, Arc::clone(&rt.terminal)))
                .collect()
        };
        for (id, terminal) in &terminals {
            let seq = terminal.output_seq();
            if emitted_seq.insert(*id, seq) != Some(seq) {
                server.emit(events::PANE_OUTPUT_CHANGED, json!({ "pane": id }));
            }
        }
        emitted_seq.retain(|id, _| terminals.iter().any(|(live, _)| live == id));
        detected_seq.retain(|id, _| terminals.iter().any(|(live, _)| live == id));

        if tick.is_multiple_of(DETECT_EVERY) {
            server.reap_exited();
            server.detect(&mut detected_seq, tick.is_multiple_of(FORCE_DETECT_EVERY));
        }
    }
}

async fn handle_client(server: Arc<Server>, stream: UnixStream) {
    let (read_half, mut write_half) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMessage>();
    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let mut line = serde_json::to_string(&message).expect("messages always serialize");
            line.push('\n');
            if write_half.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    let mut tasks = Vec::new();
    let mut lines = BufReader::new(read_half).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                let _ = tx.send(error_response(0, anyhow!("malformed request: {err}")));
                continue;
            }
        };
        let id = request.id;
        match request.method.as_str() {
            "events.subscribe" => {
                tasks.push(tokio::spawn(forward_events(
                    server.events.subscribe(),
                    tx.clone(),
                )));
                let _ = tx.send(ok_response(id, json!({})));
            }
            // Waiting must not block later requests on the same connection.
            "agent.wait" => {
                let (server, tx) = (Arc::clone(&server), tx.clone());
                tasks.push(tokio::spawn(async move {
                    let result = parse(request.params).map(|p| wait_for_status(server, p));
                    let response = match result {
                        Ok(wait) => to_response(id, wait.await),
                        Err(err) => error_response(id, err),
                    };
                    let _ = tx.send(response);
                }));
            }
            // Everything else runs in order, so keystrokes are never reordered.
            _ => {
                let result = dispatch(&server, &request.method, request.params).await;
                let _ = tx.send(to_response(id, result));
            }
        }
    }

    for task in tasks {
        task.abort();
    }
    writer.abort();
}

async fn forward_events(
    mut events: broadcast::Receiver<Event>,
    tx: mpsc::UnboundedSender<ServerMessage>,
) {
    loop {
        match events.recv().await {
            Ok(event) => {
                if tx.send(ServerMessage::Event(event)).is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn dispatch(server: &Server, method: &str, params: Value) -> Result<Value> {
    match method {
        "server.ping" => Ok(json!({ "version": env!("CARGO_PKG_VERSION") })),
        "server.stop" => {
            server.shutdown.notify_one();
            Ok(json!({}))
        }
        "pane.create" => Ok(json!(server.create_pane(parse(params)?)?)),
        "pane.list" => Ok(json!(server.inner.lock().unwrap().state.list())),
        "pane.get" => {
            let p: PaneParams = parse(params)?;
            Ok(json!(server.pane_info(p.pane)?))
        }
        "pane.read" => {
            let p: ReadParams = parse(params)?;
            let terminal = server.terminal(p.pane)?;
            Ok(match p.format {
                ReadFormat::Text => json!({ "text": last_lines(&terminal.text(), p.lines) }),
                ReadFormat::Ansi => json!({ "ansi": String::from_utf8_lossy(&terminal.ansi()) }),
            })
        }
        "pane.send_text" => {
            let p: SendTextParams = parse(params)?;
            server.write_to_pane(p.pane, p.text.into_bytes()).await?;
            Ok(json!({}))
        }
        "pane.send_keys" => {
            let p: SendKeysParams = parse(params)?;
            let application_cursor = server.terminal(p.pane)?.application_cursor();
            let mut bytes = Vec::new();
            for key in &p.keys {
                bytes.extend(
                    keys::encode(key, application_cursor)
                        .ok_or_else(|| anyhow!("unknown key {key:?}"))?,
                );
            }
            server.write_to_pane(p.pane, bytes).await?;
            Ok(json!({}))
        }
        "pane.resize" => Ok(json!(server.resize_pane(parse(params)?)?)),
        "pane.rename" => Ok(json!(server.rename_pane(parse(params)?)?)),
        "pane.move" => Ok(json!({ "index": server.move_pane(parse(params)?)? })),
        "pane.close" => {
            let p: PaneParams = parse(params)?;
            server.close_pane(p.pane)?;
            Ok(json!({}))
        }
        _ => bail!("unknown method {method:?}"),
    }
}

async fn wait_for_status(server: Arc<Server>, params: WaitParams) -> Result<Value> {
    // Subscribe before checking, so a change between the two can't be missed.
    let mut events = server.events.subscribe();
    let current = server.pane_info(params.pane)?;
    if current.status == params.status {
        return Ok(json!(current));
    }

    let pane = json!(params.pane);
    let status = json!(params.status);
    let wait = async {
        loop {
            match events.recv().await {
                Ok(event) if event.data["pane"] == pane => match event.event.as_str() {
                    events::PANE_AGENT_STATUS_CHANGED if event.data["status"] == status => {
                        return server.pane_info(params.pane).map(|info| json!(info));
                    }
                    events::PANE_EXITED | events::PANE_CLOSED => {
                        bail!("pane {} went away while waiting", params.pane)
                    }
                    _ => {}
                },
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let info = server.pane_info(params.pane)?;
                    if info.status == params.status {
                        return Ok(json!(info));
                    }
                }
                Err(broadcast::error::RecvError::Closed) => bail!("server is shutting down"),
            }
        }
    };

    match params.timeout_ms {
        Some(ms) => tokio::time::timeout(Duration::from_millis(ms), wait)
            .await
            .map_err(|_| {
                anyhow!(
                    "timed out waiting for pane {} to be {:?}",
                    params.pane,
                    params.status
                )
            })?,
        None => wait.await,
    }
}

#[derive(Deserialize)]
struct PaneParams {
    pane: PaneId,
}

#[derive(Deserialize)]
struct CreateParams {
    command: Option<String>,
    cwd: Option<String>,
    name: Option<String>,
    rows: Option<u16>,
    cols: Option<u16>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum ReadFormat {
    #[default]
    Text,
    Ansi,
}

#[derive(Deserialize)]
struct ReadParams {
    pane: PaneId,
    #[serde(default)]
    format: ReadFormat,
    lines: Option<usize>,
}

#[derive(Deserialize)]
struct SendTextParams {
    pane: PaneId,
    text: String,
}

#[derive(Deserialize)]
struct SendKeysParams {
    pane: PaneId,
    keys: Vec<String>,
}

#[derive(Deserialize)]
struct ResizeParams {
    pane: PaneId,
    rows: u16,
    cols: u16,
}

#[derive(Deserialize)]
struct RenameParams {
    pane: PaneId,
    name: Option<String>,
}

#[derive(Deserialize)]
struct MoveParams {
    pane: PaneId,
    index: usize,
}

#[derive(Deserialize)]
struct WaitParams {
    pane: PaneId,
    status: AgentState,
    timeout_ms: Option<u64>,
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).context("invalid params")
}

fn to_response(id: u64, result: Result<Value>) -> ServerMessage {
    match result {
        Ok(value) => ok_response(id, value),
        Err(err) => error_response(id, err),
    }
}

fn ok_response(id: u64, result: Value) -> ServerMessage {
    ServerMessage::Response(Response {
        id,
        result: Some(result),
        error: None,
    })
}

fn error_response(id: u64, err: anyhow::Error) -> ServerMessage {
    ServerMessage::Response(Response {
        id,
        result: None,
        error: Some(format!("{err:#}")),
    })
}

fn no_such_pane(id: PaneId) -> anyhow::Error {
    anyhow!("no such pane: {id}")
}

/// The last `count` lines of `text`, ignoring trailing blank lines.
fn last_lines(text: &str, count: Option<usize>) -> String {
    let Some(count) = count else {
        return text.to_owned();
    };
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_lines_ignores_trailing_blank_lines() {
        assert_eq!(last_lines("a\nb\nc\n\n\n", Some(2)), "b\nc");
        assert_eq!(last_lines("a\nb", Some(10)), "a\nb");
        assert_eq!(last_lines("a\n", None), "a\n");
    }
}
