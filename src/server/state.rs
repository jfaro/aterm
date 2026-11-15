//! Pure server state: no PTYs, threads or I/O, so it is trivially testable.

use std::collections::BTreeMap;

use crate::detect::Detection;
use crate::protocol::PaneId;
use crate::protocol::PaneInfo;

#[derive(Default)]
pub struct AppState {
    /// The most recently allocated pane id.
    last_id: PaneId,

    /// Every live pane, keyed by id.
    panes: BTreeMap<PaneId, PaneInfo>,

    /// The sidebar order, holding exactly the ids in `panes`.
    order: Vec<PaneId>,
}

impl AppState {
    pub fn allocate_id(&mut self) -> PaneId {
        self.last_id += 1;
        self.last_id
    }

    pub fn insert(&mut self, pane: PaneInfo) {
        let id = pane.id;
        if self.panes.insert(id, pane).is_none() {
            self.order.push(id);
        }
    }

    pub fn remove(&mut self, id: PaneId) -> Option<PaneInfo> {
        let removed = self.panes.remove(&id)?;
        self.order.retain(|&other| other != id);
        Some(removed)
    }

    /// Moves a pane to `index` in the sidebar order.
    ///
    /// Note: an `index` past the end moves the pane to the last position.
    /// Returns the index the pane ended up at, or `None` if there is no such pane.
    pub fn move_to(&mut self, id: PaneId, index: usize) -> Option<usize> {
        // Take the pane out of its current position.
        let from = self.order.iter().position(|&other| other == id)?;
        self.order.remove(from);

        // Put it back at the clamped target position.
        let to = index.min(self.order.len());
        self.order.insert(to, id);

        Some(to)
    }

    pub fn get(&self, id: PaneId) -> Option<&PaneInfo> {
        self.panes.get(&id)
    }

    pub fn get_mut(&mut self, id: PaneId) -> Option<&mut PaneInfo> {
        self.panes.get_mut(&id)
    }

    pub fn list(&self) -> Vec<PaneInfo> {
        self.order
            .iter()
            .filter_map(|id| self.panes.get(id))
            .cloned()
            .collect()
    }

    /// Gives a pane a user-chosen name, or with `None` hands naming back to
    /// [`auto_label`] using the pane's current window `title`.
    pub fn rename(&mut self, id: PaneId, name: Option<String>, title: &str) -> Option<&PaneInfo> {
        let pane = self.panes.get_mut(&id)?;
        match name {
            Some(name) => {
                pane.name = name;
                pane.auto_named = false;
            }
            None => {
                pane.name = auto_label(title, pane.agent.as_deref(), &pane.command);
                pane.auto_named = true;
            }
        }
        Some(pane)
    }

    /// Re-derives an auto-named pane's name from its window `title`. Returns
    /// true when the name changed, i.e. when observers should be notified.
    pub fn refresh_auto_name(&mut self, id: PaneId, title: &str) -> bool {
        let Some(pane) = self.panes.get_mut(&id).filter(|pane| pane.auto_named) else {
            return false;
        };
        let label = auto_label(title, pane.agent.as_deref(), &pane.command);
        if pane.name == label {
            return false;
        }
        pane.name = label;
        true
    }

    /// Records a detection result observed at `now_ms`. Returns true when the
    /// agent or its status changed, i.e. when observers should be notified.
    pub fn apply_detection(
        &mut self,
        id: PaneId,
        agent: Option<String>,
        detection: Detection,
        now_ms: u64,
    ) -> bool {
        let Some(pane) = self.panes.get_mut(&id) else {
            return false;
        };
        let changed = pane.agent != agent || pane.status != detection.state;
        if changed {
            pane.status_since = now_ms;
        }
        pane.agent = agent;
        pane.status = detection.state;
        pane.rule = detection.rule;
        changed
    }
}

/// Name for a pane the user hasn't named: its window title, else the agent
/// running in it, else the program it was started with.
pub fn auto_label(title: &str, agent: Option<&str>, command: &str) -> String {
    let title = clean_title(title);
    if !title.is_empty() {
        return title;
    }
    agent.map_or_else(|| program_name(command), str::to_owned)
}

