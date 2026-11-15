//! Pure server state: no PTYs, threads or I/O, so it is trivially testable.

use std::collections::BTreeMap;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;

use crate::detect::Detection;
use crate::protocol::FolderId;
use crate::protocol::FolderInfo;
use crate::protocol::PaneId;
use crate::protocol::PaneInfo;

/// Every pane id is in exactly one group: `ungrouped`, or the `panes` of the
/// folder its [`PaneInfo::folder`] names.
#[derive(Default)]
pub struct AppState {
    /// The most recently allocated pane id.
    last_id: PaneId,

    /// The most recently allocated folder id.
    last_folder_id: FolderId,

    /// Every live pane, keyed by id.
    panes: BTreeMap<PaneId, PaneInfo>,

    /// Top-level panes in sidebar order, shown before every folder.
    ungrouped: Vec<PaneId>,

    /// Folders in sidebar order.
    folders: Vec<Folder>,
}

struct Folder {
    info: FolderInfo,

    /// The folder's panes in sidebar order.
    panes: Vec<PaneId>,
}

impl AppState {
    pub fn allocate_id(&mut self) -> PaneId {
        self.last_id += 1;
        self.last_id
    }

    /// Adds a pane at the end of the group its `folder` names, or of the top
    /// level if there is no such folder. Re-inserting a pane keeps its place.
    pub fn insert(&mut self, mut pane: PaneInfo) {
        if let Some(old) = self.panes.get(&pane.id) {
            pane.folder = old.folder;
        } else {
            if !self.has_folder(pane.folder) {
                pane.folder = None;
            }
            self.group_mut(pane.folder)
                .expect("folder was just checked")
                .push(pane.id);
        }
        self.panes.insert(pane.id, pane);
    }

    pub fn remove(&mut self, id: PaneId) -> Option<PaneInfo> {
        let removed = self.panes.remove(&id)?;
        if let Some(group) = self.group_mut(removed.folder) {
            group.retain(|&other| other != id);
        }
        Some(removed)
    }

    /// Moves a pane to `index` within the group `folder` names, or within its
    /// current group when `folder` is `None`; `Some(None)` is the top level.
    ///
    /// Note: an `index` past the end moves the pane to the group's last position.
    /// Returns the group and index the pane ended up at, or `None` if there is
    /// no such pane or folder.
    pub fn move_to(
        &mut self,
        id: PaneId,
        folder: Option<Option<FolderId>>,
        index: usize,
    ) -> Option<(Option<FolderId>, usize)> {
        let from = self.panes.get(&id)?.folder;
        let to = folder.unwrap_or(from);
        if !self.has_folder(to) {
            return None;
        }

        self.group_mut(from)?.retain(|&other| other != id);
        let group = self.group_mut(to).expect("folder was just checked");
        let index = index.min(group.len());
        group.insert(index, id);
        self.panes.get_mut(&id)?.folder = to;

        Some((to, index))
    }

    pub fn get(&self, id: PaneId) -> Option<&PaneInfo> {
        self.panes.get(&id)
    }

    pub fn get_mut(&mut self, id: PaneId) -> Option<&mut PaneInfo> {
        self.panes.get_mut(&id)
    }

    /// Every pane in sidebar order: the top level, then each folder in turn.
    pub fn list(&self) -> Vec<PaneInfo> {
        self.ungrouped
            .iter()
            .chain(self.folders.iter().flat_map(|folder| &folder.panes))
            .filter_map(|id| self.panes.get(id))
            .cloned()
            .collect()
    }

    /// Whether `folder` names an existing folder; `None`, the top level, always exists.
    pub fn has_folder(&self, folder: Option<FolderId>) -> bool {
        folder.is_none_or(|id| self.folders.iter().any(|f| f.info.id == id))
    }

    /// Every folder in sidebar order.
    pub fn folders(&self) -> Vec<FolderInfo> {
        self.folders.iter().map(|f| f.info.clone()).collect()
    }

    /// Adds an empty folder after the existing ones.
    pub fn create_folder(&mut self, name: &str) -> Result<FolderInfo> {
        let name = folder_name(name)?;
        self.last_folder_id += 1;
        let info = FolderInfo {
            id: self.last_folder_id,
            name,
        };
        self.folders.push(Folder {
            info: info.clone(),
            panes: Vec::new(),
        });
        Ok(info)
    }

    pub fn rename_folder(&mut self, id: FolderId, name: &str) -> Result<FolderInfo> {
        let name = folder_name(name)?;
        let folder = self.folder_mut(id)?;
        folder.info.name = name;
        Ok(folder.info.clone())
    }

