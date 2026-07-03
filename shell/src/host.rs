//! The domain host: the app-side owner of the live model plane (workspaces + notifications)
//! and the seam every Phase-4 socket effect lands on. Port of `app/Sidebar/WorkspaceHost.cs`'s
//! socket-routing half + `app/Ipc/PipeServerEffects.cs`'s dispatcher marshalling.
//!
//! Why a thread, not a `Mutex`: [`WorkspaceManager`] and [`NotificationCoordinator`] are
//! `Rc`/`Box<dyn Fn>`-based — `!Send`, "UI-thread only by convention" (KTD3). They cannot be
//! shared across the pipe server's per-client threads. So the domain lives on one dedicated
//! thread and callers hand it work over a channel — exactly the C# `DispatcherQueue` seam
//! (`RunOnDispatcher` for fire-and-forget, `InvokeOnDispatcher` for queries). [`DomainHost`] is
//! the `Send + Sync + Clone` handle the socket effects hold; [`Domain`] is the state it guards.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::domain::capacity::CapacityModel;
use crate::domain::ids::{BranchId, SurfaceId};
use crate::domain::notifications::{
    NotificationCoordinator, NotificationId, SurfaceNotification, TerminalNotification,
};
use crate::domain::split_tree::{Direction, Orientation, SplitTreeEvent};
use crate::domain::surface_manager::{
    CreateSurfaceError, NoSurfaceFactory, SurfaceFactory, SurfaceManager,
};
use crate::domain::workspace::WorkspaceManager;
use crate::view::{build_tree_view, TreeView};

/// The single-threaded model plane the socket effects mutate: the workspace manager (sidebar
/// rows + reported metadata) and the notification coordinator (queue + store + policy). Both are
/// `!Send`; this type only ever exists on the domain thread. Its methods mirror the domain-facing
/// half of the C# `WorkspaceHost` socket routing.
pub struct Domain {
    workspaces: WorkspaceManager,
    notifications: NotificationCoordinator,
    surfaces: SurfaceManager,
}

impl Default for Domain {
    fn default() -> Self {
        Self::new()
    }
}

impl Domain {
    /// Build the domain with no live-surface backing: `focus`/`send` resolve the model but reach
    /// no engine ([`NoSurfaceFactory`]). The frontend spawn command installs the real
    /// engine-backed factory via [`Self::with_surface_factory`].
    pub fn new() -> Self {
        Self::with_surface_factory(Arc::new(NoSurfaceFactory))
    }

    /// Build the domain over a concrete [`SurfaceFactory`], ungoverned (tests, degraded startup).
    pub fn with_surface_factory(factory: Arc<dyn SurfaceFactory>) -> Self {
        Self::with_surfaces(factory, None)
    }

    /// Build the domain over a concrete [`SurfaceFactory`] and an optional RAM safe-zone
    /// [`CapacityModel`] — the seam the app wires the engine bridge + capacity governor onto (the
    /// crown-jewel spawn cap). `None` capacity leaves the manager ungoverned (tests).
    pub fn with_surfaces(
        factory: Arc<dyn SurfaceFactory>,
        capacity: Option<Arc<CapacityModel>>,
    ) -> Self {
        Self {
            workspaces: WorkspaceManager::new(),
            notifications: NotificationCoordinator::new(),
            surfaces: SurfaceManager::new(factory, capacity),
        }
    }

    /// The selected workspace's focused surface — the seeded model surface at startup, and the
    /// default target for caller-scoped notifications. Handy for wiring/tests.
    pub fn selected_focused_surface(&self) -> Option<SurfaceId> {
        self.workspaces.selected().controller().focused_surface()
    }

    // ---- Surface verbs -------------------------------------------------------------------------

