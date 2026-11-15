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

    /// Records a detection result. Returns true when the agent or its status
    /// changed, i.e. when observers should be notified.
    pub fn apply_detection(
        &mut self,
        id: PaneId,
        agent: Option<String>,
        detection: Detection,
    ) -> bool {
        let Some(pane) = self.panes.get_mut(&id) else {
            return false;
        };
        let changed = pane.agent != agent || pane.status != detection.state;
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

        assert!(state.apply_detection(id, Some("claude".into()), working()));
        assert!(!state.apply_detection(id, Some("claude".into()), working()));
        assert!(state.apply_detection(
            id,
            None,
            Detection {
                state: AgentState::Unknown,
                rule: None
            }
        ));
        assert!(!state.apply_detection(999, None, working()));
        assert_eq!(state.get(id).unwrap().status, AgentState::Unknown);
    }
}
