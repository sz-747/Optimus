//! Port of core/Sidebar/SidebarProjection.cs + core/Splits/TabHeaderDto.cs + core/Splits/Shortcuts.cs
//! (P2 unit 8). Two pure model→DTO projections the chrome renders by value (issue-#2586: the view
//! never holds a live, mutating workspace/surface), plus the platform-neutral chord→action table and
//! dispatcher (R8). Kept out of the view plane so all three are unit-testable without a webview.

use crate::domain::ids::SurfaceId;
use crate::domain::split_tree::{Direction, Orientation, SplitTreeController};
use crate::domain::workspace::{PullRequestInfo, Workspace, WorkspaceId, WorkspaceManager};

// ---- Sidebar row projection -------------------------------------------------------------------

/// A by-value snapshot of one sidebar row (issue-#2586: rows are values, never observables bound to
/// a live `Workspace`). The Rust analog of `SidebarRowDto`; recomputed on every change.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SidebarRow {
    pub id: WorkspaceId,
    pub title: String,
    pub is_selected: bool,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub pr_badge: Option<String>,
    pub pr_status: Option<String>,
    pub cwd: Option<String>,
    pub status: Option<String>,
    pub progress: Option<String>,
    pub latest_text: Option<String>,
    pub unread_count: i32,
}

/// Project every workspace into a row. `unread_of` / `latest_of` supply the per-workspace
/// notification feed (each workspace's coordinator lives in the view plane); when `None`, rows
/// project zero unread / no latest text.
pub fn project_sidebar(
    manager: &WorkspaceManager,
    unread_of: Option<&dyn Fn(WorkspaceId) -> i32>,
    latest_of: Option<&dyn Fn(WorkspaceId) -> Option<String>>,
) -> Vec<SidebarRow> {
    let selected = manager.selected_id();
    manager
        .workspaces()
        .iter()
        .map(|w| SidebarRow {
            id: w.id(),
            title: w.display_title(),
            is_selected: w.id() == selected,
            git_branch: w.git_branch().map(|g| g.branch.clone()),
            git_dirty: w.git_branch().map(|g| g.is_dirty).unwrap_or(false),
            pr_badge: pr_badge(w.pull_request()),
            pr_status: w.pull_request().map(|p| p.status.clone()),
            cwd: compact_path(w.current_directory()),
            status: status_summary(w),
            progress: w.progress().map(str::to_string),
            latest_text: latest_of.and_then(|f| f(w.id())),
            unread_count: unread_of.map(|f| f(w.id())).unwrap_or(0),
        })
        .collect()
}

/// "#42 open", or `None` when nothing was reported.
fn pr_badge(pr: Option<&PullRequestInfo>) -> Option<String> {
    let p = pr?;
    if p.number.is_empty() {
        return None;
    }
    let number = if p.number.starts_with('#') {
        p.number.clone()
    } else {
        format!("#{}", p.number)
    };
    Some(if p.status.is_empty() {
        number
    } else {
        format!("{number} {}", p.status)
    })
}

/// "claude busy · codex idle", or `None` when no agent reported a status. Keys sort ordinal.
fn status_summary(w: &Workspace) -> Option<String> {
    let entries = w.status_entries();
    if entries.is_empty() {
        return None;
    }
    let mut sorted: Vec<(&String, &String)> = entries.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    Some(
        sorted
            .into_iter()
            .map(|(k, v)| {
                if k.is_empty() {
                    v.clone()
                } else {
                    format!("{k} {v}")
                }
            })
            .collect::<Vec<_>>()
            .join(" · "),
    )
}

/// Abbreviate the user-profile prefix to "~" so rows stay short (macOS parity).
fn compact_path(path: Option<&str>) -> Option<String> {
    let path = path?;
    if path.is_empty() {
        return None;
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.is_empty()
            && path.len() >= home.len()
            && path[..home.len()].eq_ignore_ascii_case(&home)
        {
            let rest = &path[home.len()..];
            return Some(if rest.is_empty() {
                "~".to_string()
            } else {
                format!("~{rest}")
            });
        }
    }
    Some(path.to_string())
}

// ---- Tab header projection --------------------------------------------------------------------

/// A by-value snapshot of one tab's header (KTD5). `unread` drives the Phase-3 unread dot.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TabHeader {
    pub id: SurfaceId,
    pub title: String,
    pub is_selected: bool,
    pub unread: bool,
}