    /// Select the workspace owning `surface`, then focus the live surface if one is registered
    /// (the port of C# `WorkspaceHost.FocusSurface` → `SelectWorkspace` + `FocusSurfaceById`). The
    /// workspace select is always real; the surface focus is a no-op until the engine bridge backs
    /// this id (`find_by_surface` gates on the surface being in a tree, so a stray id does nothing).
    pub fn focus_surface(&mut self, surface: SurfaceId) {
        if let Some(workspace) = self.workspaces.find_by_surface(surface).map(|w| w.id()) {
            self.workspaces.select_workspace(workspace);
            if let Some(live) = self.surfaces.get(surface) {
                live.focus_surface();
            }
        }
    }

    /// Forward text to the live surface for `surface` (the `surface.send_text` verb). No-op when no
    /// engine backs the id — a keystroke to a surface that was never spawned has nowhere to land.
    pub fn send_text(&self, surface: SurfaceId, text: &str) {
        if let Some(live) = self.surfaces.get(surface) {
            live.send_text(text);
        }
    }

    /// Forward a key press to the live surface for `surface` (the `surface.send_key` verb). No-op
    /// when no engine backs the id.
    pub fn send_key(&self, surface: SurfaceId, virtual_key: u32, modifiers: u32) {
        if let Some(live) = self.surfaces.get(surface) {
            live.send_key(virtual_key, modifiers);
        }
    }

    /// Resize the live surface's grid (the frontend `dev_resize` path — xterm.js fit). No-op when
    /// no engine backs the id.
    pub fn resize(&self, surface: SurfaceId, cols: u16, rows: u16) {
        if let Some(live) = self.surfaces.get(surface) {
            live.resize(cols, rows);
        }
    }

    /// Create the engine-backed surface for `id` through the capacity-governed choke point
    /// (RAM safe-zone). `Ok(None)` = refused at cap; `Ok(Some(id))` = live. The frontend spawn
    /// command calls this once the engine factory is installed; with [`NoSurfaceFactory`] it errs.
    pub fn try_create_surface(
        &mut self,
        id: SurfaceId,
        cwd: Option<&str>,
        cmdline: Option<&str>,
    ) -> Result<Option<SurfaceId>, CreateSurfaceError> {
        Ok(self
            .surfaces
            .try_create_surface(id, cwd, cmdline)?
            .map(|surface| surface.id()))
    }

    /// Tear down the live surface for `id`, freeing its safe-zone slot. No-op for unknown ids.
    pub fn dispose_surface(&mut self, id: SurfaceId) {
        self.surfaces.dispose_surface(id);
    }

    // ---- Split-tree verbs (drive the selected workspace's controller) --------------------------

    /// Flatten the selected workspace's split tree into the serializable [`TreeView`] the frontend
    /// renders (pane rects + divider handles + focus/zoom). Every mutating verb below pairs with
    /// this in one domain job so the frontend gets the fresh layout back from each call.
    pub fn tree_view(&self) -> TreeView {
        build_tree_view(&self.workspaces.selected().controller().snapshot())
    }

    /// Split the focused pane, returning the new pane's surface id (a *model* surface with no
    /// engine yet — the caller stages a channel and calls [`try_create_surface`](Self::try_create_surface)
    /// to back it, rolling the split back via [`close_surface`](Self::close_surface) if refused at
    /// cap). `None` when there is no tree/pane to split.
    pub fn split_focused(
        &mut self,
        orientation: Orientation,
        insert_first: bool,
    ) -> Option<SurfaceId> {
        let pane = self.workspaces.selected().controller().focused_pane();
        let events =
            self.workspaces
                .selected_mut()
                .controller_mut()
                .split(pane, orientation, insert_first);
        first_created(&events)
    }

    /// Add a tab (new surface) to the focused pane, same backer contract as [`split_focused`].
    pub fn new_tab_focused(&mut self) -> Option<SurfaceId> {
        let pane = self.workspaces.selected().controller().focused_pane();
        let events = self
            .workspaces
            .selected_mut()
            .controller_mut()
            .new_tab(pane);
        first_created(&events)
    }

    /// Close the tab backing `surface`, disposing the engine of every surface the tree removed
    /// (the pane heals / empties per the controller's rules). Also the rollback path when a fresh
    /// split's engine is refused at cap.
    pub fn close_surface(&mut self, surface: SurfaceId) {
        let events = self
            .workspaces
            .selected_mut()
            .controller_mut()
            .close_tab(surface);
        for event in &events {
            if let SplitTreeEvent::SurfaceClosed(closed) = event {
                self.surfaces.dispose_surface(*closed);
            }
        }
    }

