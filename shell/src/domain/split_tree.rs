//! Port of core/Splits/SplitTree.cs + SplitTreeController.cs — the immutable split-tree node
//! model plus the mutable controller over it (KTD3). Operations never mutate a node; they build
//! new nodes and the controller swaps its root, which is what makes a captured [`TreeSnapshot`]
//! stable (R12). C# events are ported as returned [`SplitTreeEvent`]s: the host inspects the
//! `Vec` an operation returns instead of subscribing to `SurfaceCreated`/`SurfaceClosed`/`Emptied`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::domain::ids::{BranchId, IdAllocator, PaneId, SurfaceId};
use crate::domain::layout;

/// Split orientation. The convention is stated explicitly here (KTD7) to avoid the perennial
/// axis confusion:
/// - [`Orientation::Vertical`] — panes sit **side-by-side**, separated by a **vertical**
///   divider. This is the "split right" action.
/// - [`Orientation::Horizontal`] — panes are **stacked**, separated by a **horizontal**
///   divider. This is the "split down" action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

/// Direction for keyboard pane-focus navigation (R8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// A leaf pane: an ordered tab list of surfaces plus the currently selected surface. The
/// selected surface is the only one composited for this pane (R3/R11). Closing the leaf's last
/// tab removes the pane and heals the tree (R6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaneLeaf {
    pub id: PaneId,
    pub tabs: Vec<SurfaceId>,
    pub selected: SurfaceId,
}

/// An interior split: two children separated by a divider. `divider_position` is the first
/// child's share, a fraction in [0,1] clamped on every write (KTD7); the second child gets
/// `1 - divider_position`.
#[derive(Clone, Debug)]
pub struct SplitBranch {
    pub id: BranchId,
    pub orientation: Orientation,
    pub first: Arc<SplitNode>,
    pub second: Arc<SplitNode>,
    pub divider_position: f64,
}

/// An immutable node in the split tree — either a [`PaneLeaf`] or a [`SplitBranch`].
#[derive(Clone, Debug)]
pub enum SplitNode {
    Leaf(PaneLeaf),
    Branch(SplitBranch),
}

impl SplitNode {
    pub fn as_leaf(&self) -> Option<&PaneLeaf> {
        match self {
            SplitNode::Leaf(leaf) => Some(leaf),
            SplitNode::Branch(_) => None,
        }
    }

    pub fn as_branch(&self) -> Option<&SplitBranch> {
        match self {
            SplitNode::Branch(branch) => Some(branch),
            SplitNode::Leaf(_) => None,
        }
    }

    /// Every leaf pane in document order (first-before-second, depth-first).
    pub fn leaves(&self) -> Vec<&PaneLeaf> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves<'a>(&'a self, out: &mut Vec<&'a PaneLeaf>) {
        match self {
            SplitNode::Leaf(leaf) => out.push(leaf),
            SplitNode::Branch(branch) => {
                branch.first.collect_leaves(out);
                branch.second.collect_leaves(out);
            }
        }
    }

    /// The leaf with the given [`PaneId`], or `None`.
    pub fn find_pane(&self, id: PaneId) -> Option<&PaneLeaf> {
        self.leaves().into_iter().find(|l| l.id == id)
    }

    /// The leaf whose tab list contains `surface`, or `None`.
    pub fn find_containing(&self, surface: SurfaceId) -> Option<&PaneLeaf> {
        self.leaves()
            .into_iter()
            .find(|l| l.tabs.contains(&surface))
    }
}

/// An immutable value-type snapshot of the controller state handed to the view (KTD3/KTD5).
/// Because `root` is an immutable node tree, a captured snapshot is unaffected by subsequent
/// controller mutations. `root` is `None` when the tree is empty (the host re-seeds — see
/// [`SplitTreeController::seed_root`]).
#[derive(Clone, Debug)]
pub struct TreeSnapshot {
    pub root: Option<Arc<SplitNode>>,
    pub focused_pane: PaneId,
    pub zoomed_pane: Option<PaneId>,
    pub version: i32,
}

impl TreeSnapshot {
    /// The focused pane's selected surface, or `None` when the focused pane is absent from
    /// this snapshot (e.g. an empty tree). Mirrors [`SplitTreeController::focused_surface`]
    /// exactly (KTD6).
    pub fn focused_surface(&self) -> Option<SurfaceId> {
        self.root
            .as_ref()?
            .find_pane(self.focused_pane)
            .map(|l| l.selected)
    }
}

