//! Port of core/Notifications — the notification plane's pure domain core: the raw
//! surface payload, the recorded model, the burst-absorbing queue, the single-owner
//! store with derived indexes, and the effects policy. The coordinator (which wires
//! these to the split tree) lives in a later unit.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::domain::ids::{PaneId, SurfaceId};

/// The raw notification payload as it leaves a surface: the title and body the engine
/// extracted from an OSC 9 / 777 / 99 escape sequence, marshalled and copied into owned
/// strings before it reaches here. Deliberately UI-free and minimal — the recorded model
/// ([`TerminalNotification`]), ids, timestamps, and read state are added by the store,
/// not carried on the wire.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SurfaceNotification {
    pub title: String,
    pub subtitle: String,
    pub body: String,
}

impl SurfaceNotification {
    pub fn new(
        title: impl Into<String>,
        subtitle: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            subtitle: subtitle.into(),
            body: body.into(),
        }
    }
}

/// What a notification click should do. Phase 3 only ever focuses the originating surface
/// (derived from [`TerminalNotification::surface_id`], so a `None` click action means
/// exactly that). The enum exists so a later phase can add richer actions (e.g.
/// reveal-in-Explorer) behind the same field without reshaping the record.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotificationClickKind {
    FocusSurface,
    RevealInExplorer,
}

/// An optional click behavior carried on a [`TerminalNotification`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NotificationClickAction {
    pub kind: NotificationClickKind,
    pub argument: Option<String>,
}

/// Identity of one recorded notification. Stands in for the C# `Guid`: minted from a
/// process-wide monotonic counter, unique within a session, which is all the store's
/// id-keyed mutations require.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct NotificationId(pub u64);

impl NotificationId {
    /// Mint a fresh, never-before-issued id.
    pub fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// One recorded terminal notification: the raw [`SurfaceNotification`] (title/body)
/// enriched with identity, the owning pane/surface, a timestamp, and read/effect state.
/// Value semantics make the store trivially testable. Note the model has **no TabId**:
/// a tab *is* a surface, so [`surface_id`](Self::surface_id) is the tab key and
/// [`pane_id`](Self::pane_id) is the owning pane (resolved from the snapshot at record
/// time by the coordinator).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TerminalNotification {
    pub id: NotificationId,
    pub surface_id: SurfaceId,
    pub pane_id: PaneId,
    pub title: String,
    pub subtitle: String,
    pub body: String,
    pub created_at: SystemTime,
    pub is_read: bool,
    pub pane_flash: bool,
    pub click_action: Option<NotificationClickAction>,
}

impl TerminalNotification {
    /// Build a fresh, unread notification with a new id and a current timestamp. The
    /// store orders by insertion, not [`created_at`](Self::created_at), so tests need
    /// not control the clock; callers that do care can overwrite the field.
    pub fn create(surface: SurfaceId, pane: PaneId, title: impl Into<String>) -> Self {
        Self {
            id: NotificationId::next(),
            surface_id: surface,
            pane_id: pane,
            title: title.into(),
            subtitle: String::new(),
            body: String::new(),
            created_at: SystemTime::now(),
            is_read: false,
            pane_flash: true,
            click_action: None,
        }
    }
}

/// One queued, not-yet-delivered notification. The `generation` is the queue's internal
/// coalescing-boundary stamp at enqueue time; callers consume `surface` and `payload`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NotificationEntry {
    pub surface: SurfaceId,
    pub payload: SurfaceNotification,
    pub generation: i64,
}

/// Absorbs notification bursts and drops orphans before anything reaches the store. A
/// pure, synchronous, UI-free component: the only timing/threading concern (scheduling
/// the drain) lives in the shell. Ported in intent from the macOS `TerminalMutationBus`:
/// a coalescing key of `(generation, surface)`, a max-16 delivery cap, target
/// revalidation via a caller-supplied liveness probe, and a generation counter so a
/// clear acts as a true boundary.
///
/// The pane is deliberately **not** part of the key or the entry: a [`SurfaceId`] is
/// globally unique and never reissued, so the surface alone identifies the target. The
/// pane is re-derived from the live snapshot at delivery time by the coordinator.
#[derive(Default)]
pub struct NotificationQueue {
    pending: Vec<NotificationEntry>,
    generation: i64,
}