    /// Close the focused pane's selected surface (the `Ctrl+W`/close-button path).
    pub fn close_focused(&mut self) {
        if let Some(surface) = self.workspaces.selected().controller().focused_surface() {
            self.close_surface(surface);
        }
    }

    /// Move focus to the nearest pane in `direction` (no-op if none lies that way).
    pub fn move_focus(&mut self, direction: Direction) {
        self.workspaces
            .selected_mut()
            .controller_mut()
            .move_focus(direction);
    }

    /// Focus a pane explicitly (pointer click).
    pub fn focus_pane(&mut self, pane: crate::domain::ids::PaneId) {
        self.workspaces
            .selected_mut()
            .controller_mut()
            .focus_pane(pane);
    }

    /// Select `surface` as the visible tab within `pane`.
    pub fn select_tab(&mut self, pane: crate::domain::ids::PaneId, surface: SurfaceId) {
        self.workspaces
            .selected_mut()
            .controller_mut()
            .select_tab(pane, surface);
    }

    pub fn select_next_tab(&mut self) {
        self.workspaces
            .selected_mut()
            .controller_mut()
            .select_next_tab();
    }

    pub fn select_previous_tab(&mut self) {
        self.workspaces
            .selected_mut()
            .controller_mut()
            .select_previous_tab();
    }

    /// Toggle full-bleed zoom of the focused pane.
    pub fn toggle_zoom(&mut self) {
        self.workspaces
            .selected_mut()
            .controller_mut()
            .toggle_zoom();
    }

    /// Set a branch divider fraction (clamped [0,1] controller-side) — the splitter-drag path.
    pub fn set_divider(&mut self, branch: BranchId, fraction: f64) {
        self.workspaces
            .selected_mut()
            .controller_mut()
            .set_divider_position(branch, fraction);
    }

    /// Reset every divider to 0.5.
    pub fn equalize(&mut self) {
        self.workspaces.selected_mut().controller_mut().equalize();
    }

    // ---- Notification verbs --------------------------------------------------------------------

    /// Record a notification aimed at `surface` (the `notify` / `notification.create` path).
    /// Resolution is by surface owner; the `workspace_id` hint is accepted but the surface is
    /// authoritative (it uniquely identifies the workspace via the shared id allocator).
    pub fn create_for_target(
        &mut self,
        _workspace_id: &str,
        surface: SurfaceId,
        title: &str,
        subtitle: &str,
        body: &str,
    ) {
        self.record_notification(surface, SurfaceNotification::new(title, subtitle, body));
    }

    /// Record a caller-scoped notification (`notification.create_for_caller`). Targets the
    /// caller's preferred surface when it parses and is live, else the selected workspace's
    /// focused surface (macOS parity: a parseable `OPTIMUS_SURFACE_ID` pins the target).
    pub fn create_for_caller(
        &mut self,
        preferred_surface_id: Option<&str>,
        title: &str,
        subtitle: &str,
        body: &str,
    ) {
        let target = preferred_surface_id
            .and_then(parse_surface_id)
            .filter(|s| self.workspaces.find_by_surface(*s).is_some())
            .or_else(|| self.selected_focused_surface());
        if let Some(surface) = target {
            self.record_notification(surface, SurfaceNotification::new(title, subtitle, body));
        }
    }

    /// Enqueue + drain against the owning workspace's tree so the notification is recorded under
    /// the right pane. A surface not in any tree has nothing to attach to and is dropped (the
    /// coordinator's own orphan rule). `app_focused = false`: the domain layer can't see window
    /// foreground state — false records the notification unread and requests a toast, the safe
    /// default; wire real foreground state when the window bridge lands.
    fn record_notification(&mut self, surface: SurfaceId, payload: SurfaceNotification) {
        let Some(snapshot) = self
            .workspaces
            .find_by_surface(surface)
            .map(|w| w.controller().snapshot())
        else {
            return;
        };
        self.notifications.on_notification(surface, payload, false); // CLI path never coalesces
        self.notifications.drain(&snapshot, false);
    }