/// Surface-lifecycle effect of a controller operation. The C# controller raised these as
/// events (`SurfaceCreated` / `SurfaceClosed` / `Emptied`); here the mutating operations
/// return them so the host can create/dispose the matching engine (R2/R6/R9).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitTreeEvent {
    /// An operation introduced a new surface; the host creates its engine.
    SurfaceCreated(SurfaceId),
    /// A surface was removed; the host disposes its engine (R2/R9).
    SurfaceClosed(SurfaceId),
    /// The last surface of the last pane closed and the tree became empty (R6). The host
    /// re-seeds via [`SplitTreeController::seed_root`] so the window never goes contentless.
    Emptied,
}

/// The mutable controller over an immutable split tree (KTD3). Holds the current root plus the
/// focused pane and exposes the split/tab/divider/focus operations (a port of the macOS
/// bonsplit controller).
///
/// **Derived focus (R7):** the focused *surface* is never stored — it is always
/// `selected_tab(focused_pane)`. Changing the selected tab changes the focused surface.
///
/// **Surface lifecycle** is delegated to the host: the controller knows only IDs and returns
/// [`SplitTreeEvent`]s from the operations that create or remove surfaces.
pub struct SplitTreeController {
    root: Option<Arc<SplitNode>>,
    focused_pane: PaneId,
    zoomed_pane: Option<PaneId>,
    ids: Rc<RefCell<IdAllocator>>,
    version: i32,
}

impl Default for SplitTreeController {
    fn default() -> Self {
        Self::new()
    }
}

impl SplitTreeController {
    /// Create a controller seeded with one pane holding one surface, that pane focused. The
    /// seed does **not** report events; the host reads [`Self::all_surfaces`] after
    /// construction to create the initial engine.
    pub fn new() -> Self {
        Self::with_allocator(Rc::new(RefCell::new(IdAllocator::new())))
    }

    /// As [`Self::new`], with a shared allocator so multiple workspaces mint globally unique
    /// surface ids (Phase 5).
    pub fn with_allocator(ids: Rc<RefCell<IdAllocator>>) -> Self {
        let (surface, pane_id) = {
            let mut alloc = ids.borrow_mut();
            let surface = alloc.next_surface();
            let pane_id = alloc.next_pane();
            (surface, pane_id)
        };
        let pane = PaneLeaf {
            id: pane_id,
            tabs: vec![surface],
            selected: surface,
        };
        Self {
            root: Some(Arc::new(SplitNode::Leaf(pane))),
            focused_pane: pane_id,
            zoomed_pane: None,
            ids,
            version: 0,
        }
    }

    // ---- Queries (derived; nothing cached) -------------------------------------------------

    /// `true` when there are no panes (closed the very last surface — R6).
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// The focused pane.
    pub fn focused_pane(&self) -> PaneId {
        self.focused_pane
    }

    /// The pane rendered full-bleed when zoomed (U7), or `None`.
    pub fn zoomed_pane(&self) -> Option<PaneId> {
        self.zoomed_pane
    }

    /// The focused surface — **derived** as `selected_tab(focused_pane)` (R7).
    pub fn focused_surface(&self) -> Option<SurfaceId> {
        self.root
            .as_ref()?
            .find_pane(self.focused_pane)
            .map(|l| l.selected)
    }

    /// Every pane id, in document order.
    pub fn all_pane_ids(&self) -> Vec<PaneId> {
        match &self.root {
            None => Vec::new(),
            Some(root) => root.leaves().iter().map(|l| l.id).collect(),
        }
    }

    /// Every surface id across all panes, in document order.
    pub fn all_surfaces(&self) -> Vec<SurfaceId> {
        match &self.root {
            None => Vec::new(),
            Some(root) => root
                .leaves()
                .iter()
                .flat_map(|l| l.tabs.iter().copied())
                .collect(),
        }
    }

    /// The tab list of a pane (empty if the pane does not exist).
    pub fn tabs(&self, pane: PaneId) -> Vec<SurfaceId> {
        self.root
            .as_ref()
            .and_then(|root| root.find_pane(pane))
            .map(|l| l.tabs.clone())
            .unwrap_or_default()
    }

    /// The selected surface of a pane, or `None` if the pane does not exist.
    pub fn selected_tab(&self, pane: PaneId) -> Option<SurfaceId> {
        self.root.as_ref()?.find_pane(pane).map(|l| l.selected)
    }

    /// Capture an immutable snapshot of the current state.
    pub fn snapshot(&self) -> TreeSnapshot {
        TreeSnapshot {
            root: self.root.clone(),
            focused_pane: self.focused_pane,
            zoomed_pane: self.zoomed_pane,
            version: self.version,
        }
    }

    // ---- Structural operations ---------------------------------------------------------------

