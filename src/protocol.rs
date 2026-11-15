//! Wire protocol: newline-delimited JSON over a Unix socket.
//!
//! Clients send [`Request`]s. The server answers each with a [`Response`]
//! carrying the same `id`, and pushes [`Event`]s to clients that called
//! `events.subscribe`.

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::detect::AgentState;

pub type PaneId = u32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event: String,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessage {
    Response(Response),
    Event(Event),
}

/// Runtime facts about a pane, shared by the server and every client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneInfo {
    /// Unique for the server's lifetime; never reused.
    pub id: PaneId,

    /// Display name, set by the user or derived from the command.
    pub name: String,

    /// Command line the pane was started with, or `shell`.
    pub command: String,

    /// Working directory the pane was started in.
    pub cwd: String,

    /// Terminal height in rows.
    pub rows: u16,

    /// Terminal width in columns.
    pub cols: u16,

    /// Manifest id of the agent in the foreground, if one was identified.
    pub agent: Option<String>,

    /// What the agent is doing, or `Unknown` when no agent was identified.
    pub status: AgentState,

    /// When `status` (or `agent`) last changed, in Unix milliseconds.
    pub status_since: u64,

    /// Id of the detection rule that produced `status`, for debugging manifests.
    pub rule: Option<String>,
}

/// Milliseconds since the Unix epoch, saturating at zero for pre-epoch clocks.
pub fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

pub mod events {
    pub const PANE_CREATED: &str = "pane.created";
    pub const PANE_UPDATED: &str = "pane.updated";
    pub const PANE_OUTPUT_CHANGED: &str = "pane.output_changed";
    pub const PANE_AGENT_STATUS_CHANGED: &str = "pane.agent_status_changed";
    pub const PANE_EXITED: &str = "pane.exited";
    pub const PANE_CLOSED: &str = "pane.closed";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn server_messages_round_trip() {
        let response = ServerMessage::Response(Response {
            id: 7,
            result: Some(json!({"ok": true})),
            error: None,
        });
        let event = ServerMessage::Event(Event {
            event: events::PANE_CLOSED.into(),
            data: json!({"pane": 3}),
        });

        let response_line = serde_json::to_string(&response).unwrap();
        let event_line = serde_json::to_string(&event).unwrap();

        assert!(matches!(
            serde_json::from_str(&response_line).unwrap(),
            ServerMessage::Response(Response { id: 7, .. })
        ));
        assert!(matches!(
            serde_json::from_str(&event_line).unwrap(),
            ServerMessage::Event(Event { ref event, .. }) if event == events::PANE_CLOSED
        ));
    }

    #[test]
    fn error_response_has_no_result() {
        let line = r#"{"id":1,"error":"no such pane"}"#;
        let ServerMessage::Response(response) = serde_json::from_str(line).unwrap() else {
            panic!("expected response");
        };
        assert_eq!(response.error.as_deref(), Some("no such pane"));
        assert!(response.result.is_none());
    }
}
