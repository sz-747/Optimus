//! Port of core/Sidebar/Workspace.cs + WorkspaceManager.cs — workspaces (sidebar rows owning
//! split trees plus socket-reported metadata) and their single ordered, never-empty owner.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::domain::ids::{IdAllocator, SurfaceId};
use crate::domain::split_tree::SplitTreeController;

/// Identity of a workspace — one sidebar row, owning one split tree of tabbed terminal panes.
/// Monotonic and never reused within a session, like the Phase-2 ids (KTD6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct WorkspaceId(pub i32);

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "W{}", self.0)
    }
}

/// Git state reported for a workspace via the Phase-4 pipe (`report_git_branch`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GitBranchInfo {
    pub branch: String,
    pub is_dirty: bool,
}

/// Pull-request state reported via the Phase-4 pipe (`report_pr`). Mirrors what crosses the
/// socket-effects boundary (the wire's `url` param is dropped there today; add it here when a
/// row gains click-through).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PullRequestInfo {
    pub number: String,
    pub label: String,
    pub status: String,
    pub branch: Option<String>,
    pub is_stale: bool,
}

/// One workspace (plan Phase 5 U1, ported from macOS `Workspace`): owns its split tree (the
/// Phase-2 controller) plus the sidebar-facing metadata — title, working directory, git branch,
/// PR status, agent status entries, and progress. The metadata is **not computed by the app**:
/// it arrives over the Phase-4 socket (`report_git_branch` / `report_pr` / `report_pwd` /
/// `set-status`) from shell integration and agent hooks, and from engine title events.
///
/// Pure domain, single-threaded by convention (KTD3): all mutation happens on the UI thread.
/// Mutators bump the shared change counter (the port of the C# `Changed` event) so the sidebar
/// can recompute its row snapshot; the view never binds to this object directly
/// (issue-#2586 discipline — see `SidebarProjection`).
pub struct Workspace {
    id: WorkspaceId,
    controller: SplitTreeController,
    changed: Rc<Cell<u64>>,
    title: String,
    custom_title: Option<String>,
    current_directory: Option<String>,
    git_branch: Option<GitBranchInfo>,
    pull_request: Option<PullRequestInfo>,
    progress: Option<String>,
    watch_git_status: bool,
    status_entries: HashMap<String, String>,
}

impl Workspace {
    pub fn new(id: WorkspaceId, controller: SplitTreeController) -> Self {
        Self::with_changed_signal(id, controller, Rc::new(Cell::new(0)))
    }

    /// As [`Self::new`], with a shared change counter so metadata mutations are observable at
    /// the manager (the port of the C# manager re-raising each workspace's `Changed`).
    pub fn with_changed_signal(
        id: WorkspaceId,
        controller: SplitTreeController,
        changed: Rc<Cell<u64>>,
    ) -> Self {
        Self {
            id,
            controller,
            changed,
            title: String::new(),
            custom_title: None,
            current_directory: None,
            git_branch: None,
            pull_request: None,
            progress: None,
            watch_git_status: true,
            status_entries: HashMap::new(),
        }
    }

    pub fn id(&self) -> WorkspaceId {
        self.id
    }

    /// The workspace's split tree (Phase 2). The view plane renders it; the manager routes to it.
    pub fn controller(&self) -> &SplitTreeController {
        &self.controller
    }

    pub fn controller_mut(&mut self) -> &mut SplitTreeController {
        &mut self.controller
    }

    /// Bumped after any metadata mutation (never by controller/tree changes — those have their
    /// own events). The port of the C# `Changed` event.
    pub fn changed_count(&self) -> u64 {
        self.changed.get()
    }

    /// Derived title — the focused surface's last-seen shell title (pushed in by the view plane).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// User-assigned title; when set it wins over [`Self::title`] in the sidebar.
    pub fn custom_title(&self) -> Option<&str> {
        self.custom_title.as_deref()
    }

    /// The title the sidebar shows: custom > derived > the workspace id.
    pub fn display_title(&self) -> String {
        if let Some(custom) = &self.custom_title {
            if !custom.trim().is_empty() {
                return custom.clone();
            }
        }
        if !self.title.trim().is_empty() {
            return self.title.clone();
        }
        self.id.to_string()
    }

    /// Working directory, reported via `report_pwd`.
    pub fn current_directory(&self) -> Option<&str> {
        self.current_directory.as_deref()
    }

    /// Git branch + dirty flag, reported via `report_git_branch`. `None` until first report.
    pub fn git_branch(&self) -> Option<&GitBranchInfo> {
        self.git_branch.as_ref()
    }