/// Project `tabs` into headers, marking exactly the one equal to `selected`. `title_of` supplies the
/// live title for a surface; when it returns `None`/empty (or is `None`), the surface id is used as a
/// stable placeholder. `is_unread` reports the Phase-3 unread flag; when `None`, nothing is unread.
pub fn project_tab_headers(
    tabs: &[SurfaceId],
    selected: SurfaceId,
    title_of: Option<&dyn Fn(SurfaceId) -> Option<String>>,
    is_unread: Option<&dyn Fn(SurfaceId) -> bool>,
) -> Vec<TabHeader> {
    tabs.iter()
        .map(|&id| {
            let title = title_of
                .and_then(|f| f(id))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| id.to_string());
            TabHeader {
                id,
                title,
                is_selected: id == selected,
                unread: is_unread.map(|f| f(id)).unwrap_or(false),
            }
        })
        .collect()
}

// ---- Shortcut chord table + dispatcher --------------------------------------------------------

/// Modifier flags for a keyboard chord — platform-neutral so the table is unit-testable.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChordModifiers(u8);

impl ChordModifiers {
    pub const NONE: ChordModifiers = ChordModifiers(0);
    pub const CTRL: ChordModifiers = ChordModifiers(1);
    pub const SHIFT: ChordModifiers = ChordModifiers(2);
    pub const ALT: ChordModifiers = ChordModifiers(4);
    pub const SUPER: ChordModifiers = ChordModifiers(8);

    pub fn contains(self, other: ChordModifiers) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for ChordModifiers {
    type Output = ChordModifiers;
    fn bitor(self, rhs: ChordModifiers) -> ChordModifiers {
        ChordModifiers(self.0 | rhs.0)
    }
}

/// A platform-neutral key chord: a modifier set plus a numeric Windows virtual-key value
/// (e.g. `0x44` = 'D', `0x25` = Left) — the frontend maps `KeyboardEvent.code` onto these.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyChord {
    pub modifiers: ChordModifiers,
    pub key_code: i32,
}

impl KeyChord {
    pub fn new(modifiers: ChordModifiers, key_code: i32) -> Self {
        Self {
            modifiers,
            key_code,
        }
    }
}

/// The workspace actions reachable by keyboard (R8) — port of the macOS bonsplit shortcut set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShortcutAction {
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    NextTab,
    PreviousTab,
    NewTab,
    CloseTab,
    SplitRight,
    SplitDown,
    Equalize,
    ToggleZoom,
}

/// Numeric Windows virtual-key codes used by the default chords (stable Win32 VK_* values).
mod vkey {
    pub const TAB: i32 = 0x09;
    pub const LEFT: i32 = 0x25;
    pub const UP: i32 = 0x26;
    pub const RIGHT: i32 = 0x27;
    pub const DOWN: i32 = 0x28;
    pub const D0: i32 = 0x30;
    pub const D: i32 = 0x44;
    pub const E: i32 = 0x45;
    pub const T: i32 = 0x54;
    pub const W: i32 = 0x57;
    pub const Z: i32 = 0x5A;
}

/// The default chord→action bindings. All sit in the Ctrl(+Shift) namespace so they never shadow a
/// plain key headed for the terminal; Ctrl+Shift+C/V (owned by the terminal surface) are avoided.
pub fn defaults() -> Vec<(KeyChord, ShortcutAction)> {
    let ctrl = ChordModifiers::CTRL;
    let ctrl_shift = ChordModifiers::CTRL | ChordModifiers::SHIFT;
    vec![
        (
            KeyChord::new(ctrl_shift, vkey::LEFT),
            ShortcutAction::FocusLeft,
        ),
        (
            KeyChord::new(ctrl_shift, vkey::RIGHT),
            ShortcutAction::FocusRight,
        ),
        (KeyChord::new(ctrl_shift, vkey::UP), ShortcutAction::FocusUp),
        (
            KeyChord::new(ctrl_shift, vkey::DOWN),
            ShortcutAction::FocusDown,
        ),
        (KeyChord::new(ctrl, vkey::TAB), ShortcutAction::NextTab),
        (
            KeyChord::new(ctrl_shift, vkey::TAB),
            ShortcutAction::PreviousTab,
        ),
        (KeyChord::new(ctrl_shift, vkey::T), ShortcutAction::NewTab),
        (KeyChord::new(ctrl_shift, vkey::W), ShortcutAction::CloseTab),
        (
            KeyChord::new(ctrl_shift, vkey::D),
            ShortcutAction::SplitRight,
        ),
        (
            KeyChord::new(ctrl_shift, vkey::E),
            ShortcutAction::SplitDown,
        ),
        (
            KeyChord::new(ctrl_shift, vkey::D0),
            ShortcutAction::Equalize,
        ),
        (
            KeyChord::new(ctrl_shift, vkey::Z),
            ShortcutAction::ToggleZoom,
        ),
    ]
}