impl NotificationQueue {
    /// Maximum entries delivered in a single [`drain`](Self::drain) (macOS parity).
    pub const MAX_PER_DRAIN: usize = 16;

    pub fn new() -> Self {
        Self::default()
    }

    /// The current coalescing generation. Monotonic; bumped by
    /// [`mark_clear_boundary`](Self::mark_clear_boundary) so that entries enqueued after
    /// a clear are never coalesced with — or dropped by — entries from before it.
    pub fn generation(&self) -> i64 {
        self.generation
    }

    /// Entries waiting to be drained (test/inspection aid).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Queue a notification for `surface`. With `coalesce` any pending entry for the
    /// same surface *in the current generation* is replaced, so a burst collapses to
    /// the latest payload. The CLI path passes `coalesce: false` so scripted
    /// notifications are never merged.
    pub fn enqueue(&mut self, surface: SurfaceId, payload: SurfaceNotification, coalesce: bool) {
        if coalesce {
            let generation = self.generation;
            self.pending
                .retain(|e| !(e.generation == generation && e.surface == surface));
        }
        self.pending.push(NotificationEntry {
            surface,
            payload,
            generation: self.generation,
        });
    }

    /// Deliver up to [`MAX_PER_DRAIN`](Self::MAX_PER_DRAIN) live entries, oldest first.
    /// Each entry is probed with `is_live`: a surface that no longer exists (the probe
    /// returns false) is an orphan and is dropped without delivery or counting against
    /// the cap. Returns whether any entries remain pending after this drain, so the
    /// caller can reschedule.
    pub fn drain(
        &mut self,
        mut is_live: impl FnMut(SurfaceId) -> bool,
        mut deliver: impl FnMut(NotificationEntry),
    ) -> bool {
        let mut delivered = 0;
        let idx = 0; // never advances: every path either removes at idx or breaks
        while idx < self.pending.len() {
            if !is_live(self.pending[idx].surface) {
                self.pending.remove(idx); // orphan: drop, do not advance idx, do not count
                continue;
            }
            if delivered >= Self::MAX_PER_DRAIN {
                break; // cap reached; this live entry and everything after it stay pending
            }
            deliver(self.pending.remove(idx));
            delivered += 1;
        }
        !self.pending.is_empty()
    }

    /// Advance the coalescing generation. Pair with
    /// [`clear_pending`](Self::clear_pending): bumping first moves every live entry
    /// below the new boundary, so a subsequent clear drops them while anything enqueued
    /// afterward (at the new generation) survives.
    pub fn mark_clear_boundary(&mut self) {
        self.generation += 1;
    }

    /// Drop pending entries that match `in_scope` and were enqueued before the current
    /// boundary (generation strictly less than [`generation`](Self::generation)). Call
    /// [`mark_clear_boundary`](Self::mark_clear_boundary) first so post-clear entries
    /// are preserved.
    pub fn clear_pending(&mut self, mut in_scope: impl FnMut(SurfaceId) -> bool) {
        let generation = self.generation;
        self.pending
            .retain(|e| !(e.generation < generation && in_scope(e.surface)));
    }
}

/// The single owner of recorded notifications — the notification-plane analog of the
/// surface manager. Keeps one ordered, newest-first list and rebuilds its derived
/// indexes on every mutation. Two invariants mirror the macOS store: **at most one live
/// notification per surface** (a new one for a surface replaces the old), and a
/// **latest-per-pane** view that survives marking-read (it feeds the sidebar's "most
/// recent" row).
///
/// Pure domain: never touches a thread primitive — all callers run on one thread, so
/// the recompute-on-mutation approach is both correct and cheap at the low
/// notification rate.
#[derive(Default)]
pub struct NotificationStore {
    // Backing list, newest first. Small (one entry per surface with a live
    // notification), so a full reindex on mutation is cheaper than maintaining
    // incremental deltas and far easier to trust.
    items: Vec<TerminalNotification>,
    unread_count: usize,
    unread_count_by_pane: HashMap<PaneId, usize>,
    latest_by_pane: HashMap<PaneId, TerminalNotification>,
    unread_by_pane_surface: HashSet<(PaneId, SurfaceId)>,
    unread_surfaces: HashSet<SurfaceId>,
}