    /// PR state, reported via `report_pr`. `None` until first report.
    pub fn pull_request(&self) -> Option<&PullRequestInfo> {
        self.pull_request.as_ref()
    }

    /// Agent progress string (`set-progress`), e.g. "3/5". `None` when idle.
    pub fn progress(&self) -> Option<&str> {
        self.progress.as_deref()
    }

    /// The "watch git status" gate (plan Phase 5 U2): when off, git/PR reports for this
    /// workspace are dropped, letting a user silence a noisy repo without uninstalling shell
    /// integration.
    pub fn watch_git_status(&self) -> bool {
        self.watch_git_status
    }

    pub fn set_watch_git_status(&mut self, watch: bool) {
        self.watch_git_status = watch;
    }

    /// Agent status entries keyed by agent (`set-status` "key:value", e.g. "claude" → "busy").
    pub fn status_entries(&self) -> &HashMap<String, String> {
        &self.status_entries
    }

    pub fn set_title(&mut self, title: &str) {
        if self.title == title {
            return;
        }
        self.title = title.to_string();
        self.raise_changed();
    }

    pub fn set_custom_title(&mut self, custom_title: Option<&str>) {
        let normalized = custom_title
            .filter(|t| !t.trim().is_empty())
            .map(str::to_string);
        if self.custom_title == normalized {
            return;
        }
        self.custom_title = normalized;
        self.raise_changed();
    }

    pub fn report_working_directory(&mut self, path: &str) {
        if self.current_directory.as_deref() == Some(path) {
            return;
        }
        self.current_directory = Some(path.to_string());
        self.raise_changed();
    }

    /// Record a git report; dropped when [`Self::watch_git_status`] is off.
    pub fn report_git_branch(&mut self, info: GitBranchInfo) {
        if !self.watch_git_status || self.git_branch.as_ref() == Some(&info) {
            return;
        }
        self.git_branch = Some(info);
        self.raise_changed();
    }

    /// Record a PR report; dropped when [`Self::watch_git_status`] is off.
    pub fn report_pull_request(&mut self, info: PullRequestInfo) {
        if !self.watch_git_status || self.pull_request.as_ref() == Some(&info) {
            return;
        }
        self.pull_request = Some(info);
        self.raise_changed();
    }

    /// Apply a `set-status` payload. The hook convention is `key:value` (e.g. "claude:busy");
    /// a bare value uses an empty key. An empty value clears the entry.
    pub fn set_status(&mut self, status: &str) {
        let (key, value) = match status.find(':') {
            Some(sep) => (&status[..sep], &status[sep + 1..]),
            None => ("", status),
        };

        if value.is_empty() {
            if self.status_entries.remove(key).is_none() {
                return;
            }
        } else {
            if self.status_entries.get(key).map(String::as_str) == Some(value) {
                return;
            }
            self.status_entries
                .insert(key.to_string(), value.to_string());
        }
        self.raise_changed();
    }

    pub fn set_progress(&mut self, progress: Option<&str>) {
        let normalized = progress.filter(|p| !p.is_empty()).map(str::to_string);
        if self.progress == normalized {
            return;
        }
        self.progress = normalized;
        self.raise_changed();
    }

    fn raise_changed(&self) {
        self.changed.set(self.changed.get() + 1);
    }
}

/// Lifecycle notifications returned by [`WorkspaceManager::close_workspace`] (the port of the
/// C# `WorkspaceCreated` / `WorkspaceClosed` events). `Created` carries the seeded replacement's
/// id (the workspace lives in the list — the host builds its view); `Closed` carries the removed
/// workspace by value so the host can tear down its view and engines.
pub enum WorkspaceEvent {
    Created(WorkspaceId),
    Closed(Workspace),
}