/// Basename of the program in a command line, e.g. `claude` for
/// `/opt/bin/claude --resume`.
pub fn program_name(command: &str) -> String {
    let program = command.split_whitespace().next().unwrap_or("shell");
    program.rsplit('/').next().unwrap_or(program).to_owned()
}

/// Strips control characters and leading status glyphs (spinners, `✳`, …),
/// which agents animate and which would otherwise churn the name.
fn clean_title(title: &str) -> String {
    let title: String = title.chars().filter(|c| !c.is_control()).collect();
    title
        .trim_start_matches(|c: char| c.is_whitespace() || (!c.is_ascii() && !c.is_alphanumeric()))
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::AgentState;

    fn pane(id: PaneId) -> PaneInfo {
        PaneInfo {
            id,
            name: format!("p{id}"),
            auto_named: true,
            command: "sh".into(),
            cwd: "/".into(),
            rows: 24,
            cols: 80,
            agent: None,
            status: AgentState::Unknown,
            status_since: 0,
            rule: None,
        }
    }

    #[test]
    fn ids_are_never_reused() {
        let mut state = AppState::default();
        let first = state.allocate_id();
        state.insert(pane(first));
        state.remove(first);
        assert_ne!(state.allocate_id(), first);
    }

    #[test]
    fn panes_list_in_order_and_can_be_moved() {
        let mut state = AppState::default();
        for _ in 0..3 {
            let id = state.allocate_id();
            state.insert(pane(id));
        }
        let ids = |state: &AppState| state.list().iter().map(|p| p.id).collect::<Vec<_>>();
        assert_eq!(ids(&state), [1, 2, 3]);

        assert_eq!(state.move_to(3, 0), Some(0));
        assert_eq!(ids(&state), [3, 1, 2]);
        assert_eq!(state.move_to(3, 99), Some(2));
        assert_eq!(ids(&state), [1, 2, 3]);
        assert_eq!(state.move_to(42, 0), None);

        state.remove(2);
        assert_eq!(ids(&state), [1, 3]);
        let id = state.allocate_id();
        state.insert(pane(id));
        assert_eq!(ids(&state), [1, 3, 4]);
    }

    #[test]
    fn detection_reports_only_real_changes() {
        let mut state = AppState::default();
        let id = state.allocate_id();
        state.insert(pane(id));
        let working = || Detection {
            state: AgentState::Working,
            rule: Some("spinner".into()),
        };

        assert!(state.apply_detection(id, Some("claude".into()), working(), 10));
        assert!(!state.apply_detection(id, Some("claude".into()), working(), 20));
        assert_eq!(state.get(id).unwrap().status_since, 10);
        assert!(state.apply_detection(
            id,
            None,
            Detection {
                state: AgentState::Unknown,
                rule: None
            },
            30
        ));
        assert!(!state.apply_detection(999, None, working(), 40));
        let pane = state.get(id).unwrap();
        assert_eq!(pane.status, AgentState::Unknown);
        assert_eq!(pane.status_since, 30);
    }

    #[test]
    fn auto_label_prefers_title_then_agent_then_program() {
        assert_eq!(
            auto_label("⠋ Fix the tests ", Some("claude"), "claude"),
            "Fix the tests"
        );
        assert_eq!(auto_label("~/src\x07", None, "shell"), "~/src");
        assert_eq!(auto_label(" ✳ ", Some("codex"), "shell"), "codex");
        assert_eq!(auto_label("", None, "/opt/bin/claude --resume"), "claude");
        assert_eq!(auto_label("", None, "shell"), "shell");
    }

    #[test]
    fn auto_name_follows_title_until_renamed() {
        let mut state = AppState::default();
        let id = state.allocate_id();
        state.insert(pane(id));

        assert!(state.refresh_auto_name(id, "build"));
        assert!(!state.refresh_auto_name(id, "build"));
        assert_eq!(state.get(id).unwrap().name, "build");

        state.rename(id, Some("mine".into()), "build");
        assert!(!state.refresh_auto_name(id, "test"));
        assert_eq!(state.get(id).unwrap().name, "mine");

        let pane = state.rename(id, None, "test").unwrap();
        assert!(pane.auto_named);
        assert_eq!(pane.name, "test");
        assert!(!state.refresh_auto_name(999, "x"));
    }
}