    pub fn notification_list(&self) -> Vec<TerminalNotification> {
        self.notifications.list_notifications().to_vec()
    }

    pub fn notification_dismiss(&mut self, id: NotificationId) {
        self.notifications.dismiss_notification(id);
    }

    pub fn notification_dismiss_for_surface(&mut self, surface: SurfaceId) {
        self.notifications.dismiss_notification_for_surface(surface);
    }

    pub fn notification_dismiss_all_read(&mut self) {
        self.notifications.dismiss_all_read();
    }

    pub fn notification_clear(&mut self) {
        self.notifications.clear_notifications();
    }

    pub fn notification_mark_read(&mut self, id: NotificationId) {
        self.notifications.mark_read(id);
    }

    pub fn notification_mark_read_for_surface(&mut self, surface: SurfaceId) {
        self.notifications.mark_read_for_surface(surface);
    }

    pub fn notification_mark_all_read(&mut self) {
        self.notifications.mark_all_read();
    }

    /// Open a notification by id: select the owning workspace (the C# path then focuses the
    /// surface control — deferred to the view bridge).
    pub fn notification_open(&mut self, id: NotificationId) {
        let surface = self
            .notifications
            .list_notifications()
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.surface_id);
        if let Some(surface) = surface {
            self.focus_surface(surface);
        }
    }

    /// Jump to the newest unread notification, selecting its workspace. Returns whether one
    /// existed (drives the CLI `OK` / `ERROR: no unread` reply).
    pub fn jump_to_unread(&mut self) -> bool {
        match self.notifications.jump_to_unread_target() {
            Some((_pane, surface)) => {
                self.focus_surface(surface);
                true
            }
            None => false,
        }
    }

    // ---- Workspace-metadata verbs (land on the sidebar row) ------------------------------------

    pub fn set_status(&mut self, status: &str) {
        self.workspaces.selected_mut().set_status(status);
    }

    pub fn set_progress(&mut self, progress: &str) {
        // The workspace normalizes an empty string to "no progress".
        self.workspaces.selected_mut().set_progress(Some(progress));
    }

    pub fn report_git_branch(&mut self, surface: SurfaceId, branch: &str, is_dirty: bool) {
        self.workspaces.report_git_branch(surface, branch, is_dirty);
    }

    pub fn report_pr(
        &mut self,
        surface: SurfaceId,
        number: &str,
        label: &str,
        status: &str,
        branch: Option<&str>,
        is_stale: bool,
    ) {
        self.workspaces
            .report_pr(surface, number, label, status, branch, is_stale);
    }

    pub fn report_pwd(&mut self, surface: SurfaceId, path: &str) {
        self.workspaces.report_pwd(surface, path);
    }
}

/// The surface id a structural op created, if any — extracted from the controller's event list so
/// the host knows which new pane needs an engine backer.
fn first_created(events: &[SplitTreeEvent]) -> Option<SurfaceId> {
    events.iter().find_map(|event| match event {
        SplitTreeEvent::SurfaceCreated(surface) => Some(*surface),
        _ => None,
    })
}

/// Parse a surface id from `S<n>` or a bare integer (mirrors the router's `parse_surface_id`).
fn parse_surface_id(value: &str) -> Option<SurfaceId> {
    let trimmed = value.trim();
    let digits = trimmed
        .strip_prefix(['S', 's'])
        .filter(|rest| !rest.is_empty())
        .unwrap_or(trimmed);
    digits.parse::<i32>().ok().map(SurfaceId)
}

/// A unit of work to run against the [`Domain`] on its own thread.
type Job = Box<dyn FnOnce(&mut Domain) + Send>;

