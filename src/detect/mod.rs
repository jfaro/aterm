//! Agent detection: works out which agent runs in a pane and what it is doing.
//!
//! Detection only ever sees a [`Snapshot`] of the screen, never the terminal
//! parser itself, so rules can be tested against plain-text fixtures.

mod manifest;
pub mod process;

use serde::Deserialize;
use serde::Serialize;

pub use manifest::Detectors;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    /// Agent finished its turn and is waiting at its prompt.
    Idle,

    /// Agent is actively working.
    Working,

    /// Agent needs a human decision (permission prompt, confirmation, …).
    Blocked,

    /// Plain shell or unrecognized program.
    Unknown,
}

/// What detection rules are allowed to look at.
pub struct Snapshot<'a> {
    /// Window title set by the program via OSC 0/2.
    pub title: &'a str,

    /// Visible screen rows, top to bottom, with trailing whitespace trimmed.
    pub lines: &'a [String],
}

/// Result of evaluating a manifest against a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub state: AgentState,
    pub rule: Option<String>,
}
