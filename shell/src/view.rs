//! Frontend-facing view DTOs. The domain's `TreeSnapshot` is an `Arc`-linked recursive tree
//! (`!Send`, not serde-friendly); the frontend wants a flat, serializable layout it can render
//! as absolutely-positioned panes without re-walking a tree in JS. This module flattens a
//! snapshot into normalized [0,1] rectangles — pane rects (what backs each terminal) and divider
//! handles (draggable splitters) — computed by one walk that mirrors `layout::compute_leaf_rects`.

use serde::Serialize;

use crate::domain::capacity::{CapacityIndicatorViewModel, CapacityLevel, CapacityState};
use crate::domain::layout::{compute_leaf_rects, LayoutRect};
use crate::domain::projections::project_sidebar;
use crate::domain::split_tree::{Orientation, SplitNode, TreeSnapshot};
use crate::domain::workspace::WorkspaceManager;

/// A normalized rectangle in [0,1] space; the frontend scales it to the viewport with percentages.
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
pub struct RectView {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<LayoutRect> for RectView {
    fn from(r: LayoutRect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }
    }
}

/// One terminal pane: its id, the tabs it holds, which surface is selected/visible, its rectangle,
/// and transient focus/zoom flags the chrome styles on.
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct PaneView {
    pub pane_id: i32,
    pub tabs: Vec<i32>,
    pub selected: i32,
    pub rect: RectView,
    pub focused: bool,
    pub zoomed: bool,
}

/// One draggable divider: the branch it adjusts, its orientation, current fraction, and the rect
/// of the branch region it sits inside (so the frontend positions the handle line).
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
pub struct DividerView {
    pub branch_id: i32,
    /// `"vertical"` = side-by-side panes (a vertical divider line); `"horizontal"` = stacked.
    pub orientation: &'static str,
    pub fraction: f64,
    pub rect: RectView,
}

/// The whole layout the frontend renders: flat panes + dividers, plus the focus/zoom/version the
/// chrome needs. `version` lets the frontend drop a stale render if snapshots arrive out of order.
#[derive(Serialize, Clone, PartialEq, Debug, Default)]
pub struct TreeView {
    pub panes: Vec<PaneView>,
    pub dividers: Vec<DividerView>,
    pub focused_pane: i32,
    pub zoomed_pane: Option<i32>,
    pub version: i32,
}

/// Flatten a domain snapshot into the frontend view. Pane rectangles come from the tested
/// `compute_leaf_rects`; the divider walk reuses the same split math so a handle always sits on
/// the boundary between the two child rects.
pub fn build_tree_view(snapshot: &TreeSnapshot) -> TreeView {
    let (panes, dividers) = match &snapshot.root {
        None => (Vec::new(), Vec::new()),
        Some(root) => {
            let bounds = LayoutRect::new(0.0, 0.0, 1.0, 1.0);
            let rects = compute_leaf_rects(root, bounds);

            let mut panes = Vec::new();
            for leaf in root.leaves() {
                let rect = rects
                    .get(&leaf.id)
                    .copied()
                    .unwrap_or_else(|| LayoutRect::new(0.0, 0.0, 0.0, 0.0));
                panes.push(PaneView {
                    pane_id: leaf.id.0,
                    tabs: leaf.tabs.iter().map(|s| s.0).collect(),
                    selected: leaf.selected.0,
                    rect: rect.into(),
                    focused: leaf.id == snapshot.focused_pane,
                    zoomed: snapshot.zoomed_pane == Some(leaf.id),
                });
            }

            let mut dividers = Vec::new();
            collect_dividers(root, bounds, &mut dividers);
            (panes, dividers)
        }
    };

    TreeView {
        panes,
        dividers,
        focused_pane: snapshot.focused_pane.0,
        zoomed_pane: snapshot.zoomed_pane.map(|p| p.0),
        version: snapshot.version,
    }
}