/// The `Send + Sync + Clone` handle every socket effect holds. Hands work to the domain thread:
/// [`run`](Self::run) is fire-and-forget (`RunOnDispatcher`), [`query`](Self::query) blocks for
/// a result (`InvokeOnDispatcher`). Cloning shares the same domain (the sender is refcounted); the
/// thread lives until the last handle drops (channel closes → the loop ends).
#[derive(Clone)]
pub struct DomainHost {
    // Mutex wrapper only exists to make the `!Sync` mpsc `Sender` `Sync` so the dispatch closure
    // can be `Arc<dyn Fn + Send + Sync>`. Contention is nil — a send just moves a box.
    tx: Arc<Mutex<Sender<Job>>>,
}

impl DomainHost {
    /// Spawn the domain thread with no live-surface backing ([`NoSurfaceFactory`], ungoverned) —
    /// used by tests and the pipe-only paths. The app uses [`Self::spawn_with`] to install the
    /// engine factory + capacity governor.
    pub fn spawn() -> Self {
        Self::spawn_with(Arc::new(NoSurfaceFactory), None)
    }

    /// Spawn the domain thread over a concrete [`SurfaceFactory`] and optional [`CapacityModel`].
    /// The [`Domain`] is built on that thread and never leaves it — only `Send` work + `Send`
    /// arguments cross the channel (the factory and capacity model are `Send + Sync`).
    pub fn spawn_with(
        factory: Arc<dyn SurfaceFactory>,
        capacity: Option<Arc<CapacityModel>>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("optimus-domain".into())
            .spawn(move || {
                let mut domain = Domain::with_surfaces(factory, capacity);
                while let Ok(job) = rx.recv() {
                    job(&mut domain);
                }
            })
            .expect("spawn optimus-domain thread");
        Self {
            tx: Arc::new(Mutex::new(tx)),
        }
    }