/// The single owner of workspaces (plan Phase 5): an ordered list with one selected, the
/// workspace analog of the surface manager. All workspace controllers share one [`IdAllocator`],
/// so a bare [`SurfaceId`] (e.g. `OPTIMUS_SURFACE_ID` from the Phase-4 pipe) resolves to exactly
/// one workspace via [`Self::find_by_surface`] — that is how surface-keyed `report_*` commands
/// land on the right sidebar row.
///
/// Invariant: never empty (the workspace analog of R6) — closing the last workspace immediately
/// seeds a fresh one, so the window never goes contentless. Pure domain, UI-thread only (KTD3).
///
/// The C# `Changed` event is ported as [`Self::changed_count`]: it bumps when the list or
/// selection changes, and workspace metadata changes bump it too (shared counter).
pub struct WorkspaceManager {
    ids: Rc<RefCell<IdAllocator>>,
    changed: Rc<Cell<u64>>,
    workspaces: Vec<Workspace>,
    next_workspace: i32,
    selected_id: WorkspaceId,
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceManager {
    /// Seed with one workspace, selected — mirrors the controller's seeded first pane.
    pub fn new() -> Self {
        let mut manager = Self {
            ids: Rc::new(RefCell::new(IdAllocator::new())),
            changed: Rc::new(Cell::new(0)),
            workspaces: Vec::new(),
            next_workspace: 0,
            selected_id: WorkspaceId(0),
        };
        let seed = manager.new_workspace_core();
        manager.selected_id = seed;
        manager
    }

    /// The workspaces in sidebar order.
    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    /// The selected workspace's id.
    pub fn selected_id(&self) -> WorkspaceId {
        self.selected_id
    }

    /// The selected workspace (always present — the list is never empty).
    pub fn selected(&self) -> &Workspace {
        match self.workspaces.iter().find(|w| w.id == self.selected_id) {
            Some(workspace) => workspace,
            None => &self.workspaces[0],
        }
    }

    /// Mutable access to the selected workspace.
    pub fn selected_mut(&mut self) -> &mut Workspace {
        let selected_id = self.selected_id;
        let index = self
            .workspaces
            .iter()
            .position(|w| w.id == selected_id)
            .unwrap_or(0);
        &mut self.workspaces[index]
    }

    /// Bumped when the list or selection changes; workspace metadata changes bump it too. The
    /// port of the C# `Changed` event.
    pub fn changed_count(&self) -> u64 {
        self.changed.get()
    }

    /// Create a workspace at the end of the list and select it. The returned id is the port of
    /// the C# `WorkspaceCreated` event — the host builds the new workspace's view from it.
    pub fn new_workspace(&mut self) -> WorkspaceId {
        let id = self.new_workspace_core();
        self.selected_id = id;
        self.raise_changed();
        id
    }

    /// Close `id`. Selection moves to the nearest neighbor; closing the last workspace seeds a
    /// replacement first (never-empty invariant). No-op (empty Vec) for unknown ids.
    pub fn close_workspace(&mut self, id: WorkspaceId) -> Vec<WorkspaceEvent> {
        let Some(index) = self.workspaces.iter().position(|w| w.id == id) else {
            return Vec::new();
        };

        let mut events = Vec::new();
        if self.workspaces.len() == 1 {
            events.push(WorkspaceEvent::Created(self.new_workspace_core()));
        }

        let closing = self.workspaces.remove(index);
        if self.selected_id == id {
            self.selected_id = self.workspaces[index.min(self.workspaces.len() - 1)].id;
        }

        events.push(WorkspaceEvent::Closed(closing));
        self.raise_changed();
        events
    }

    /// Select `id`. No-op for unknown ids.
    pub fn select_workspace(&mut self, id: WorkspaceId) {
        if self.selected_id == id || self.workspaces.iter().all(|w| w.id != id) {
            return;
        }
        self.selected_id = id;
        self.raise_changed();
    }

    /// The workspace with `id`, or `None`.
    pub fn find(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    /// Mutable access to the workspace with `id`, or `None`.
    pub fn find_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    /// The workspace whose tree currently holds `surface`, or `None`. Unambiguous because every
    /// controller mints from the shared allocator (ids are never reused).
    pub fn find_by_surface(&self, surface: SurfaceId) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|w| w.controller.all_surfaces().contains(&surface))
    }

    /// Mutable access to the workspace whose tree currently holds `surface`, or `None`.
    pub fn find_by_surface_mut(&mut self, surface: SurfaceId) -> Option<&mut Workspace> {
        self.workspaces
            .iter_mut()
            .find(|w| w.controller.all_surfaces().contains(&surface))
    }

    // ---- Surface-keyed report routing (the Phase-4 → sidebar seam) ----------------------------

    /// Route `report_git_branch` to the workspace owning `surface`.
    pub fn report_git_branch(&mut self, surface: SurfaceId, branch: &str, is_dirty: bool) {
        if let Some(workspace) = self.find_by_surface_mut(surface) {
            workspace.report_git_branch(GitBranchInfo {
                branch: branch.to_string(),
                is_dirty,
            });
        }
    }

    /// Route `report_pr` to the workspace owning `surface`.
    pub fn report_pr(
        &mut self,
        surface: SurfaceId,
        number: &str,
        label: &str,
        status: &str,
        branch: Option<&str>,
        is_stale: bool,
    ) {
        if let Some(workspace) = self.find_by_surface_mut(surface) {
            workspace.report_pull_request(PullRequestInfo {
                number: number.to_string(),
                label: label.to_string(),
                status: status.to_string(),
                branch: branch.map(str::to_string),
                is_stale,
            });
        }
    }