    /// Split `pane` into a branch of the given `orientation`, adding a new pane with one new
    /// surface. The new pane becomes `first` when `insert_first` is set, else `second`; the new
    /// pane is focused (R4).
    pub fn split(
        &mut self,
        pane: PaneId,
        orientation: Orientation,
        insert_first: bool,
    ) -> Vec<SplitTreeEvent> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        if root.find_pane(pane).is_none() {
            return Vec::new();
        }

        let (surface, new_pane_id, branch_id) = {
            let mut alloc = self.ids.borrow_mut();
            let surface = alloc.next_surface();
            let new_pane_id = alloc.next_pane();
            let branch_id = alloc.next_branch();
            (surface, new_pane_id, branch_id)
        };
        let new_leaf = Arc::new(SplitNode::Leaf(PaneLeaf {
            id: new_pane_id,
            tabs: vec![surface],
            selected: surface,
        }));

        self.root = Some(rewrite_leaf(&root, pane, &mut |existing, _| {
            let (first, second) = if insert_first {
                (Arc::clone(&new_leaf), Arc::clone(existing))
            } else {
                (Arc::clone(existing), Arc::clone(&new_leaf))
            };
            Arc::new(SplitNode::Branch(SplitBranch {
                id: branch_id,
                orientation,
                first,
                second,
                divider_position: 0.5,
            }))
        }));
        self.focused_pane = new_pane_id;
        self.emit();
        vec![SplitTreeEvent::SurfaceCreated(surface)]
    }

    /// Add a new surface (tab) to `pane`, select it, and focus the pane (R1).
    pub fn new_tab(&mut self, pane: PaneId) -> Vec<SplitTreeEvent> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        if root.find_pane(pane).is_none() {
            return Vec::new();
        }

        let surface = self.ids.borrow_mut().next_surface();
        self.root = Some(rewrite_leaf(&root, pane, &mut |_, leaf| {
            let mut tabs = leaf.tabs.clone();
            tabs.push(surface);
            Arc::new(SplitNode::Leaf(PaneLeaf {
                id: leaf.id,
                tabs,
                selected: surface,
            }))
        }));
        self.focused_pane = pane;
        self.emit();
        vec![SplitTreeEvent::SurfaceCreated(surface)]
    }

    /// Close the tab backing `surface`. If the pane has other tabs, the surface is removed and
    /// selection moves to an adjacent tab (R2). If it was the pane's last tab, the pane is
    /// removed and the tree heals — the sibling is promoted into the parent's slot (R6) — and
    /// focus reconciles to a surviving pane. Closing the last surface of the last pane empties
    /// the tree and reports [`SplitTreeEvent::Emptied`].
    pub fn close_tab(&mut self, surface: SurfaceId) -> Vec<SplitTreeEvent> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        let Some(leaf) = root.find_containing(surface).cloned() else {
            return Vec::new();
        };

        if leaf.tabs.len() > 1 {
            let Some(idx) = leaf.tabs.iter().position(|&s| s == surface) else {
                return Vec::new();
            };
            let mut remaining = leaf.tabs.clone();
            remaining.remove(idx);
            let selected = if leaf.selected == surface {
                // Prefer the tab that shifts into the closed slot; fall back to the new last tab.
                remaining[idx.min(remaining.len() - 1)]
            } else {
                leaf.selected
            };
            self.root = Some(rewrite_leaf(&root, leaf.id, &mut |_, l| {
                Arc::new(SplitNode::Leaf(PaneLeaf {
                    id: l.id,
                    tabs: remaining.clone(),
                    selected,
                }))
            }));
            self.emit();
            return vec![SplitTreeEvent::SurfaceClosed(surface)];
        }

        // Last tab in the pane → remove the pane.
        let mut events = vec![SplitTreeEvent::SurfaceClosed(surface)];

        if let SplitNode::Leaf(root_leaf) = root.as_ref() {
            if root_leaf.id == leaf.id {
                // The root itself was the only pane → the tree is now empty.
                self.root = None;
                self.zoomed_pane = None;
                events.push(SplitTreeEvent::Emptied);
                return events;
            }
        }

        let (new_root, survivor) = remove_leaf_promoting_sibling(&root, leaf.id);
        if self.zoomed_pane == Some(leaf.id) {
            self.zoomed_pane = None;
        }
        if new_root.find_pane(self.focused_pane).is_none() {
            if let Some(id) = survivor.or_else(|| new_root.leaves().first().map(|l| l.id)) {
                self.focused_pane = id;
            }
        }
        self.root = Some(new_root);
        self.emit();
        events
    }

    /// Select `surface` within `pane` and focus the pane (R3).
    pub fn select_tab(&mut self, pane: PaneId, surface: SurfaceId) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(leaf) = root.find_pane(pane) else {
            return;
        };
        if !leaf.tabs.contains(&surface) {
            return;
        }
        if leaf.selected != surface {
            self.root = Some(rewrite_leaf(&root, pane, &mut |_, l| {
                Arc::new(SplitNode::Leaf(PaneLeaf {
                    selected: surface,
                    ..l.clone()
                }))
            }));
        }
        self.focused_pane = pane;
        self.emit();
    }

    /// Select the next tab in the focused pane, wrapping; never crosses panes (R7).
    pub fn select_next_tab(&mut self) {
        self.cycle_tab(1);
    }

    /// Select the previous tab in the focused pane, wrapping; never crosses panes (R7).
    pub fn select_previous_tab(&mut self) {
        self.cycle_tab(-1);
    }

    fn cycle_tab(&mut self, step: isize) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(leaf) = root.find_pane(self.focused_pane) else {
            return;
        };
        if leaf.tabs.len() <= 1 {
            return;
        }
        let n = leaf.tabs.len() as isize;
        let idx = leaf
            .tabs
            .iter()
            .position(|&s| s == leaf.selected)
            .unwrap_or(0) as isize;
        let next = (((idx + step) % n) + n) % n;
        let selected = leaf.tabs[next as usize];
        let pane = leaf.id;
        self.root = Some(rewrite_leaf(&root, pane, &mut |_, l| {
            Arc::new(SplitNode::Leaf(PaneLeaf {
                selected,
                ..l.clone()
            }))
        }));
        self.emit();
    }

    /// Focus a pane explicitly (e.g. a pointer click). No-op if the pane does not exist.
    pub fn focus_pane(&mut self, pane: PaneId) {
        let exists = self
            .root
            .as_ref()
            .is_some_and(|root| root.find_pane(pane).is_some());
        if !exists {
            return;
        }
        self.focused_pane = pane;
        self.emit();
    }

    /// Move focus to the nearest pane in `direction` (R8). No-op (no snapshot) when there is
    /// no pane that way.
    pub fn move_focus(&mut self, direction: Direction) {
        let target = match &self.root {
            None => return,
            Some(root) => layout::navigate_from(root, self.focused_pane, direction),
        };
        if let Some(t) = target {
            if t != self.focused_pane {
                self.focused_pane = t;
                self.emit();
            }
        }
    }

    /// Set a branch's divider fraction, clamped to [0,1] (R5/KTD7).
    pub fn set_divider_position(&mut self, branch: BranchId, fraction: f64) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let clamped = layout::clamp01(fraction);
        let (updated, changed) = set_divider(&root, branch, clamped);
        if changed {
            self.root = Some(updated);
            self.emit();
        }
    }

    /// Reset every branch divider to 0.5 (R5).
    pub fn equalize(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.root = Some(equalize_node(&root));
        self.emit();
    }

    /// Toggle full-bleed zoom of the focused pane (U7). Transient view state, not tree shape.
    pub fn toggle_zoom(&mut self) {
        if self.root.is_none() {
            return;
        }
        self.zoomed_pane = if self.zoomed_pane == Some(self.focused_pane) {
            None
        } else {
            Some(self.focused_pane)
        };
        self.emit();
    }

    /// Clear any active zoom (U7).
    pub fn clear_zoom(&mut self) {
        if self.zoomed_pane.is_none() {
            return;
        }
        self.zoomed_pane = None;
        self.emit();
    }

    /// Re-seed a fresh root pane + surface after the tree emptied (R6). Returns the new
    /// surface so the host can create its engine.
    pub fn seed_root(&mut self) -> SurfaceId {
        let (surface, pane_id) = {
            let mut alloc = self.ids.borrow_mut();
            let surface = alloc.next_surface();
            let pane_id = alloc.next_pane();
            (surface, pane_id)
        };
        self.root = Some(Arc::new(SplitNode::Leaf(PaneLeaf {
            id: pane_id,
            tabs: vec![surface],
            selected: surface,
        })));
        self.focused_pane = pane_id;
        self.zoomed_pane = None;
        self.emit();
        surface
    }

    fn emit(&mut self) {
        self.version += 1;
    }
}