impl NotificationStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every recorded notification, newest first.
    pub fn items(&self) -> &[TerminalNotification] {
        &self.items
    }

    /// Total unread across all panes/surfaces.
    pub fn unread_count(&self) -> usize {
        self.unread_count
    }

    /// Unread count per pane (panes with zero unread are absent).
    pub fn unread_count_by_pane(&self) -> &HashMap<PaneId, usize> {
        &self.unread_count_by_pane
    }

    /// Newest notification per pane regardless of read state (the sidebar feed).
    pub fn latest_by_pane(&self) -> &HashMap<PaneId, TerminalNotification> {
        &self.latest_by_pane
    }

    /// The set of currently-unread `(pane, surface)` keys (badge source).
    pub fn unread_by_pane_surface(&self) -> &HashSet<(PaneId, SurfaceId)> {
        &self.unread_by_pane_surface
    }

    /// Whether `surface` currently has an unread notification (badge probe).
    pub fn is_surface_unread(&self, surface: SurfaceId) -> bool {
        self.unread_surfaces.contains(&surface)
    }

    /// Record `n`, replacing any existing notification for the same surface so each
    /// surface holds at most one (macOS parity). The new entry becomes the newest.
    pub fn add(&mut self, n: TerminalNotification) {
        self.items.retain(|x| x.surface_id != n.surface_id);
        self.items.insert(0, n);
        self.reindex();
    }

    /// Mark the notification with `id` read, if present.
    pub fn mark_read(&mut self, id: NotificationId) {
        self.mutate_where(|n| n.id == id, |n| n.is_read = true);
    }

    /// Mark `surface`'s notification (if any) read.
    pub fn mark_read_for_surface(&mut self, surface: SurfaceId) {
        self.mutate_where(|n| n.surface_id == surface, |n| n.is_read = true);
    }

    /// Mark every notification owned by `pane` read.
    pub fn mark_read_for_pane(&mut self, pane: PaneId) {
        self.mutate_where(|n| n.pane_id == pane, |n| n.is_read = true);
    }

    /// Mark the notification with `id` unread, if present.
    pub fn mark_unread(&mut self, id: NotificationId) {
        self.mutate_where(|n| n.id == id, |n| n.is_read = false);
    }

    /// Remove the notification with `id`, if present.
    pub fn remove(&mut self, id: NotificationId) {
        self.remove_where(|n| n.id == id);
    }

    /// Drop `surface`'s notification (if any).
    pub fn clear_for_surface(&mut self, surface: SurfaceId) {
        self.remove_where(|n| n.surface_id == surface);
    }

    /// Drop every notification owned by `pane`.
    pub fn clear_for_pane(&mut self, pane: PaneId) {
        self.remove_where(|n| n.pane_id == pane);
    }

    /// Drop every notification.
    pub fn clear_all(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.items.clear();
        self.reindex();
    }

    fn mutate_where(
        &mut self,
        mut matches: impl FnMut(&TerminalNotification) -> bool,
        mut change: impl FnMut(&mut TerminalNotification),
    ) {
        let mut any = false;
        for n in &mut self.items {
            if matches(n) {
                change(n);
                any = true;
            }
        }
        if any {
            self.reindex();
        }
    }

    fn remove_where(&mut self, mut matches: impl FnMut(&TerminalNotification) -> bool) {
        let before = self.items.len();
        self.items.retain(|n| !matches(n));
        if self.items.len() != before {
            self.reindex();
        }
    }

    // Rebuild all derived indexes from the ordered list. `items` is newest-first, so
    // the first entry seen for a pane is its latest.
    fn reindex(&mut self) {
        let mut unread_count_by_pane: HashMap<PaneId, usize> = HashMap::new();
        let mut latest_by_pane: HashMap<PaneId, TerminalNotification> = HashMap::new();
        let mut unread_by_pane_surface: HashSet<(PaneId, SurfaceId)> = HashSet::new();
        let mut unread_surfaces: HashSet<SurfaceId> = HashSet::new();
        let mut unread = 0;

        for n in &self.items {
            latest_by_pane.entry(n.pane_id).or_insert_with(|| n.clone());
            if !n.is_read {
                unread += 1;
                *unread_count_by_pane.entry(n.pane_id).or_insert(0) += 1;
                unread_by_pane_surface.insert((n.pane_id, n.surface_id));
                unread_surfaces.insert(n.surface_id);
            }
        }

        self.unread_count = unread;
        self.unread_count_by_pane = unread_count_by_pane;
        self.latest_by_pane = latest_by_pane;
        self.unread_by_pane_surface = unread_by_pane_surface;
        self.unread_surfaces = unread_surfaces;
    }
}

