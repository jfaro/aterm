//! Pure server state: no PTYs, threads or I/O, so it is trivially testable.

use std::collections::BTreeMap;

use crate::detect::Detection;
use crate::protocol::PaneId;
use crate::protocol::PaneInfo;

#[derive(Default)]
pub struct AppState {
    last_id: PaneId,
    panes: BTreeMap<PaneId, PaneInfo>,
}

impl AppState {
    pub fn allocate_id(&mut self) -> PaneId {
        self.last_id += 1;
        self.last_id
    }

    pub fn insert(&mut self, pane: PaneInfo) {
        self.panes.insert(pane.id, pane);
    }

    pub fn remove(&mut self, id: PaneId) -> Option<PaneInfo> {
        self.panes.remove(&id)
    }

    pub fn get(&self, id: PaneId) -> Option<&PaneInfo> {
        self.panes.get(&id)
    }

    pub fn get_mut(&mut self, id: PaneId) -> Option<&mut PaneInfo> {
        self.panes.get_mut(&id)
    }

    pub fn list(&self) -> Vec<PaneInfo> {
        self.panes.values().cloned().collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::AgentState;

    fn pane(id: PaneId) -> PaneInfo {
        PaneInfo {
            id,
            name: format!("p{id}"),
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
}