// ---- Tree rewrites (pure, immutable) ---------------------------------------------------------

/// Rebuild the path from the root down to the leaf `target`, replacing that leaf with whatever
/// `f` returns (given the leaf's `Arc` and its data). Untouched subtrees are shared, so a
/// branch not on the path keeps its node — and its `BranchId` — unchanged.
fn rewrite_leaf(
    node: &Arc<SplitNode>,
    target: PaneId,
    f: &mut dyn FnMut(&Arc<SplitNode>, &PaneLeaf) -> Arc<SplitNode>,
) -> Arc<SplitNode> {
    match node.as_ref() {
        SplitNode::Leaf(leaf) => {
            if leaf.id == target {
                f(node, leaf)
            } else {
                Arc::clone(node)
            }
        }
        SplitNode::Branch(branch) => {
            let first = rewrite_leaf(&branch.first, target, f);
            let second = rewrite_leaf(&branch.second, target, f);
            if Arc::ptr_eq(&first, &branch.first) && Arc::ptr_eq(&second, &branch.second) {
                Arc::clone(node)
            } else {
                Arc::new(SplitNode::Branch(SplitBranch {
                    first,
                    second,
                    ..branch.clone()
                }))
            }
        }
    }
}

/// Remove the leaf `target` by promoting its sibling into the parent branch's slot (R6). The
/// returned survivor is the first leaf of the promoted subtree, used to reconcile focus when
/// the focused pane was the one removed.
fn remove_leaf_promoting_sibling(
    node: &Arc<SplitNode>,
    target: PaneId,
) -> (Arc<SplitNode>, Option<PaneId>) {
    let SplitNode::Branch(branch) = node.as_ref() else {
        return (Arc::clone(node), None);
    };

    if let SplitNode::Leaf(fl) = branch.first.as_ref() {
        if fl.id == target {
            let survivor = branch.second.leaves().first().map(|l| l.id);
            return (Arc::clone(&branch.second), survivor);
        }
    }
    if let SplitNode::Leaf(sl) = branch.second.as_ref() {
        if sl.id == target {
            let survivor = branch.first.leaves().first().map(|l| l.id);
            return (Arc::clone(&branch.first), survivor);
        }
    }

    let (first, s1) = remove_leaf_promoting_sibling(&branch.first, target);
    if !Arc::ptr_eq(&first, &branch.first) {
        return (
            Arc::new(SplitNode::Branch(SplitBranch {
                first,
                ..branch.clone()
            })),
            s1,
        );
    }
    let (second, s2) = remove_leaf_promoting_sibling(&branch.second, target);
    if !Arc::ptr_eq(&second, &branch.second) {
        return (
            Arc::new(SplitNode::Branch(SplitBranch {
                second,
                ..branch.clone()
            })),
            s2,
        );
    }
    (Arc::clone(node), None)
}