/// What a notification should *do*. `Default` yields the all-enabled set; struct-update
/// syntax expresses the suppression rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NotificationEffects {
    pub record: bool,
    pub mark_unread: bool,
    pub desktop: bool,
    pub sound: bool,
    pub pane_flash: bool,
    pub reorder_workspace: bool,
}

impl Default for NotificationEffects {
    fn default() -> Self {
        Self {
            record: true,
            mark_unread: true,
            desktop: true,
            sound: true,
            pane_flash: true,
            reorder_workspace: true,
        }
    }
}

/// Whether the originating surface is in front of the user right now. All three must
/// hold for the toast to be suppressed: the app window is foreground, the surface is
/// the focused one, and it is actually visible (its tab is selected and no other pane
/// is zoomed over it). Re-derived from the live snapshot at delivery time by the
/// coordinator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeliveryContext {
    pub app_focused: bool,
    pub surface_focused: bool,
    pub surface_visible: bool,
}

/// Decides the [`NotificationEffects`] for a notification. This is the **reduced** port
/// of the macOS policy: static defaults plus the suppress-when-in-front rule. The macOS
/// external shell-hook pipeline (process spawn, JSON patch/merge, trust authorization)
/// is out of scope; the overrides seam is the single hook a later hook-derived-effects
/// implementation will plug into without reshaping callers.
#[derive(Default)]
pub struct NotificationPolicy {
    defaults: NotificationEffects,
    #[allow(clippy::type_complexity)]
    overrides: Option<Box<dyn Fn(&SurfaceNotification) -> NotificationEffects>>,
}

impl NotificationPolicy {
    /// Policy with the all-enabled defaults and no override seam.
    pub fn new() -> Self {
        Self::default()
    }

    /// Policy with custom static defaults and no override seam.
    pub fn with_defaults(defaults: NotificationEffects) -> Self {
        Self {
            defaults,
            overrides: None,
        }
    }

    /// Policy whose per-request effects come from `overrides` (the hook-injection seam).
    pub fn with_overrides(
        overrides: impl Fn(&SurfaceNotification) -> NotificationEffects + 'static,
    ) -> Self {
        Self {
            defaults: NotificationEffects::default(),
            overrides: Some(Box::new(overrides)),
        }
    }

    /// Resolve the effects for `request` given the current delivery context. Starts
    /// from the override seam (if any) or the defaults, then forces `desktop` and
    /// `mark_unread` off when the surface is already in front of the user: the
    /// notification is still recorded, but as already-read and with no OS toast.
    pub fn decide(
        &self,
        request: &SurfaceNotification,
        context: DeliveryContext,
    ) -> NotificationEffects {
        let mut effects = match &self.overrides {
            Some(f) => f(request),
            None => self.defaults,
        };
        if Self::is_in_front_of_user(context) {
            effects.desktop = false;
            effects.mark_unread = false;
        }
        effects
    }

