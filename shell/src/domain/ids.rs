//! Port of core/Splits/Ids.cs — pane/surface/branch identity, monotonic and never reused
//! within a session (KTD6), so stale snapshots can never alias later-created panes (R12).

use std::fmt;

/// Identity of a pane (a leaf in the split tree that owns an ordered tab list).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct PaneId(pub i32);

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P{}", self.0)
    }
}

/// Identity of a surface — one terminal (one engine instance), presented as one tab.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct SurfaceId(pub i32);

impl fmt::Display for SurfaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "S{}", self.0)
    }
}

/// Identity of a split branch (an interior node). Stable across rewrites: an operation
/// that does not touch a branch preserves its id.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct BranchId(pub i32);

impl fmt::Display for BranchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "B{}", self.0)
    }
}

/// Mints pane/surface/branch ids. One allocator is shared across all workspaces so
/// `SurfaceId`s are globally unique — a surface id alone (e.g. `OPTIMUS_SURFACE_ID` over
/// the pipe) unambiguously identifies its workspace.
#[derive(Default)]
pub struct IdAllocator {
    pane: i32,
    surface: i32,
    branch: i32,
}

impl IdAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_pane(&mut self) -> PaneId {
        self.pane += 1;
        PaneId(self.pane)
    }

    pub fn next_surface(&mut self) -> SurfaceId {
        self.surface += 1;
        SurfaceId(self.surface)
    }

    pub fn next_branch(&mut self) -> BranchId {
        self.branch += 1;
        BranchId(self.branch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic_and_start_at_one() {
        let mut alloc = IdAllocator::new();
        assert_eq!(alloc.next_pane(), PaneId(1));
        assert_eq!(alloc.next_pane(), PaneId(2));
        assert_eq!(alloc.next_surface(), SurfaceId(1));
        assert_eq!(alloc.next_surface(), SurfaceId(2));
        assert_eq!(alloc.next_branch(), BranchId(1));
    }

    #[test]
    fn counters_are_independent_per_kind() {
        let mut alloc = IdAllocator::new();
        alloc.next_pane();
        alloc.next_pane();
        alloc.next_pane();
        assert_eq!(alloc.next_surface(), SurfaceId(1));
        assert_eq!(alloc.next_branch(), BranchId(1));
    }

    #[test]
    fn display_matches_csharp_tostring() {
        assert_eq!(PaneId(3).to_string(), "P3");
        assert_eq!(SurfaceId(7).to_string(), "S7");
        assert_eq!(BranchId(2).to_string(), "B2");
    }
}