    /// Route `report_pwd` to the workspace owning `surface`.
    pub fn report_pwd(&mut self, surface: SurfaceId, path: &str) {
        if let Some(workspace) = self.find_by_surface_mut(surface) {
            workspace.report_working_directory(path);
        }
    }

    fn new_workspace_core(&mut self) -> WorkspaceId {
        self.next_workspace += 1;
        let id = WorkspaceId(self.next_workspace);
        let workspace = Workspace::with_changed_signal(
            id,
            SplitTreeController::with_allocator(Rc::clone(&self.ids)),
            Rc::clone(&self.changed),
        );
        self.workspaces.push(workspace);
        id
    }

    fn raise_changed(&self) {
        self.changed.set(self.changed.get() + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // ---- Lifecycle -----------------------------------------------------------------------------

    #[test]
    fn manager_seeds_one_selected_workspace() {
        let m = WorkspaceManager::new();

        assert_eq!(1, m.workspaces().len());
        let seed = &m.workspaces()[0];
        assert_eq!(seed.id(), m.selected_id());
        assert!(std::ptr::eq(seed, m.selected()));
        assert_eq!(1, seed.controller().all_surfaces().len());
    }

    #[test]
    fn new_workspace_appends_and_selects() {
        let mut m = WorkspaceManager::new();
        let first = m.selected().id();

        let second = m.new_workspace();

        let ids: Vec<WorkspaceId> = m.workspaces().iter().map(|w| w.id()).collect();
        assert_eq!(vec![first, second], ids);
        assert_eq!(second, m.selected_id());
    }

    #[test]
    fn surface_ids_are_globally_unique_across_workspaces() {
        let mut m = WorkspaceManager::new();
        let a = m.selected().id();
        let b = m.new_workspace();
        let pane_a = m.find(a).unwrap().controller().focused_pane();
        m.find_mut(a).unwrap().controller_mut().new_tab(pane_a);
        let pane_b = m.find(b).unwrap().controller().focused_pane();
        m.find_mut(b).unwrap().controller_mut().new_tab(pane_b);

        let mut all = m.find(a).unwrap().controller().all_surfaces();
        all.extend(m.find(b).unwrap().controller().all_surfaces());

        assert_eq!(4, all.len());
        let distinct: HashSet<SurfaceId> = all.iter().copied().collect();
        assert_eq!(all.len(), distinct.len());
    }

    #[test]
    fn close_workspace_moves_selection_to_neighbor() {
        let mut m = WorkspaceManager::new();
        let first = m.selected().id();
        let second = m.new_workspace();

        m.close_workspace(second);

        assert_eq!(1, m.workspaces().len());
        assert_eq!(first, m.workspaces()[0].id());
        assert_eq!(first, m.selected_id());
    }

    #[test]
    fn closing_a_non_selected_workspace_keeps_selection() {
        let mut m = WorkspaceManager::new();
        let first = m.selected().id();
        let second = m.new_workspace();

        m.close_workspace(first);

        assert_eq!(second, m.selected_id());
    }

    // The workspace analog of R6: the sidebar is never empty.
    #[test]
    fn closing_the_last_workspace_seeds_a_replacement() {
        let mut m = WorkspaceManager::new();
        let original = m.selected_id();

        m.close_workspace(original);

        assert_eq!(1, m.workspaces().len());
        let replacement = &m.workspaces()[0];
        assert_ne!(original, replacement.id());
        assert_eq!(replacement.id(), m.selected_id());
        assert_eq!(1, replacement.controller().all_surfaces().len());
    }

    #[test]
    fn lifecycle_events_fire_for_host_view_wiring() {
        let mut m = WorkspaceManager::new();

        // Created notification is the return value of new_workspace (C# WorkspaceCreated).
        let second = m.new_workspace();
        assert_eq!(second, m.workspaces()[1].id());

        let events = m.close_workspace(second);
        assert_eq!(1, events.len());
        match &events[0] {
            WorkspaceEvent::Closed(closed) => assert_eq!(second, closed.id()),
            WorkspaceEvent::Created(_) => panic!("expected Closed event"),
        }
    }

    #[test]
    fn select_workspace_ignores_unknown_ids() {
        let mut m = WorkspaceManager::new();
        let selected = m.selected_id();

        m.select_workspace(WorkspaceId(999));

        assert_eq!(selected, m.selected_id());
    }

    // ---- Surface→workspace resolution + report routing ------------------------------------------

    #[test]
    fn find_by_surface_resolves_the_owning_workspace() {
        let mut m = WorkspaceManager::new();
        let a = m.selected().id();
        let b = m.new_workspace();
        let surface_in_b = m.find(b).unwrap().controller().all_surfaces()[0];
        let surface_in_a = m.find(a).unwrap().controller().all_surfaces()[0];

        assert_eq!(b, m.find_by_surface(surface_in_b).unwrap().id());
        assert_eq!(a, m.find_by_surface(surface_in_a).unwrap().id());
        assert!(m.find_by_surface(SurfaceId(999)).is_none());
    }

    #[test]
    fn report_git_branch_lands_on_the_owning_workspace_only() {
        let mut m = WorkspaceManager::new();
        let a = m.selected().id();
        let b = m.new_workspace();
        let surface_in_b = m.find(b).unwrap().controller().all_surfaces()[0];

        m.report_git_branch(surface_in_b, "feat/sidebar", true);

        assert!(m.find(a).unwrap().git_branch().is_none());
        assert_eq!(
            Some(&GitBranchInfo {
                branch: "feat/sidebar".to_string(),
                is_dirty: true
            }),
            m.find(b).unwrap().git_branch()
        );
    }

    #[test]
    fn report_pr_and_pwd_update_workspace_metadata() {
        let mut m = WorkspaceManager::new();
        let surface = m.selected().controller().all_surfaces()[0];

        m.report_pr(
            surface,
            "42",
            "Add sidebar",
            "open",
            Some("feat/sidebar"),
            false,
        );
        m.report_pwd(surface, r"C:\dev\x");

        let w = m.selected();
        assert_eq!(
            Some(&PullRequestInfo {
                number: "42".to_string(),
                label: "Add sidebar".to_string(),
                status: "open".to_string(),
                branch: Some("feat/sidebar".to_string()),
                is_stale: false
            }),
            w.pull_request()
        );
        assert_eq!(Some(r"C:\dev\x"), w.current_directory());
    }

    #[test]
    fn reports_for_unknown_surfaces_are_dropped() {
        let mut m = WorkspaceManager::new();

        m.report_git_branch(SurfaceId(999), "main", false);

        assert!(m.selected().git_branch().is_none());
    }

    // The "watch git status" gate (plan Phase 5 U2).
    #[test]
    fn watch_git_status_off_suppresses_git_and_pr_reports() {
        let mut m = WorkspaceManager::new();
        m.selected_mut().set_watch_git_status(false);
        let surface = m.selected().controller().all_surfaces()[0];

        m.report_git_branch(surface, "main", false);
        m.report_pr(surface, "42", "x", "open", None, false);

        let w = m.selected();
        assert!(w.git_branch().is_none());
        assert!(w.pull_request().is_none());
    }

    // ---- Workspace metadata --------------------------------------------------------------------

    #[test]
    fn display_title_prefers_custom_then_derived_then_id() {
        let mut m = WorkspaceManager::new();

        assert_eq!(m.selected().id().to_string(), m.selected().display_title());

        m.selected_mut().set_title("pwsh — C:\\dev");
        assert_eq!("pwsh — C:\\dev", m.selected().display_title());

        m.selected_mut().set_custom_title(Some("backend"));
        assert_eq!("backend", m.selected().display_title());

        m.selected_mut().set_custom_title(None);
        assert_eq!("pwsh — C:\\dev", m.selected().display_title());
    }

    #[test]
    fn set_status_parses_key_value_and_clears_on_empty_value() {
        let mut m = WorkspaceManager::new();
        let w = m.selected_mut();

        w.set_status("claude:busy");
        assert_eq!("busy", w.status_entries()["claude"]);

        w.set_status("claude:idle");
        assert_eq!("idle", w.status_entries()["claude"]);

        w.set_status("claude:");
        assert!(!w.status_entries().contains_key("claude"));
    }

    #[test]
    fn metadata_changes_raise_the_manager_changed_event() {
        let mut m = WorkspaceManager::new();
        let before = m.changed_count();

        m.selected_mut().set_title("t");
        let surface = m.selected().controller().all_surfaces()[0];
        m.report_pwd(surface, r"C:\x");
        m.selected_mut().set_progress(Some("3/5"));

        assert_eq!(3, m.changed_count() - before);
    }

    // Idempotent mutations must not spam the sidebar with re-renders.
    #[test]
    fn redundant_mutations_do_not_raise_changed() {
        let mut m = WorkspaceManager::new();
        m.selected_mut().set_title("t");
        let before = m.changed_count();

        m.selected_mut().set_title("t");
        m.selected_mut().set_progress(None);
        m.selected_mut().set_custom_title(Some(""));

        assert_eq!(before, m.changed_count());
    }
}