    fn is_in_front_of_user(c: DeliveryContext) -> bool {
        c.app_focused && c.surface_focused && c.surface_visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- NotificationQueue -------------------------------------------------------
    // Coverage for the burst absorber: coalescing per (generation, surface), the
    // max-16-per-drain cap, orphan-dropping via a caller-supplied liveness probe, and
    // the clear boundary. The drain is a pure synchronous method, so every branch is
    // exercised here with no UI, dispatcher, or tree.

    fn p(title: &str) -> SurfaceNotification {
        SurfaceNotification::new(title, "", "")
    }

    #[test] // Covers AE4.
    fn coalesce_keeps_latest_for_same_surface() {
        let mut q = NotificationQueue::new();
        q.enqueue(SurfaceId(1), p("first"), true);
        q.enqueue(SurfaceId(1), p("second"), true);

        let mut delivered: Vec<NotificationEntry> = Vec::new();
        let more = q.drain(|_| true, |e| delivered.push(e));

        assert!(!more);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].payload.title, "second");
    }

    #[test] // The CLI path opts out of coalescing.
    fn no_coalesce_keeps_both() {
        let mut q = NotificationQueue::new();
        q.enqueue(SurfaceId(1), p("first"), false);
        q.enqueue(SurfaceId(1), p("second"), false);

        let mut delivered: Vec<NotificationEntry> = Vec::new();
        q.drain(|_| true, |e| delivered.push(e));

        assert_eq!(delivered.len(), 2);
    }

    #[test] // Covers R5.
    fn drain_caps_at_16_and_reports_more() {
        let mut q = NotificationQueue::new();
        for i in 1..=17 {
            q.enqueue(SurfaceId(i), p(&format!("n{i}")), false);
        }

        let mut first: Vec<NotificationEntry> = Vec::new();
        let mut more = q.drain(|_| true, |e| first.push(e));
        assert_eq!(first.len(), 16);
        assert!(more);

        let mut second: Vec<NotificationEntry> = Vec::new();
        more = q.drain(|_| true, |e| second.push(e));
        assert_eq!(second.len(), 1);
        assert!(!more);
    }

    #[test] // Covers AE5.
    fn orphan_is_dropped_not_delivered() {
        let mut q = NotificationQueue::new();
        q.enqueue(SurfaceId(1), p("dead"), true);

        let mut delivered: Vec<NotificationEntry> = Vec::new();
        let more = q.drain(|_| false, |e| delivered.push(e)); // nothing live

        assert!(delivered.is_empty());
        assert!(!more);
    }

    #[test] // Covers R5.
    fn mixed_live_and_orphan_delivers_only_live() {
        let mut q = NotificationQueue::new();
        q.enqueue(SurfaceId(1), p("live"), true);
        q.enqueue(SurfaceId(2), p("dead"), true);
        q.enqueue(SurfaceId(3), p("live"), true);

        let dead = SurfaceId(2);
        let mut delivered: Vec<NotificationEntry> = Vec::new();
        q.drain(|s| s != dead, |e| delivered.push(e));

        assert_eq!(delivered.len(), 2);
        assert!(!delivered.iter().any(|e| e.surface == dead));
    }

    #[test] // Covers R5 — a clear is a true coalescing boundary.
    fn clear_drops_pre_boundary_pending_but_keeps_post_boundary() {
        let mut q = NotificationQueue::new();
        q.enqueue(SurfaceId(1), p("old"), true); // gen 0
        q.mark_clear_boundary(); // gen -> 1
        q.enqueue(SurfaceId(1), p("new"), true); // gen 1
        q.clear_pending(|_| true); // drops only pre-boundary (gen < 1)

        let mut delivered: Vec<NotificationEntry> = Vec::new();
        q.drain(|_| true, |e| delivered.push(e));

        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].payload.title, "new");
    }

    #[test]
    fn clear_scope_drops_only_matching_surface() {
        let mut q = NotificationQueue::new();
        q.enqueue(SurfaceId(1), p("s1"), true);
        q.enqueue(SurfaceId(2), p("s2"), true);
        q.mark_clear_boundary();
        q.clear_pending(|s| s == SurfaceId(1));

        let mut delivered: Vec<NotificationEntry> = Vec::new();
        q.drain(|_| true, |e| delivered.push(e));

        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].surface, SurfaceId(2));
    }

    // ---- NotificationStore -------------------------------------------------------
    // Coverage for the ordered, newest-first record list with
    // at-most-one-live-notification-per-surface de-dupe and the derived indexes the
    // UI and the sidebar read.

    fn n(surface: i32, pane: i32, title: &str) -> TerminalNotification {
        TerminalNotification::create(SurfaceId(surface), PaneId(pane), title)
    }

    #[test] // Covers R6.
    fn add_one_indexes_unread_and_latest() {
        let mut store = NotificationStore::new();
        let a = n(1, 10, "build done");
        let id = a.id;
        store.add(a);

        assert_eq!(store.unread_count(), 1);
        assert_eq!(store.unread_count_by_pane()[&PaneId(10)], 1);
        assert_eq!(store.latest_by_pane()[&PaneId(10)].id, id);
        assert!(store.is_surface_unread(SurfaceId(1)));
        assert!(store
            .unread_by_pane_surface()
            .contains(&(PaneId(10), SurfaceId(1))));
    }

    #[test] // At most one live notification per surface (macOS parity).
    fn add_second_for_same_surface_replaces_first() {
        let mut store = NotificationStore::new();
        store.add(n(1, 10, "first"));
        store.add(n(1, 10, "second"));

        assert_eq!(store.items().len(), 1);
        assert_eq!(store.items()[0].title, "second");
        assert_eq!(store.unread_count(), 1);
    }

    #[test] // Covers R6.
    fn add_two_surfaces_same_pane_counts_both_latest_is_newer() {
        let mut store = NotificationStore::new();
        store.add(n(1, 10, "older"));
        let newer = n(2, 10, "newer");
        let newer_id = newer.id;
        store.add(newer);

        assert_eq!(store.unread_count_by_pane()[&PaneId(10)], 2);
        assert_eq!(store.latest_by_pane()[&PaneId(10)].id, newer_id);
    }

    #[test] // Covers R6.
    fn mark_read_by_id_drops_unread_but_keeps_in_latest() {
        let mut store = NotificationStore::new();
        let a = n(1, 10, "t");
        let id = a.id;
        store.add(a);
        store.mark_read(id);

        assert_eq!(store.unread_count(), 0);
        assert!(store.latest_by_pane().contains_key(&PaneId(10)));
        assert!(store.latest_by_pane()[&PaneId(10)].is_read);
        assert!(!store.is_surface_unread(SurfaceId(1)));
    }

    #[test] // Covers R6.
    fn mark_read_by_surface_and_pane_clear_only_their_scope() {
        let mut store = NotificationStore::new();
        store.add(n(1, 10, "t"));
        store.add(n(2, 10, "t"));
        store.add(n(3, 20, "t"));

        store.mark_read_for_surface(SurfaceId(1));
        assert!(!store.is_surface_unread(SurfaceId(1)));
        assert!(store.is_surface_unread(SurfaceId(2))); // same pane untouched
        assert!(store.is_surface_unread(SurfaceId(3)));

        store.mark_read_for_pane(PaneId(10));
        assert!(!store.is_surface_unread(SurfaceId(2))); // pane 10 now fully read
        assert!(store.is_surface_unread(SurfaceId(3))); // pane 20 untouched
    }

    #[test] // Covers R6.
    fn unread_by_pane_surface_tracks_membership() {
        let mut store = NotificationStore::new();
        let a = n(1, 10, "t");
        let id = a.id;
        store.add(a);
        assert!(store
            .unread_by_pane_surface()
            .contains(&(PaneId(10), SurfaceId(1))));

        store.mark_read(id);
        assert!(!store
            .unread_by_pane_surface()
            .contains(&(PaneId(10), SurfaceId(1))));
    }

    #[test]
    fn mark_unread_restores_unread() {
        let mut store = NotificationStore::new();
        let a = n(1, 10, "t");
        let id = a.id;
        store.add(a);
        store.mark_read(id);
        store.mark_unread(id);

        assert_eq!(store.unread_count(), 1);
        assert!(store.is_surface_unread(SurfaceId(1)));
    }

    #[test]
    fn remove_and_clear_scopes_prune() {
        let mut store = NotificationStore::new();
        let a = n(1, 10, "t");
        let a_id = a.id;
        store.add(a);
        store.add(n(2, 10, "t"));
        store.add(n(3, 20, "t"));

        store.remove(a_id);
        assert_eq!(store.items().len(), 2);
        assert!(!store.is_surface_unread(SurfaceId(1)));

        store.clear_for_surface(SurfaceId(2));
        assert!(!store.is_surface_unread(SurfaceId(2)));
        assert_eq!(store.items().len(), 1);

        store.add(n(4, 20, "t"));
        store.clear_for_pane(PaneId(20));
        assert!(store.items().is_empty()); // both pane-20 entries gone

        store.add(n(5, 30, "t"));
        store.clear_all();
        assert!(store.items().is_empty());
        assert_eq!(store.unread_count(), 0);
    }

    #[test]
    fn newest_first_ordering_preserved_through_mutations() {
        let mut store = NotificationStore::new();
        store.add(n(1, 10, "a"));
        store.add(n(2, 10, "b"));
        store.add(n(3, 10, "c")); // newest

        let titles = |s: &NotificationStore| {
            s.items()
                .iter()
                .map(|i| i.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(titles(&store), ["c", "b", "a"]);

        store.mark_read(store.items()[1].id); // mark "b" read — order unchanged
        assert_eq!(titles(&store), ["c", "b", "a"]);

        store.remove(store.items()[0].id); // remove "c"
        assert_eq!(titles(&store), ["b", "a"]);
    }

    // ---- NotificationPolicy ------------------------------------------------------
    // Coverage for the default effect set, the suppress-when-in-front-of-the-user
    // rule, and the override seam. This is the reduced port — the macOS external
    // shell-hook pipeline is out of scope; the seam is the only hook into that future.

    fn req() -> SurfaceNotification {
        SurfaceNotification::new("title", "", "body")
    }

    fn ctx(app_focused: bool, surface_focused: bool, surface_visible: bool) -> DeliveryContext {
        DeliveryContext {
            app_focused,
            surface_focused,
            surface_visible,
        }
    }

    #[test] // Covers R7.
    fn default_policy_enables_all_effects_when_not_in_front() {
        let policy = NotificationPolicy::new();
        let effects = policy.decide(&req(), ctx(true, false, true));

        assert!(effects.record);
        assert!(effects.mark_unread);
        assert!(effects.desktop);
        assert!(effects.sound);
        assert!(effects.pane_flash);
        assert!(effects.reorder_workspace);
    }

    #[test] // Covers R4, AE2.
    fn in_front_of_user_suppresses_toast_and_marks_read() {
        let policy = NotificationPolicy::new();
        let effects = policy.decide(&req(), ctx(true, true, true));

        assert!(!effects.desktop); // no OS toast
        assert!(!effects.mark_unread); // recorded as already-read
        assert!(effects.record); // but still recorded
        assert!(effects.pane_flash); // flash is still allowed
    }

    #[test] // Covers R4 boundary.
    fn backgrounded_app_still_toasts_even_if_surface_focused() {
        let policy = NotificationPolicy::new();
        let effects = policy.decide(&req(), ctx(false, true, true));

        assert!(effects.desktop);
        assert!(effects.mark_unread);
    }

    #[test] // Covers R4 boundary.
    fn focused_but_not_visible_is_not_suppressed() {
        let policy = NotificationPolicy::new();
        let effects = policy.decide(&req(), ctx(true, true, false));

        assert!(effects.desktop);
    }

    #[test] // The hook-injection seam.
    fn override_seam_can_disable_record() {
        let policy = NotificationPolicy::with_overrides(|_| NotificationEffects {
            record: false,
            ..NotificationEffects::default()
        });
        let effects = policy.decide(&req(), ctx(false, false, false));

        assert!(!effects.record);
    }

    #[test] // Covers R7.
    fn custom_defaults_flow_through_when_not_suppressed() {
        let policy = NotificationPolicy::with_defaults(NotificationEffects {
            sound: false,
            pane_flash: false,
            ..NotificationEffects::default()
        });
        let effects = policy.decide(&req(), ctx(false, false, false));

        assert!(!effects.sound);
        assert!(!effects.pane_flash);
        assert!(effects.desktop);
    }
}