/// The always-visible capacity indicator (CLAUDE.md thesis): the RAM safe-zone meter the chrome
/// binds to. Presentation mirrors the ported [`CapacityIndicatorViewModel`] (label/fraction/level)
/// — that view model is the source of truth; `capacity_view_matches_the_view_model` guards parity.
#[derive(Serialize, Clone, PartialEq, Debug, Default)]
pub struct CapacityView {
    pub used: i32,
    pub reserved: i32,
    pub max: i32,
    /// `"calm" | "warn" | "cap"` → the `--capacity-*` token levels in tokens.css.
    pub level: &'static str,
    /// Fill fraction [0,1] on the `used + reserved` basis (the 8-pip meter).
    pub fraction: f64,
    /// `"X / Y terminals"` (X = used + reserved).
    pub label: String,
    /// At the safe-zone cap — the chrome disables the New-Workspace affordance.
    pub at_cap: bool,
    /// Cap hint line, or `None` below the cap.
    pub hint: Option<&'static str>,
}

fn level_token(level: CapacityLevel) -> &'static str {
    match level {
        CapacityLevel::Calm => "calm",
        CapacityLevel::Warn => "warn",
        CapacityLevel::Cap => "cap",
    }
}

/// Build the capacity meter DTO from the model's current [`CapacityState`] (`None` = the governor
/// failed to start; the meter shows an em-dash placeholder, same as the view model).
pub fn build_capacity_view(state: Option<CapacityState>) -> CapacityView {
    let Some(s) = state else {
        return CapacityView {
            level: "calm",
            label: "— / — terminals".to_string(),
            ..Default::default()
        };
    };
    let filled = s.used + s.reserved;
    let fraction = if s.max > 0 {
        (f64::from(filled) / f64::from(s.max)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let at_cap = s.level == CapacityLevel::Cap;
    CapacityView {
        used: s.used,
        reserved: s.reserved,
        max: s.max,
        level: level_token(s.level),
        fraction,
        label: format!("{filled} / {} terminals", s.max),
        at_cap,
        hint: at_cap.then_some(CapacityIndicatorViewModel::CAP_HINT),
    }
}

/// One sidebar row: a workspace's identity + live git/PR/status projection. A flat, serde-friendly
/// mirror of the ported [`project_sidebar`] [`SidebarRow`](crate::domain::projections::SidebarRow)
/// (which carries `!Serialize` domain ids); this is the wire shape the chrome renders.
#[derive(Serialize, Clone, PartialEq, Debug, Default)]
pub struct SidebarRowView {
    pub id: i32,
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

/// Project every workspace into a sidebar row. Notification feeds (`unread`/`latest`) are left at
/// their defaults for now — the per-workspace coordinator wiring lands with the toasts unit.
pub fn build_sidebar(manager: &WorkspaceManager) -> Vec<SidebarRowView> {
    project_sidebar(manager, None, None)
        .into_iter()
        .map(|r| SidebarRowView {
            id: r.id.0,
            title: r.title,
            is_selected: r.is_selected,
            git_branch: r.git_branch,
            git_dirty: r.git_dirty,
            pr_badge: r.pr_badge,
            pr_status: r.pr_status,
            cwd: r.cwd,
            status: r.status,
            progress: r.progress,
            latest_text: r.latest_text,
            unread_count: r.unread_count,
        })
        .collect()
}

/// Walk the tree recording one [`DividerView`] per branch, splitting `rect` exactly as
/// `layout::compute_leaf_rects` does so the handle lands on the child boundary.
fn collect_dividers(node: &SplitNode, rect: LayoutRect, out: &mut Vec<DividerView>) {
    let SplitNode::Branch(branch) = node else {
        return;
    };
    let f = branch.divider_position.clamp(0.0, 1.0);
    let (first_rect, second_rect, orientation) = if branch.orientation == Orientation::Vertical {
        let w1 = rect.width * f;
        (
            LayoutRect::new(rect.x, rect.y, w1, rect.height),
            LayoutRect::new(rect.x + w1, rect.y, rect.width - w1, rect.height),
            "vertical",
        )
    } else {
        let h1 = rect.height * f;
        (
            LayoutRect::new(rect.x, rect.y, rect.width, h1),
            LayoutRect::new(rect.x, rect.y + h1, rect.width, rect.height - h1),
            "horizontal",
        )
    };
    out.push(DividerView {
        branch_id: branch.id.0,
        orientation,
        fraction: f,
        rect: rect.into(),
    });
    collect_dividers(&branch.first, first_rect, out);
    collect_dividers(&branch.second, second_rect, out);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domain::ids::{BranchId, PaneId, SurfaceId};
    use crate::domain::split_tree::{PaneLeaf, SplitBranch};

    fn pane(id: i32) -> Arc<SplitNode> {
        Arc::new(SplitNode::Leaf(PaneLeaf {
            id: PaneId(id),
            tabs: vec![SurfaceId(id)],
            selected: SurfaceId(id),
        }))
    }

    fn snapshot(root: Option<Arc<SplitNode>>, focused: i32, zoomed: Option<i32>) -> TreeSnapshot {
        TreeSnapshot {
            root,
            focused_pane: PaneId(focused),
            zoomed_pane: zoomed.map(PaneId),
            version: 7,
        }
    }

    // ---- Capacity meter --------------------------------------------------------------------

    #[test]
    fn capacity_view_formats_label_fraction_and_level() {
        let state = CapacityState::new(3, 1, 8, CapacityLevel::Warn);
        let view = build_capacity_view(Some(state));
        assert_eq!("4 / 8 terminals", view.label); // used + reserved
        assert_eq!(0.5, view.fraction);
        assert_eq!("warn", view.level);
        assert!(!view.at_cap);
        assert_eq!(None, view.hint);
    }

    #[test]
    fn capacity_view_at_cap_carries_the_hint() {
        let state = CapacityState::new(8, 0, 8, CapacityLevel::Cap);
        let view = build_capacity_view(Some(state));
        assert_eq!(1.0, view.fraction);
        assert!(view.at_cap);
        assert_eq!(Some(CapacityIndicatorViewModel::CAP_HINT), view.hint);
    }

    #[test]
    fn capacity_view_without_a_model_shows_the_placeholder() {
        let view = build_capacity_view(None);
        assert_eq!("— / — terminals", view.label);
        assert_eq!(0.0, view.fraction);
        assert!(!view.at_cap);
    }

    #[test]
    fn empty_tree_flattens_to_no_panes() {
        let view = build_tree_view(&snapshot(None, 0, None));
        assert!(view.panes.is_empty());
        assert!(view.dividers.is_empty());
        assert_eq!(7, view.version);
    }

    #[test]
    fn single_pane_fills_the_bounds_and_carries_focus() {
        let view = build_tree_view(&snapshot(Some(pane(1)), 1, None));
        assert_eq!(1, view.panes.len());
        let p = &view.panes[0];
        assert_eq!(
            RectView {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0
            },
            p.rect
        );
        assert!(p.focused);
        assert!(!p.zoomed);
        assert!(view.dividers.is_empty());
    }

    #[test]
    fn vertical_branch_yields_two_side_by_side_panes_and_one_divider() {
        let root = Arc::new(SplitNode::Branch(SplitBranch {
            id: BranchId(1),
            orientation: Orientation::Vertical,
            first: pane(1),
            second: pane(2),
            divider_position: 0.5,
        }));
        let view = build_tree_view(&snapshot(Some(root), 2, Some(2)));

        assert_eq!(2, view.panes.len());
        let left = view.panes.iter().find(|p| p.pane_id == 1).unwrap();
        let right = view.panes.iter().find(|p| p.pane_id == 2).unwrap();
        assert_eq!(
            RectView {
                x: 0.0,
                y: 0.0,
                width: 0.5,
                height: 1.0
            },
            left.rect
        );
        assert_eq!(
            RectView {
                x: 0.5,
                y: 0.0,
                width: 0.5,
                height: 1.0
            },
            right.rect
        );

        assert_eq!(1, view.dividers.len());
        assert_eq!("vertical", view.dividers[0].orientation);
        assert_eq!(BranchId(1).0, view.dividers[0].branch_id);

        // Zoom + focus flags surface on the right pane.
        assert!(right.focused && right.zoomed);
        assert!(!left.focused && !left.zoomed);
        assert_eq!(Some(2), view.zoomed_pane);
    }
}