    /// Deletes a folder, moving its panes to the end of the top level.
    /// Returns the moved panes.
    pub fn delete_folder(&mut self, id: FolderId) -> Result<Vec<PaneId>> {
        let index = self
            .folders
            .iter()
            .position(|f| f.info.id == id)
            .ok_or_else(|| no_such_folder(id))?;
        let moved = self.folders.remove(index).panes;
        for pane in &moved {
            if let Some(info) = self.panes.get_mut(pane) {
                info.folder = None;
            }
        }
        self.ungrouped.extend(&moved);
        Ok(moved)
    }

    fn folder_mut(&mut self, id: FolderId) -> Result<&mut Folder> {
        self.folders
            .iter_mut()
            .find(|f| f.info.id == id)
            .ok_or_else(|| no_such_folder(id))
    }

    fn group_mut(&mut self, folder: Option<FolderId>) -> Option<&mut Vec<PaneId>> {
        match folder {
            None => Some(&mut self.ungrouped),
            Some(id) => self.folder_mut(id).ok().map(|f| &mut f.panes),
        }
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

pub fn no_such_folder(id: FolderId) -> anyhow::Error {
    anyhow!("no such folder: {id}")
}

/// `name` trimmed, rejecting blank names: folders are always named.
fn folder_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("folder name can't be empty");
    }
    Ok(name.to_owned())
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
            folder: None,
        }
    }

    fn ids(state: &AppState) -> Vec<PaneId> {
        state.list().iter().map(|p| p.id).collect()
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
        assert_eq!(ids(&state), [1, 2, 3]);

        assert_eq!(state.move_to(3, None, 0), Some((None, 0)));
        assert_eq!(ids(&state), [3, 1, 2]);
        assert_eq!(state.move_to(3, None, 99), Some((None, 2)));
        assert_eq!(ids(&state), [1, 2, 3]);
        assert_eq!(state.move_to(42, None, 0), None);

        state.remove(2);
        assert_eq!(ids(&state), [1, 3]);
        let id = state.allocate_id();
        state.insert(pane(id));
        assert_eq!(ids(&state), [1, 3, 4]);
    }

    #[test]
    fn folders_group_panes_after_the_top_level() {
        let mut state = AppState::default();
        let a = state.create_folder(" a ").unwrap();
        let b = state.create_folder("b").unwrap();
        assert_eq!(a.name, "a");
        for folder in [Some(b.id), None, Some(a.id), Some(99)] {
            let id = state.allocate_id();
            state.insert(PaneInfo { folder, ..pane(id) });
        }
        // Pane 4 named a missing folder, so it lands at the top level.
        assert_eq!(ids(&state), [2, 4, 3, 1]);
        assert_eq!(state.get(4).unwrap().folder, None);

        assert_eq!(state.move_to(2, Some(Some(b.id)), 0), Some((Some(b.id), 0)));
        assert_eq!(ids(&state), [4, 3, 2, 1]);
        assert_eq!(state.get(2).unwrap().folder, Some(b.id));
        assert_eq!(state.move_to(2, Some(Some(99)), 0), None);
        assert_eq!(state.get(2).unwrap().folder, Some(b.id));

        // Moving within a folder stays in it.
        assert_eq!(state.move_to(2, None, 9), Some((Some(b.id), 1)));
        assert_eq!(ids(&state), [4, 3, 1, 2]);

        state.remove(3);
        assert_eq!(ids(&state), [4, 1, 2]);
        assert_eq!(state.delete_folder(b.id).unwrap(), [1, 2]);
        assert_eq!(ids(&state), [4, 1, 2]);
        assert!(state.list().iter().all(|p| p.folder.is_none()));
        assert_eq!(state.folders(), [a]);
        assert!(state.delete_folder(b.id).is_err());
    }

    #[test]
    fn folders_always_have_a_name() {
        let mut state = AppState::default();
        assert!(state.create_folder("  ").is_err());
        let folder = state.create_folder("a").unwrap();
        assert!(state.rename_folder(folder.id, "").is_err());
        assert_eq!(state.rename_folder(folder.id, "b").unwrap().name, "b");
        assert!(state.rename_folder(99, "c").is_err());
        assert_eq!(state.folders()[0].name, "b");

        // Ids are never reused.
        state.delete_folder(folder.id).unwrap();
        assert_ne!(state.create_folder("c").unwrap().id, folder.id);
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
