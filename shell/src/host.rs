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

use crate::domain::ids::SurfaceId;
use crate::domain::notifications::{
    NotificationCoordinator, NotificationId, SurfaceNotification, TerminalNotification,
};
use crate::domain::workspace::WorkspaceManager;

/// The single-threaded model plane the socket effects mutate: the workspace manager (sidebar
/// rows + reported metadata) and the notification coordinator (queue + store + policy). Both are
/// `!Send`; this type only ever exists on the domain thread. Its methods mirror the domain-facing
/// half of the C# `WorkspaceHost` socket routing.
pub struct Domain {
    workspaces: WorkspaceManager,
    notifications: NotificationCoordinator,
}

impl Default for Domain {
    fn default() -> Self {
        Self::new()
    }
}

impl Domain {
    pub fn new() -> Self {
        Self {
            workspaces: WorkspaceManager::new(),
            notifications: NotificationCoordinator::new(),
        }
    }

    /// The selected workspace's focused surface — the seeded model surface at startup, and the
    /// default target for caller-scoped notifications. Handy for wiring/tests.
    pub fn selected_focused_surface(&self) -> Option<SurfaceId> {
        self.workspaces.selected().controller().focused_surface()
    }

    // ---- Surface verbs -------------------------------------------------------------------------

    /// Select the workspace owning `surface`. The C# `FocusSurface` then focuses the surface
    /// control; that half needs the view/engine bridge (P4/P5), so here we do only the real,
    /// view-free step — moving selection to the owning workspace.
    pub fn focus_surface(&mut self, surface: SurfaceId) {
        if let Some(workspace) = self.workspaces.find_by_surface(surface).map(|w| w.id()) {
            self.workspaces.select_workspace(workspace);
        }
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
    /// Spawn the domain thread and return a handle to it. The [`Domain`] is built on that thread
    /// and never leaves it — the only thing that crosses the channel is `Send` work + `Send`
    /// arguments.
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("optimus-domain".into())
            .spawn(move || {
                let mut domain = Domain::new();
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