    /// Run `f` on the domain thread, fire-and-forget. Drops silently if the thread is gone.
    pub fn run(&self, f: impl FnOnce(&mut Domain) + Send + 'static) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(Box::new(f));
        }
    }

    /// Run `f` on the domain thread and block for its result. Returns `T::default()` if the
    /// domain thread has died (a domain-side panic) rather than propagating — a single bad
    /// request must not take the server down.
    pub fn query<T: Default + Send + 'static>(
        &self,
        f: impl FnOnce(&mut Domain) -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = mpsc::channel();
        self.run(move |d| {
            let _ = tx.send(f(d));
        });
        rx.recv().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Domain (synchronous, no thread) -------------------------------------------------------

    #[test]
    fn create_for_target_records_a_notification_on_a_live_surface() {
        let mut domain = Domain::new();
        let surface = domain.selected_focused_surface().expect("seeded surface");

        domain.create_for_target("", surface, "build", "done", "ok");

        let list = domain.notification_list();
        assert_eq!(1, list.len());
        assert_eq!("build", list[0].title);
        assert_eq!(surface, list[0].surface_id);
        assert!(!list[0].is_read); // app-focus unknown -> recorded unread
    }

    #[test]
    fn create_for_unknown_surface_records_nothing() {
        let mut domain = Domain::new();
        domain.create_for_target("", SurfaceId(9999), "x", "", "");
        assert!(domain.notification_list().is_empty());
    }

    #[test]
    fn create_for_caller_targets_the_selected_focused_surface() {
        let mut domain = Domain::new();
        let surface = domain.selected_focused_surface().unwrap();

        domain.create_for_caller(None, "caller", "", "body");

        let list = domain.notification_list();
        assert_eq!(1, list.len());
        assert_eq!(surface, list[0].surface_id);
    }

    #[test]
    fn mark_read_then_dismiss_walks_a_notification_through_its_lifecycle() {
        let mut domain = Domain::new();
        let surface = domain.selected_focused_surface().unwrap();
        domain.create_for_target("", surface, "t", "", "");
        let id = domain.notification_list()[0].id;

        assert!(domain.jump_to_unread()); // one unread exists
        domain.notification_mark_read(id);
        assert!(!domain.jump_to_unread()); // now everything is read

        domain.notification_dismiss(id);
        assert!(domain.notification_list().is_empty());
    }

    #[test]
    fn report_git_branch_lands_on_the_owning_workspace() {
        let mut domain = Domain::new();
        let surface = domain.selected_focused_surface().unwrap();

        domain.report_git_branch(surface, "feat/x", true);

        let workspace = domain.workspaces.find_by_surface(surface).unwrap();
        let git = workspace.git_branch().expect("branch recorded");
        assert_eq!("feat/x", git.branch);
        assert!(git.is_dirty);
    }

    #[test]
    fn parse_surface_id_accepts_prefixed_and_bare() {
        assert_eq!(Some(SurfaceId(4)), parse_surface_id("S4"));
        assert_eq!(Some(SurfaceId(7)), parse_surface_id("7"));
        assert_eq!(None, parse_surface_id("S"));
        assert_eq!(None, parse_surface_id("nope"));
    }

    // ---- Surface routing (focus/send reach the live engine-backed surface) ---------------------

    use crate::domain::surface_manager::Surface;

    /// A live surface that only records the levers pulled on it — the stand-in for the
    /// engine-backed pane, so a test proves the Domain → SurfaceManager → Surface path end to end.
    struct RecordingSurface {
        id: SurfaceId,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Surface for RecordingSurface {
        fn id(&self) -> SurfaceId {
            self.id
        }
        fn set_active(&self, _active: bool) {}
        fn focus_surface(&self) {
            self.log.lock().unwrap().push(format!("focus:{}", self.id));
        }
        fn send_text(&self, text: &str) {
            self.log
                .lock()
                .unwrap()
                .push(format!("text:{}:{text}", self.id));
        }
        fn send_key(&self, virtual_key: u32, modifiers: u32) {
            self.log
                .lock()
                .unwrap()
                .push(format!("key:{}:{virtual_key}:{modifiers}", self.id));
        }
        fn resize(&self, cols: u16, rows: u16) {
            self.log
                .lock()
                .unwrap()
                .push(format!("resize:{}:{cols}:{rows}", self.id));
        }
        fn shutdown(&self) {
            self.log
                .lock()
                .unwrap()
                .push(format!("shutdown:{}", self.id));
        }
    }

    struct RecordingFactory {
        log: Arc<Mutex<Vec<String>>>,
    }

    impl SurfaceFactory for RecordingFactory {
        fn create(
            &self,
            id: SurfaceId,
            _cwd: Option<&str>,
            _cmdline: Option<&str>,
        ) -> Result<Arc<dyn Surface>, String> {
            Ok(Arc::new(RecordingSurface {
                id,
                log: Arc::clone(&self.log),
            }))
        }
    }

    fn domain_with_recording_surfaces() -> (Domain, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(RecordingFactory {
            log: Arc::clone(&log),
        });
        (Domain::with_surface_factory(factory), log)
    }

    #[test]
    fn send_text_and_send_key_reach_the_live_surface() {
        let (mut domain, log) = domain_with_recording_surfaces();
        let surface = domain.selected_focused_surface().unwrap();
        domain
            .try_create_surface(surface, None, None)
            .unwrap()
            .expect("surface created");

        domain.send_text(surface, "echo hi\r");
        domain.send_key(surface, 13, 4); // VK_RETURN + a modifier bit

        let log = log.lock().unwrap();
        assert!(
            log.contains(&format!("text:{surface}:echo hi\r")),
            "{log:?}"
        );
        assert!(log.contains(&format!("key:{surface}:13:4")), "{log:?}");
    }

    #[test]
    fn focus_surface_selects_the_workspace_and_focuses_the_live_surface() {
        let (mut domain, log) = domain_with_recording_surfaces();
        let surface = domain.selected_focused_surface().unwrap();
        domain
            .try_create_surface(surface, None, None)
            .unwrap()
            .unwrap();

        domain.focus_surface(surface);

        assert!(log.lock().unwrap().contains(&format!("focus:{surface}")));
    }

    #[test]
    fn send_to_a_surface_with_no_engine_is_a_noop() {
        let (domain, log) = domain_with_recording_surfaces();
        // The seeded id is in a tree but was never spawned — nothing backs it.
        let surface = domain.selected_focused_surface().unwrap();

        domain.send_text(surface, "x");
        domain.send_key(surface, 13, 0);

        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn disposed_surface_stops_receiving_input() {
        let (mut domain, log) = domain_with_recording_surfaces();
        let surface = domain.selected_focused_surface().unwrap();
        domain
            .try_create_surface(surface, None, None)
            .unwrap()
            .unwrap();
        domain.dispose_surface(surface);
        log.lock().unwrap().clear(); // drop the shutdown entry

        domain.send_text(surface, "x");
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn default_domain_has_no_surface_factory_and_refuses_creation() {
        let mut domain = Domain::new();
        let surface = domain.selected_focused_surface().unwrap();
        assert!(domain.try_create_surface(surface, None, None).is_err());
    }

    // ---- Split-tree verbs (structural ops drive the selected workspace's controller) -----------

    #[test]
    fn split_focused_adds_a_pane_and_the_view_shows_two_panes_and_a_divider() {
        let (mut domain, _log) = domain_with_recording_surfaces();
        assert_eq!(1, domain.tree_view().panes.len());

        let new_surface = domain
            .split_focused(Orientation::Vertical, false)
            .expect("a new surface for the split pane");
        domain
            .try_create_surface(new_surface, None, None)
            .unwrap()
            .unwrap();

        let view = domain.tree_view();
        assert_eq!(2, view.panes.len(), "split should yield two panes");
        assert_eq!(1, view.dividers.len(), "one branch → one divider");
        // The new pane is focused (controller R4) and holds the new surface.
        let focused = view.panes.iter().find(|p| p.focused).unwrap();
        assert!(focused.tabs.contains(&new_surface.0));
    }

    #[test]
    fn close_surface_heals_back_to_one_pane_and_disposes_the_engine() {
        let (mut domain, log) = domain_with_recording_surfaces();
        let new_surface = domain.split_focused(Orientation::Vertical, false).unwrap();
        domain
            .try_create_surface(new_surface, None, None)
            .unwrap()
            .unwrap();
        log.lock().unwrap().clear();

        domain.close_surface(new_surface);

        assert_eq!(
            1,
            domain.tree_view().panes.len(),
            "tree heals to a single pane"
        );
        assert_eq!(0, domain.tree_view().dividers.len());
        assert!(
            log.lock()
                .unwrap()
                .contains(&format!("shutdown:{new_surface}")),
            "closing the pane must tear down its engine",
        );
    }

    #[test]
    fn new_tab_focused_adds_a_tab_to_the_pane_without_splitting() {
        let (mut domain, _log) = domain_with_recording_surfaces();
        let tab_surface = domain.new_tab_focused().expect("a new tab surface");

        let view = domain.tree_view();
        assert_eq!(1, view.panes.len(), "a new tab does not add a pane");
        assert!(view.panes[0].tabs.contains(&tab_surface.0));
        assert_eq!(tab_surface.0, view.panes[0].selected, "new tab is selected");
    }

    #[test]
    fn toggle_zoom_marks_the_focused_pane_zoomed_in_the_view() {
        let (mut domain, _log) = domain_with_recording_surfaces();
        assert_eq!(None, domain.tree_view().zoomed_pane);

        domain.toggle_zoom();
        let view = domain.tree_view();
        assert_eq!(Some(view.focused_pane), view.zoomed_pane);

        domain.toggle_zoom();
        assert_eq!(None, domain.tree_view().zoomed_pane);
    }

    // ---- DomainHost (the thread + channel marshalling) -----------------------------------------

    #[test]
    fn host_round_trips_a_create_and_a_query_through_the_thread() {
        let host = DomainHost::spawn();
        let surface = host
            .query(|d| d.selected_focused_surface())
            .expect("seeded surface");

        host.run(move |d| d.create_for_target("", surface, "async", "", "body"));
        let list = host.query(|d| d.notification_list());

        assert_eq!(1, list.len());
        assert_eq!("async", list[0].title);
    }
}