fn set_divider(node: &Arc<SplitNode>, id: BranchId, value: f64) -> (Arc<SplitNode>, bool) {
    let SplitNode::Branch(branch) = node.as_ref() else {
        return (Arc::clone(node), false);
    };
    if branch.id == id {
        if branch.divider_position == value {
            return (Arc::clone(node), false);
        }
        return (
            Arc::new(SplitNode::Branch(SplitBranch {
                divider_position: value,
                ..branch.clone()
            })),
            true,
        );
    }
    let (first, c1) = set_divider(&branch.first, id, value);
    if c1 {
        return (
            Arc::new(SplitNode::Branch(SplitBranch {
                first,
                ..branch.clone()
            })),
            true,
        );
    }
    let (second, c2) = set_divider(&branch.second, id, value);
    if c2 {
        return (
            Arc::new(SplitNode::Branch(SplitBranch {
                second,
                ..branch.clone()
            })),
            true,
        );
    }
    (Arc::clone(node), false)
}

fn equalize_node(node: &Arc<SplitNode>) -> Arc<SplitNode> {
    match node.as_ref() {
        SplitNode::Leaf(_) => Arc::clone(node),
        SplitNode::Branch(branch) => Arc::new(SplitNode::Branch(SplitBranch {
            first: equalize_node(&branch.first),
            second: equalize_node(&branch.second),
            divider_position: 0.5,
            ..branch.clone()
        })),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn collect_branches(node: &SplitNode, out: &mut Vec<SplitBranch>) {
        if let SplitNode::Branch(b) = node {
            out.push(b.clone());
            collect_branches(&b.first, out);
            collect_branches(&b.second, out);
        }
    }

    fn branches(node: Option<&Arc<SplitNode>>) -> Vec<SplitBranch> {
        let mut out = Vec::new();
        if let Some(n) = node {
            collect_branches(n, &mut out);
        }
        out
    }

    fn branch_count(node: Option<&Arc<SplitNode>>) -> usize {
        branches(node).len()
    }

    fn leaf_count(node: Option<&Arc<SplitNode>>) -> usize {
        node.map_or(0, |n| n.leaves().len())
    }

    fn root_leaf(snapshot: &TreeSnapshot) -> PaneLeaf {
        snapshot
            .root
            .as_ref()
            .expect("root")
            .as_leaf()
            .expect("root is a PaneLeaf")
            .clone()
    }

    fn root_branch(snapshot: &TreeSnapshot) -> SplitBranch {
        snapshot
            .root
            .as_ref()
            .expect("root")
            .as_branch()
            .expect("root is a SplitBranch")
            .clone()
    }

    // ---- R4: initial state + splits --------------------------------------------------------

    #[test] // Covers R4.
    fn initial_controller_has_one_focused_pane_with_one_selected_surface() {
        let c = SplitTreeController::new();

        let leaf = root_leaf(&c.snapshot());
        assert_eq!(1, leaf.tabs.len());
        assert_eq!(leaf.tabs[0], leaf.selected);
        assert_eq!(leaf.id, c.focused_pane());
        assert_eq!(Some(leaf.selected), c.focused_surface());
        assert_eq!(1, c.all_pane_ids().len());
    }

    #[test] // Covers R4.
    fn split_vertical_creates_branch_with_original_first_and_new_second() {
        let mut c = SplitTreeController::new();
        let original = c.focused_pane();
        let original_surface = c.focused_surface().unwrap();

        c.split(original, Orientation::Vertical, false);

        let branch = root_branch(&c.snapshot());
        assert_eq!(Orientation::Vertical, branch.orientation);
        assert_eq!(0.5, branch.divider_position);

        let first = branch.first.as_leaf().expect("first is a PaneLeaf");
        assert_eq!(original, first.id);
        assert_eq!(original_surface, first.selected);

        let second = branch.second.as_leaf().expect("second is a PaneLeaf");
        assert_eq!(1, second.tabs.len());
        assert_ne!(original_surface, second.selected);

        // New pane is focused and its lone surface is the focused surface.
        assert_eq!(second.id, c.focused_pane());
        assert_eq!(Some(second.selected), c.focused_surface());
    }

    #[test] // Covers R4.
    fn split_insert_first_places_new_pane_first() {
        let mut c = SplitTreeController::new();
        let original = c.focused_pane();

        c.split(original, Orientation::Horizontal, true);

        let branch = root_branch(&c.snapshot());
        let first = branch.first.as_leaf().expect("first is a PaneLeaf");
        let second = branch.second.as_leaf().expect("second is a PaneLeaf");
        assert_eq!(original, second.id); // the original is now Second
        assert_eq!(first.id, c.focused_pane()); // the new pane is First and focused
    }

    #[test] // Covers R4.
    fn nested_split_yields_depth_two_tree_with_three_leaves() {
        let mut c = SplitTreeController::new();
        c.split(c.focused_pane(), Orientation::Vertical, false); // 2 leaves
        c.split(c.focused_pane(), Orientation::Horizontal, false); // split the new pane → 3 leaves

        let snap = c.snapshot();
        assert_eq!(3, leaf_count(snap.root.as_ref()));
        assert_eq!(2, branch_count(snap.root.as_ref()));
        assert_eq!(3, c.all_pane_ids().len());
    }

    // ---- R6: close / heal --------------------------------------------------------------------

    #[test] // Covers R6.
    fn close_tab_on_multi_tab_pane_keeps_pane_and_moves_selection() {
        let mut c = SplitTreeController::new();
        let pane = c.focused_pane();
        c.new_tab(pane); // pane now has 2 tabs, second selected
        let tabs = c.tabs(pane);
        let selected = c.selected_tab(pane).unwrap();
        assert_eq!(2, tabs.len());

        c.close_tab(selected);

        assert_eq!(1, c.tabs(pane).len()); // pane survives with one tab
        assert_eq!(pane, c.focused_pane());
        assert_ne!(selected, c.selected_tab(pane).unwrap()); // selection moved to the adjacent tab
    }

    #[test] // Covers R6.
    fn close_tab_on_non_root_last_tab_promotes_sibling_and_reconciles_focus() {
        let mut c = SplitTreeController::new();
        c.split(c.focused_pane(), Orientation::Vertical, false); // root B1{P1, P2}; focus P2
        c.split(c.focused_pane(), Orientation::Horizontal, false); // root B1{P1, B2{P2, P3}}; focus P3
        assert_eq!(2, branch_count(c.snapshot().root.as_ref()));

        let removed = c.focused_pane(); // P3
        let surface = c.focused_surface().unwrap();
        c.close_tab(surface);

        let snap = c.snapshot();
        assert_eq!(2, leaf_count(snap.root.as_ref())); // P3 gone
        assert_eq!(1, branch_count(snap.root.as_ref())); // B2 collapsed (its divider is gone)
        assert!(!c.all_pane_ids().contains(&removed));
        assert!(c.all_pane_ids().contains(&c.focused_pane())); // focus reconciled to a survivor
    }

    #[test] // Covers R6.
    fn close_tab_on_only_panes_last_tab_empties_tree_and_signals_once() {
        let mut c = SplitTreeController::new();
        let only = c.focused_surface().unwrap();

        let events = c.close_tab(only);

        assert!(c.is_empty());
        assert!(c.snapshot().root.is_none());
        let emptied = events
            .iter()
            .filter(|e| **e == SplitTreeEvent::Emptied)
            .count();
        assert_eq!(1, emptied);
    }

    #[test] // Covers R6 — host re-seed path.
    fn seed_root_after_empty_restores_a_single_focused_pane() {
        let mut c = SplitTreeController::new();
        let only = c.focused_surface().unwrap();
        c.close_tab(only);
        assert!(c.is_empty());

        let seeded = c.seed_root();

        assert!(!c.is_empty());
        let leaf = root_leaf(&c.snapshot());
        assert_eq!(seeded, leaf.selected);
        assert_eq!(leaf.id, c.focused_pane());
    }

    // ---- R5: dividers ------------------------------------------------------------------------

    #[test] // Covers R5.
    fn set_divider_position_clamps_to_unit_interval() {
        let cases: [(f64, f64); 3] = [(-0.4, 0.0), (1.7, 1.0), (0.3, 0.3)];
        for (input, expected) in cases {
            let mut c = SplitTreeController::new();
            c.split(c.focused_pane(), Orientation::Vertical, false);
            let branch = root_branch(&c.snapshot()).id;

            c.set_divider_position(branch, input);

            assert_eq!(expected, root_branch(&c.snapshot()).divider_position);
        }
    }

    #[test] // Covers R5.
    fn equalize_resets_every_divider_in_a_nested_tree() {
        let mut c = SplitTreeController::new();
        c.split(c.focused_pane(), Orientation::Vertical, false);
        c.split(c.focused_pane(), Orientation::Horizontal, false);
        for b in branches(c.snapshot().root.as_ref()) {
            c.set_divider_position(b.id, 0.2);
        }
        for b in branches(c.snapshot().root.as_ref()) {
            assert_eq!(0.2, b.divider_position);
        }

        c.equalize();

        for b in branches(c.snapshot().root.as_ref()) {
            assert_eq!(0.5, b.divider_position);
        }
    }

    // ---- R7: derived focus + tab cycling -----------------------------------------------------

    #[test] // Covers R7.
    fn focused_surface_is_derived_from_selected_tab() {
        let mut c = SplitTreeController::new();
        let pane = c.focused_pane();
        c.new_tab(pane);
        c.new_tab(pane);
        let tabs = c.tabs(pane);

        c.select_tab(pane, tabs[0]);

        assert_eq!(Some(tabs[0]), c.selected_tab(pane));
        assert_eq!(c.selected_tab(c.focused_pane()), c.focused_surface()); // never stored separately
    }

    #[test] // Covers R7.
    fn select_next_previous_tab_wraps_within_focused_pane_only() {
        let mut c = SplitTreeController::new();
        let left = c.focused_pane();
        c.split(left, Orientation::Vertical, false); // focus moves to the new right pane
        let right = c.focused_pane();

        // Give the right (focused) pane three tabs.
        c.new_tab(right);
        c.new_tab(right);
        let tabs = c.tabs(right);
        c.select_tab(right, tabs[0]);

        c.select_previous_tab(); // wraps to the last tab
        assert_eq!(Some(tabs[tabs.len() - 1]), c.selected_tab(right));
        c.select_next_tab(); // wraps back to the first
        assert_eq!(Some(tabs[0]), c.selected_tab(right));

        // The other pane is never touched and focus never crosses panes.
        assert_eq!(right, c.focused_pane());
        assert_eq!(1, c.tabs(left).len());
    }

    // ---- R12 / KTD6: snapshot + id stability -------------------------------------------------

    #[test] // Covers R12 / KTD6.
    fn captured_snapshot_is_unchanged_by_later_mutations() {
        let mut c = SplitTreeController::new();
        let snap = c.snapshot();
        let before = root_leaf(&snap);

        c.split(before.id, Orientation::Vertical, false);
        c.new_tab(c.focused_pane());

        // The previously captured snapshot still describes the single-leaf tree.
        let still_leaf = root_leaf(&snap);
        assert_eq!(1, still_leaf.tabs.len());
        assert!(c
            .snapshot()
            .root
            .as_ref()
            .expect("root")
            .as_branch()
            .is_some()); // the live tree has moved on
    }

    #[test] // Covers KTD6.
    fn closed_ids_are_never_reissued() {
        let mut c = SplitTreeController::new();
        let s1 = c.focused_surface().unwrap();
        c.split(c.focused_pane(), Orientation::Vertical, false);
        let s2 = c.focused_surface().unwrap();

        c.close_tab(s2); // removes the new pane + surface s2
        c.new_tab(c.focused_pane());
        let s3 = c.focused_surface().unwrap();

        assert_ne!(s1, s3);
        assert_ne!(s2, s3);
        assert!(s3.0 > s2.0); // monotonic, never reused
    }

    // ---- U7: zoom (transient view state) -----------------------------------------------------

    #[test] // Covers U7.
    fn toggle_zoom_zooms_the_focused_pane_and_toggles_off() {
        let mut c = SplitTreeController::new();
        c.split(c.focused_pane(), Orientation::Vertical, false);
        let focused = c.focused_pane();

        assert_eq!(None, c.zoomed_pane());
        c.toggle_zoom();
        assert_eq!(Some(focused), c.zoomed_pane());
        assert_eq!(Some(focused), c.snapshot().zoomed_pane); // carried on the snapshot
        c.toggle_zoom();
        assert_eq!(None, c.zoomed_pane());
    }

    #[test] // Covers U7.
    fn clear_zoom_clears_when_zoomed_and_is_a_noop_otherwise() {
        let mut c = SplitTreeController::new();
        c.toggle_zoom();
        assert!(c.zoomed_pane().is_some());

        c.clear_zoom();
        assert_eq!(None, c.zoomed_pane());

        c.clear_zoom(); // no-op, must not panic
        assert_eq!(None, c.zoomed_pane());
    }

    #[test] // Covers U7 — zoom must not survive its pane being healed away.
    fn closing_the_zoomed_pane_clears_the_zoom() {
        let mut c = SplitTreeController::new();
        c.split(c.focused_pane(), Orientation::Vertical, false); // focus the new pane
        c.toggle_zoom();
        assert_eq!(Some(c.focused_pane()), c.zoomed_pane());

        let surface = c.focused_surface().unwrap();
        c.close_tab(surface); // remove the zoomed pane's last tab → pane heals away

        assert_eq!(None, c.zoomed_pane());
    }

    // ---- Surface lifecycle events (host-wiring contract; supports U2) ------------------------

    #[test]
    fn split_and_close_raise_surface_created_and_closed_events() {
        let mut c = SplitTreeController::new();
        let mut created: Vec<SurfaceId> = Vec::new();
        let mut closed: Vec<SurfaceId> = Vec::new();

        for e in c.split(c.focused_pane(), Orientation::Vertical, false) {
            match e {
                SplitTreeEvent::SurfaceCreated(s) => created.push(s),
                SplitTreeEvent::SurfaceClosed(s) => closed.push(s),
                SplitTreeEvent::Emptied => {}
            }
        }
        let new_surface = c.focused_surface().unwrap();
        assert_eq!(vec![new_surface], created);

        for e in c.close_tab(new_surface) {
            match e {
                SplitTreeEvent::SurfaceCreated(s) => created.push(s),
                SplitTreeEvent::SurfaceClosed(s) => closed.push(s),
                SplitTreeEvent::Emptied => {}
            }
        }
        assert!(closed.contains(&new_surface));
    }

    // ---- Shared id allocation (Phase 5: multiple workspaces, globally unique ids) -------------

    #[test]
    fn controllers_sharing_an_allocator_never_mint_colliding_ids() {
        let ids = Rc::new(RefCell::new(IdAllocator::new()));
        let mut a = SplitTreeController::with_allocator(Rc::clone(&ids));
        let mut b = SplitTreeController::with_allocator(Rc::clone(&ids));

        a.new_tab(a.focused_pane());
        b.split(b.focused_pane(), Orientation::Horizontal, false);
        b.new_tab(b.focused_pane());

        let surfaces: Vec<SurfaceId> = a
            .all_surfaces()
            .into_iter()
            .chain(b.all_surfaces())
            .collect();
        let distinct_surfaces: HashSet<SurfaceId> = surfaces.iter().copied().collect();
        assert_eq!(surfaces.len(), distinct_surfaces.len());

        let panes: Vec<PaneId> = a
            .all_pane_ids()
            .into_iter()
            .chain(b.all_pane_ids())
            .collect();
        let distinct_panes: HashSet<PaneId> = panes.iter().copied().collect();
        assert_eq!(panes.len(), distinct_panes.len());
    }
}