/// Resolve a chord to its action, or `None` if unbound.
pub fn resolve(modifiers: ChordModifiers, key_code: i32) -> Option<ShortcutAction> {
    let chord = KeyChord::new(modifiers, key_code);
    defaults()
        .into_iter()
        .find(|(k, _)| *k == chord)
        .map(|(_, a)| a)
}

/// Describe the default chord bound to `action` as a display string (e.g. "Ctrl+Shift+D"), sourced
/// from [`defaults`] so tooltip text never drifts from the live bindings (R3). Empty if unbound.
pub fn describe_chord(action: ShortcutAction) -> String {
    defaults()
        .into_iter()
        .find(|(_, a)| *a == action)
        .map(|(chord, _)| describe(chord))
        .unwrap_or_default()
}

/// Format a chord as `Mod(+Mod)+Key` with a stable modifier order.
fn describe(chord: KeyChord) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(4);
    if chord.modifiers.contains(ChordModifiers::CTRL) {
        parts.push("Ctrl".to_string());
    }
    if chord.modifiers.contains(ChordModifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    if chord.modifiers.contains(ChordModifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if chord.modifiers.contains(ChordModifiers::SUPER) {
        parts.push("Super".to_string());
    }
    parts.push(key_name(chord.key_code));
    parts.join("+")
}

/// Display name for a virtual-key code used by the default table.
fn key_name(key_code: i32) -> String {
    match key_code {
        vkey::TAB => "Tab".to_string(),
        vkey::LEFT => "Left".to_string(),
        vkey::UP => "Up".to_string(),
        vkey::RIGHT => "Right".to_string(),
        vkey::DOWN => "Down".to_string(),
        vkey::D0 => "0".to_string(),
        vkey::D => "D".to_string(),
        vkey::E => "E".to_string(),
        vkey::T => "T".to_string(),
        vkey::W => "W".to_string(),
        vkey::Z => "Z".to_string(),
        other => format!("0x{other:02X}"),
    }
}

/// Apply `action` to the focused pane/surface of `controller`. Operations with no target
/// (e.g. close with no focused surface) are silent no-ops.
pub fn apply(controller: &mut SplitTreeController, action: ShortcutAction) {
    match action {
        ShortcutAction::FocusLeft => controller.move_focus(Direction::Left),
        ShortcutAction::FocusRight => controller.move_focus(Direction::Right),
        ShortcutAction::FocusUp => controller.move_focus(Direction::Up),
        ShortcutAction::FocusDown => controller.move_focus(Direction::Down),
        ShortcutAction::NextTab => controller.select_next_tab(),
        ShortcutAction::PreviousTab => controller.select_previous_tab(),
        ShortcutAction::NewTab => {
            controller.new_tab(controller.focused_pane());
        }
        ShortcutAction::CloseTab => {
            if let Some(surface) = controller.focused_surface() {
                controller.close_tab(surface);
            }
        }
        ShortcutAction::SplitRight => {
            controller.split(controller.focused_pane(), Orientation::Vertical, false);
        }
        ShortcutAction::SplitDown => {
            controller.split(controller.focused_pane(), Orientation::Horizontal, false);
        }
        ShortcutAction::Equalize => controller.equalize(),
        ShortcutAction::ToggleZoom => controller.toggle_zoom(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ---- SidebarProjection ---------------------------------------------------------------------

    #[test]
    fn sidebar_projects_one_row_per_workspace_marking_the_selected_one() {
        let mut m = WorkspaceManager::new();
        let a = m.selected_id();
        let b = m.new_workspace();

        let rows = project_sidebar(&m, None, None);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, a);
        assert!(!rows[0].is_selected);
        assert_eq!(rows[1].id, b);
        assert!(rows[1].is_selected);
    }

    #[test]
    fn sidebar_row_carries_git_pr_cwd_and_progress_metadata() {
        let mut m = WorkspaceManager::new();
        let surface = m.selected().controller().all_surfaces()[0];
        m.report_git_branch(surface, "feat/sidebar", true);
        m.report_pr(
            surface,
            "42",
            "Add sidebar",
            "open",
            Some("feat/sidebar"),
            false,
        );
        m.report_pwd(surface, r"C:\dev\x");
        m.selected_mut().set_progress(Some("3/5"));
        m.selected_mut().set_status("claude:busy");

        let row = project_sidebar(&m, None, None).remove(0);

        assert_eq!(row.git_branch.as_deref(), Some("feat/sidebar"));
        assert!(row.git_dirty);
        assert_eq!(row.pr_badge.as_deref(), Some("#42 open"));
        assert_eq!(row.pr_status.as_deref(), Some("open"));
        assert_eq!(row.cwd.as_deref(), Some(r"C:\dev\x"));
        assert_eq!(row.progress.as_deref(), Some("3/5"));
        assert_eq!(row.status.as_deref(), Some("claude busy"));
    }

    #[test]
    fn sidebar_unreported_metadata_projects_as_none() {
        let m = WorkspaceManager::new();

        let row = project_sidebar(&m, None, None).remove(0);

        assert!(row.git_branch.is_none());
        assert!(!row.git_dirty);
        assert!(row.pr_badge.is_none());
        assert!(row.cwd.is_none());
        assert!(row.status.is_none());
        assert!(row.progress.is_none());
        assert!(row.latest_text.is_none());
        assert_eq!(row.unread_count, 0);
    }

    #[test]
    fn sidebar_unread_and_latest_come_from_the_supplied_callbacks() {
        let mut m = WorkspaceManager::new();
        let a = m.selected_id();
        m.new_workspace();

        let unread = |id: WorkspaceId| if id == a { 3 } else { 0 };
        let latest = |id: WorkspaceId| {
            if id == a {
                Some("Build finished".to_string())
            } else {
                None
            }
        };
        let rows = project_sidebar(&m, Some(&unread), Some(&latest));

        assert_eq!(rows[0].unread_count, 3);
        assert_eq!(rows[0].latest_text.as_deref(), Some("Build finished"));
        assert_eq!(rows[1].unread_count, 0);
        assert!(rows[1].latest_text.is_none());
    }

    #[test]
    fn sidebar_title_falls_back_from_custom_to_derived_to_id() {
        let mut m = WorkspaceManager::new();
        let id = m.selected_id();

        assert_eq!(project_sidebar(&m, None, None)[0].title, id.to_string());

        m.selected_mut().set_title("pwsh");
        assert_eq!(project_sidebar(&m, None, None)[0].title, "pwsh");

        m.selected_mut().set_custom_title(Some("backend"));
        assert_eq!(project_sidebar(&m, None, None)[0].title, "backend");
    }

    #[test]
    fn sidebar_home_directory_paths_abbreviate_to_tilde() {
        let mut m = WorkspaceManager::new();
        let home = std::env::var("USERPROFILE").expect("USERPROFILE set on Windows");
        let surface = m.selected().controller().all_surfaces()[0];
        m.report_pwd(surface, &format!(r"{home}\dev"));

        assert_eq!(
            project_sidebar(&m, None, None)[0].cwd.as_deref(),
            Some(r"~\dev")
        );
    }

    #[test]
    fn sidebar_pr_number_keeps_an_existing_hash_prefix() {
        let mut m = WorkspaceManager::new();
        let surface = m.selected().controller().all_surfaces()[0];
        m.report_pr(surface, "#7", "", "merged", None, false);

        assert_eq!(
            project_sidebar(&m, None, None)[0].pr_badge.as_deref(),
            Some("#7 merged")
        );
    }

    // ---- TabHeaderProjection -------------------------------------------------------------------

    fn s(n: i32) -> SurfaceId {
        SurfaceId(n)
    }

    #[test]
    fn tabs_project_marks_exactly_the_selected_tab() {
        let tabs = vec![s(1), s(2), s(3)];

        let headers = project_tab_headers(&tabs, s(2), None, None);

        assert_eq!(headers.len(), 3);
        assert_eq!(headers.iter().filter(|h| h.is_selected).count(), 1);
        assert!(headers.iter().find(|h| h.is_selected).unwrap().id == s(2));
        assert_eq!(
            headers.iter().map(|h| h.id).collect::<Vec<_>>(),
            vec![s(1), s(2), s(3)]
        );
    }

    #[test]
    fn tabs_use_the_title_provider_when_it_returns_a_value() {
        let tabs = vec![s(1), s(2)];
        let titles: HashMap<SurfaceId, String> =
            [(s(1), "pwsh".to_string()), (s(2), "vim".to_string())]
                .into_iter()
                .collect();

        let title_of = |id: SurfaceId| titles.get(&id).cloned();
        let headers = project_tab_headers(&tabs, s(1), Some(&title_of), None);

        assert_eq!(headers[0].title, "pwsh");
        assert_eq!(headers[1].title, "vim");
    }

    #[test]
    fn tabs_fall_back_to_the_id_when_no_title() {
        for provided in [None, Some(String::new())] {
            let tabs = vec![s(7)];
            let title_of = |_: SurfaceId| provided.clone();
            let headers = project_tab_headers(&tabs, s(7), Some(&title_of), None);
            assert_eq!(headers[0].title, s(7).to_string());
        }
    }

    #[test]
    fn tabs_fall_back_to_the_id_when_provider_is_none() {
        let tabs = vec![s(9)];
        let headers = project_tab_headers(&tabs, s(9), None, None);
        assert_eq!(headers[0].title, s(9).to_string());
    }

    #[test]
    fn tabs_of_empty_list_is_empty() {
        let headers = project_tab_headers(&[], s(0), None, None);
        assert!(headers.is_empty());
    }

    #[test]
    fn tabs_mark_unread_only_for_surfaces_the_lookup_reports() {
        let tabs = vec![s(1), s(2), s(3)];
        let is_unread = |id: SurfaceId| id == s(2);
        let headers = project_tab_headers(&tabs, s(1), None, Some(&is_unread));

        assert!(!headers[0].unread);
        assert!(headers[1].unread);
        assert!(!headers[2].unread);
    }

    #[test]
    fn tabs_set_selected_and_unread_independently() {
        let tabs = vec![s(5)];
        let is_unread = |_: SurfaceId| true;
        let headers = project_tab_headers(&tabs, s(5), None, Some(&is_unread));

        assert!(headers[0].is_selected);
        assert!(headers[0].unread);
    }

    #[test]
    fn tabs_without_unread_lookup_mark_nothing_unread() {
        let tabs = vec![s(1), s(2)];
        let headers = project_tab_headers(&tabs, s(1), None, None);
        assert!(headers.iter().all(|h| !h.unread));
    }

    // ---- ShortcutMap: the chord table ----------------------------------------------------------

    const CTRL_SHIFT: ChordModifiers = ChordModifiers(3); // CTRL | SHIFT

    #[test]
    fn resolve_maps_ctrl_shift_chords() {
        let cases = [
            (0x25, ShortcutAction::FocusLeft),
            (0x27, ShortcutAction::FocusRight),
            (0x26, ShortcutAction::FocusUp),
            (0x28, ShortcutAction::FocusDown),
            (0x54, ShortcutAction::NewTab),
            (0x57, ShortcutAction::CloseTab),
            (0x44, ShortcutAction::SplitRight),
            (0x45, ShortcutAction::SplitDown),
            (0x30, ShortcutAction::Equalize),
            (0x5A, ShortcutAction::ToggleZoom),
        ];
        for (key, expected) in cases {
            assert_eq!(resolve(CTRL_SHIFT, key), Some(expected));
        }
    }

    #[test]
    fn resolve_distinguishes_tab_cycle_by_shift() {
        assert_eq!(
            resolve(ChordModifiers::CTRL, 0x09),
            Some(ShortcutAction::NextTab)
        );
        assert_eq!(resolve(CTRL_SHIFT, 0x09), Some(ShortcutAction::PreviousTab));
    }

    #[test]
    fn resolve_returns_none_for_an_unbound_chord() {
        assert_eq!(resolve(ChordModifiers::NONE, 0x44), None); // plain 'D'
        assert_eq!(resolve(ChordModifiers::CTRL, 0x43), None); // Ctrl+C — owned by the terminal
    }

    #[test]
    fn defaults_cover_every_action_with_no_duplicate_chords() {
        let d = defaults();
        // 12 distinct actions, each reachable exactly once.
        assert_eq!(d.len(), 12);
        let mut unique_actions = d.iter().map(|(_, a)| format!("{a:?}")).collect::<Vec<_>>();
        unique_actions.sort();
        unique_actions.dedup();
        assert_eq!(unique_actions.len(), 12);
        let mut unique_chords = d.iter().map(|(k, _)| *k).collect::<Vec<_>>();
        unique_chords.sort_by_key(|c| (c.modifiers.0, c.key_code));
        unique_chords.dedup();
        assert_eq!(unique_chords.len(), d.len());
    }

    // ---- ShortcutMap: the dispatcher -----------------------------------------------------------

    #[test]
    fn apply_split_right_adds_a_side_by_side_pane_and_focuses_it() {
        let mut c = SplitTreeController::new();
        let original = c.focused_pane();

        apply(&mut c, ShortcutAction::SplitRight);

        assert_eq!(c.all_pane_ids().len(), 2);
        assert_ne!(original, c.focused_pane());
    }

    #[test]
    fn apply_new_then_close_tab_round_trips_the_focused_pane() {
        let mut c = SplitTreeController::new();
        let pane = c.focused_pane();
        assert_eq!(c.tabs(pane).len(), 1);

        apply(&mut c, ShortcutAction::NewTab);
        assert_eq!(c.tabs(pane).len(), 2);

        apply(&mut c, ShortcutAction::CloseTab);
        assert_eq!(c.tabs(pane).len(), 1);
    }

    #[test]
    fn apply_directional_focus_moves_across_a_split_and_back() {
        let mut c = SplitTreeController::new();
        let left = c.focused_pane();
        apply(&mut c, ShortcutAction::SplitRight);
        let right = c.focused_pane();
        assert_ne!(left, right);

        apply(&mut c, ShortcutAction::FocusLeft);
        assert_eq!(left, c.focused_pane());

        apply(&mut c, ShortcutAction::FocusRight);
        assert_eq!(right, c.focused_pane());
    }

    #[test]
    fn apply_toggle_zoom_flips_the_zoomed_pane() {
        let mut c = SplitTreeController::new();

        assert!(c.zoomed_pane().is_none());
        apply(&mut c, ShortcutAction::ToggleZoom);
        assert_eq!(c.zoomed_pane(), Some(c.focused_pane()));
        apply(&mut c, ShortcutAction::ToggleZoom);
        assert!(c.zoomed_pane().is_none());
    }

    // ---- ShortcutMap: chord description (tooltip text) -----------------------------------------

    #[test]
    fn describe_chord_formats_the_bound_chord() {
        let cases = [
            (ShortcutAction::SplitRight, "Ctrl+Shift+D"),
            (ShortcutAction::SplitDown, "Ctrl+Shift+E"),
            (ShortcutAction::ToggleZoom, "Ctrl+Shift+Z"),
            (ShortcutAction::NewTab, "Ctrl+Shift+T"),
            (ShortcutAction::CloseTab, "Ctrl+Shift+W"),
            (ShortcutAction::Equalize, "Ctrl+Shift+0"),
            (ShortcutAction::FocusLeft, "Ctrl+Shift+Left"),
            (ShortcutAction::FocusDown, "Ctrl+Shift+Down"),
        ];
        for (action, expected) in cases {
            assert_eq!(describe_chord(action), expected);
        }
    }

    #[test]
    fn describe_chord_uses_a_single_modifier_when_the_chord_has_one() {
        assert_eq!(describe_chord(ShortcutAction::NextTab), "Ctrl+Tab");
        assert_eq!(
            describe_chord(ShortcutAction::PreviousTab),
            "Ctrl+Shift+Tab"
        );
    }

    #[test]
    fn describe_chord_returns_a_nonempty_string_for_every_action() {
        for (_, action) in defaults() {
            assert!(
                !describe_chord(action).is_empty(),
                "no chord text for {action:?}"
            );
        }
    }

    #[test]
    fn describe_chord_orders_modifiers_ctrl_before_shift() {
        assert!(describe_chord(ShortcutAction::SplitRight).starts_with("Ctrl+Shift+"));
    }
}
