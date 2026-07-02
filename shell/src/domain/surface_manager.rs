//! Port of core/Splits — SurfaceManager, SurfaceLifecycleGuard, and the ISurface /
//! ISurfaceFactory seams (KTD9, R10, RAM safe-zone plan U5).
//!
//! The manager is the single owner of live surfaces: it maps each [`SurfaceId`] to its
//! [`Surface`] and is the only thing that creates or disposes one. Decoupling engine
//! lifetime from view lifecycle means the tree can be restructured without destroying
//! surviving shells (R10). Holds no UI types, so it is unit-testable with a fake factory.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::domain::capacity::{CapacityModel, ReservationToken};
use crate::domain::ids::SurfaceId;

// ---- Surface / factory seams -------------------------------------------------------------------

/// A live terminal surface as the model plane sees it (KTD3): an id plus the lifecycle
/// levers the [`SurfaceManager`] pulls. Deliberately knows nothing about the UI host,
/// wgpu, or the engine FFI — the app provides the concrete implementation, which keeps
/// the manager unit-testable against a fake.
///
/// The C# `TitleChanged` / `NotificationRaised` events are not part of this trait: they
/// are UI-plumbing raised by the concrete surface toward the chrome (Tauri event layer),
/// and nothing in the manager consumes them.
pub trait Surface: Send + Sync {
    /// The surface's stable id (matches the model's [`SurfaceId`]).
    fn id(&self) -> SurfaceId;

    /// Composite this surface (`true`) or collapse it out of the composition tree
    /// (`false`) so inactive surfaces cost no composition (R3/R11).
    fn set_active(&self, active: bool);

    /// Give this surface OS keyboard focus (programmatic — R8).
    fn focus_surface(&self);

    /// Forward already-encoded text (xterm.js `onData` / paste) to the shell (the
    /// `surface.send_text` socket verb). The engine-backed impl hands the bytes to the PTY.
    fn send_text(&self, text: &str);

    /// Forward a key press (Windows virtual-key + modifier bitmask — the `surface.send_key`
    /// socket verb). The frontend normally encodes keys itself; this is the external-agent path,
    /// so the concrete engine-backed impl owns the virtual-key → bytes encoding.
    fn send_key(&self, virtual_key: u32, modifiers: u32);

    /// Tear the surface down: stop its render thread / engine, then release native
    /// resources. Must be idempotent (R2/R9) and must run independently of view unload
    /// so re-parenting never destroys a live shell (KTD9/R10).
    fn shutdown(&self);
}

/// Creates concrete [`Surface`]s for the [`SurfaceManager`]. The app supplies a factory
/// that builds real terminal panes; tests supply a fake. A failed spawn is an `Err` (the
/// C# factory exception).
pub trait SurfaceFactory: Send + Sync {
    /// Create a surface for `id`, optionally with a working directory / command line.
    fn create(
        &self,
        id: SurfaceId,
        cwd: Option<&str>,
        cmdline: Option<&str>,
    ) -> Result<Arc<dyn Surface>, String>;
}

/// A factory that builds nothing — the [`Domain`](crate::host::Domain)'s default until the
/// engine-backed factory is installed. Every spawn fails, so the manager holds no live surfaces
/// and `focus`/`send` route to nothing. The real `SurfaceId → engine` bridge needs the frontend's
/// per-surface output channel (the `dev_spawn_shell` command), so it is injected by that unit.
pub struct NoSurfaceFactory;

impl SurfaceFactory for NoSurfaceFactory {
    fn create(
        &self,
        _id: SurfaceId,
        _cwd: Option<&str>,
        _cmdline: Option<&str>,
    ) -> Result<Arc<dyn Surface>, String> {
        Err("no surface factory installed (frontend spawn bridge not wired)".to_string())
    }
}

// ---- Errors --------------------------------------------------------------------------------------

/// Failure modes of surface creation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CreateSurfaceError {
    /// Safe-zone cap reached (only from [`SurfaceManager::create_surface`] — the
    /// governed path [`SurfaceManager::try_create_surface`] refuses with `Ok(None)`).
    AtCapacity(SurfaceId),
    /// The factory failed to build the surface. The capacity reservation was released
    /// before this was returned — no leaked slot.
    Spawn(String),
}

impl fmt::Display for CreateSurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AtCapacity(id) => write!(
                f,
                "Safe-zone cap reached; surface {id} refused. Governed callers must use try_create_surface."
            ),
            Self::Spawn(reason) => write!(f, "surface spawn failed: {reason}"),
        }
    }
}

impl std::error::Error for CreateSurfaceError {}

// ---- SurfaceManager ------------------------------------------------------------------------------

/// The single owner of live surfaces (KTD9): the only thing that creates or disposes one.
pub struct SurfaceManager {
    factory: Arc<dyn SurfaceFactory>,
    capacity: Option<Arc<CapacityModel>>,
    surfaces: HashMap<SurfaceId, Arc<dyn Surface>>,
}

impl SurfaceManager {
    /// `capacity` is the RAM safe-zone admission governor (plan U5); `None` leaves the
    /// manager ungoverned (tests, degraded startup).
    pub fn new(factory: Arc<dyn SurfaceFactory>, capacity: Option<Arc<CapacityModel>>) -> Self {
        Self {
            factory,
            capacity,
            surfaces: HashMap::new(),
        }
    }

    /// The live surfaces, keyed by id.
    pub fn surfaces(&self) -> &HashMap<SurfaceId, Arc<dyn Surface>> {
        &self.surfaces
    }

    /// Number of live surfaces.
    pub fn count(&self) -> usize {
        self.surfaces.len()
    }

    /// Create (or return the existing) surface for `id`. Idempotent per id (R1): a
    /// duplicate request for a live id returns the same instance rather than building a
    /// second. Errs with [`CreateSurfaceError::AtCapacity`] when the governor refuses.
    pub fn create_surface(
        &mut self,
        id: SurfaceId,
        cwd: Option<&str>,
        cmdline: Option<&str>,
    ) -> Result<Arc<dyn Surface>, CreateSurfaceError> {
        match self.try_create_surface(id, cwd, cmdline)? {
            Some(surface) => Ok(surface),
            None => Err(CreateSurfaceError::AtCapacity(id)),
        }
    }

    /// Capacity-gated create (RAM safe-zone plan U5): the single spawn choke point.
    /// Returns the existing surface for a live id (idempotent, never consumes a second
    /// slot), `Ok(None)` when the [`CapacityModel`] refuses a slot (graceful refusal —
    /// no factory call, no error), or the freshly built surface otherwise. A factory
    /// failure releases the reservation before the error propagates. With no model
    /// attached, never refuses.
    pub fn try_create_surface(
        &mut self,
        id: SurfaceId,
        cwd: Option<&str>,
        cmdline: Option<&str>,
    ) -> Result<Option<Arc<dyn Surface>>, CreateSurfaceError> {
        if let Some(existing) = self.surfaces.get(&id) {
            return Ok(Some(Arc::clone(existing))); // idempotent per id (R1) — no capacity consumed
        }

        let mut token: Option<ReservationToken> = None;
        if let Some(capacity) = &self.capacity {
            token = match capacity.try_reserve(id) {
                Some(t) => Some(t),
                None => return Ok(None), // at cap — refuse without invoking the factory
            };
        }

        let surface = match self.factory.create(id, cwd, cmdline) {
            Ok(surface) => surface,
            Err(reason) => {
                // Failed spawn must not strand a reserved slot.
                if let Some(capacity) = &self.capacity {
                    capacity.release(id);
                }
                return Err(CreateSurfaceError::Spawn(reason));
            }
        };

        self.surfaces.insert(id, Arc::clone(&surface));
        // The engine spawns its child lazily, so no PID exists at this layer yet —
        // commit with pid 0 to settle the slot accounting; the pane reports the real
        // PID's memory via record_measurement during calibration (U4).
        // ACCEPTED AS DESIGNED: capacity is committed at *surface creation* — the
        // surface is the capacity unit, and its slot is freed only on surface removal
        // (dispose_surface/dispose_all → release). A pane that never configures (never
        // spawns a shell) still holds its slot; that is intentional: the slot accounts
        // for the surface's eventual cost.
        if let (Some(capacity), Some(token)) = (&self.capacity, &token) {
            capacity.commit(token, 0);
        }
        Ok(Some(surface))
    }

    /// The surface for `id`, or `None` if it is not live.
    pub fn get(&self, id: SurfaceId) -> Option<Arc<dyn Surface>> {
        self.surfaces.get(&id).map(Arc::clone)
    }

    /// Tear down only the surface for `id`, if live (R2). No-op otherwise.
    pub fn dispose_surface(&mut self, id: SurfaceId) {
        if let Some(surface) = self.surfaces.remove(&id) {
            surface.shutdown();
            if let Some(capacity) = &self.capacity {
                capacity.release(id); // free the safe-zone slot (U5)
            }
        }
    }

    /// Tear down every live surface exactly once and clear the registry (R9).
    pub fn dispose_all(&mut self) {
        // Snapshot first: shutdown() must not be able to mutate the map mid-iteration.
        let drained: Vec<(SurfaceId, Arc<dyn Surface>)> = self.surfaces.drain().collect();
        for (_, surface) in &drained {
            surface.shutdown();
        }
        if let Some(capacity) = &self.capacity {
            for (id, _) in &drained {
                capacity.release(*id); // free every safe-zone slot (U5)
            }
        }
    }
}

impl Drop for SurfaceManager {
    fn drop(&mut self) {
        self.dispose_all();
    }
}

// ---- SurfaceLifecycleGuard -----------------------------------------------------------------------

/// The attach-once / shutdown-once guard at the heart of the Phase-2 correctness fix
/// (KTD9, R10). Tree restructuring re-parents leaf controls, which fires unload/load a
/// second time; the surface must attach its GPU panel **exactly once** and tear down
/// **exactly once**, regardless of how many lifecycle events fire. Extracted from the
/// view control so the invariant is unit-testable without a UI host.
#[derive(Default)]
pub struct SurfaceLifecycleGuard {
    attached: bool,
    disposed: bool,
}

impl SurfaceLifecycleGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` once [`Self::try_attach`] has succeeded.
    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// `true` once [`Self::try_shutdown`] has succeeded.
    pub fn is_disposed(&self) -> bool {
        self.disposed
    }

    /// Returns `true` exactly once — on the first call before shutdown. The first load
    /// attaches; any later load from re-parenting gets `false` and must not re-attach
    /// (R10). Always `false` after shutdown.
    pub fn try_attach(&mut self) -> bool {
        if self.disposed || self.attached {
            return false;
        }
        self.attached = true;
        true
    }

    /// Returns `true` exactly once — making teardown idempotent (R2/R9). Subsequent
    /// calls (a second close, or a stray unload) get `false`.
    pub fn try_shutdown(&mut self) -> bool {
        if self.disposed {
            return false;
        }
        self.disposed = true;
        true
    }
}

// ---- Tests ---------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::capacity::{CapacityLevel, CapacityProvider};
    use std::sync::Mutex;

    // ---- Fakes shared with SurfaceManagerTests ------------------------------------------------

    struct FakeSurface {
        id: SurfaceId,
        log: Arc<Mutex<Vec<String>>>,
        shutdown_count: Mutex<i32>,
    }

    impl FakeSurface {
        fn new(id: SurfaceId, log: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                id,
                log,
                shutdown_count: Mutex::new(0),
            }
        }

        fn shutdown_count(&self) -> i32 {
            *self.shutdown_count.lock().unwrap()
        }
    }

    impl Surface for FakeSurface {
        fn id(&self) -> SurfaceId {
            self.id
        }

        fn set_active(&self, active: bool) {
            self.log
                .lock()
                .unwrap()
                .push(format!("active:{}:{}", self.id, active));
        }

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

        fn shutdown(&self) {
            *self.shutdown_count.lock().unwrap() += 1;
            self.log
                .lock()
                .unwrap()
                .push(format!("shutdown:{}", self.id));
        }
    }

    struct FakeFactory {
        log: Arc<Mutex<Vec<String>>>,
        created: Mutex<Vec<Arc<FakeSurface>>>,
    }

    impl FakeFactory {
        fn new(log: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                log,
                created: Mutex::new(Vec::new()),
            }
        }

        fn created(&self, index: usize) -> Arc<FakeSurface> {
            Arc::clone(&self.created.lock().unwrap()[index])
        }

        fn created_count(&self) -> usize {
            self.created.lock().unwrap().len()
        }
    }

    impl SurfaceFactory for FakeFactory {
        fn create(
            &self,
            id: SurfaceId,
            _cwd: Option<&str>,
            _cmdline: Option<&str>,
        ) -> Result<Arc<dyn Surface>, String> {
            self.log.lock().unwrap().push(format!("create:{id}"));
            let surface = Arc::new(FakeSurface::new(id, Arc::clone(&self.log)));
            self.created.lock().unwrap().push(Arc::clone(&surface));
            Ok(surface)
        }
    }

    // ---- SurfaceManagerTests -------------------------------------------------------------------

    #[test] // Covers R1.
    fn create_surface_is_idempotent_per_id_and_independent_across_ids() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory::new(log));
        let mut manager =
            SurfaceManager::new(Arc::clone(&factory) as Arc<dyn SurfaceFactory>, None);

        let first = manager.create_surface(SurfaceId(1), None, None).unwrap();
        let first_again = manager.create_surface(SurfaceId(1), None, None).unwrap(); // duplicate request
        let second = manager.create_surface(SurfaceId(2), None, None).unwrap();

        assert!(Arc::ptr_eq(&first, &first_again)); // no second instance for the same id
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(2, factory.created_count());
        assert_eq!(2, manager.count());
        // Creating id 2 left id 1 intact.
        assert!(Arc::ptr_eq(&first, &manager.get(SurfaceId(1)).unwrap()));
    }

    #[test] // Covers R2/R9.
    fn dispose_surface_tears_down_only_the_target() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory::new(log));
        let mut manager =
            SurfaceManager::new(Arc::clone(&factory) as Arc<dyn SurfaceFactory>, None);
        manager.create_surface(SurfaceId(1), None, None).unwrap();
        manager.create_surface(SurfaceId(2), None, None).unwrap();

        manager.dispose_surface(SurfaceId(1));

        assert_eq!(1, factory.created(0).shutdown_count()); // id 1 shut down once
        assert_eq!(0, factory.created(1).shutdown_count()); // id 2 untouched
        assert!(manager.get(SurfaceId(1)).is_none());
        assert!(manager.get(SurfaceId(2)).is_some());
        assert_eq!(1, manager.count());
    }

    #[test]
    fn dispose_surface_unknown_id_is_a_noop() {
        let factory = Arc::new(FakeFactory::new(Arc::new(Mutex::new(Vec::new()))));
        let mut manager = SurfaceManager::new(factory as Arc<dyn SurfaceFactory>, None);
        manager.create_surface(SurfaceId(1), None, None).unwrap();

        manager.dispose_surface(SurfaceId(99)); // must not panic

        assert_eq!(1, manager.count());
    }

    #[test] // Covers R9.
    fn dispose_all_tears_down_every_surface_exactly_once() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeFactory::new(log));
        let mut manager =
            SurfaceManager::new(Arc::clone(&factory) as Arc<dyn SurfaceFactory>, None);
        manager.create_surface(SurfaceId(1), None, None).unwrap();
        manager.create_surface(SurfaceId(2), None, None).unwrap();
        manager.create_surface(SurfaceId(3), None, None).unwrap();

        manager.dispose_all();

        for surface in factory.created.lock().unwrap().iter() {
            assert_eq!(1, surface.shutdown_count());
        }
        assert_eq!(0, manager.count());
    }

    // ---- SurfaceLifecycleGuard (R10 / KTD9 — re-parent correctness) ---------------------------

    #[test] // Covers R10.
    fn guard_attaches_once_and_ignores_reparent_reload() {
        let mut guard = SurfaceLifecycleGuard::new();

        assert!(guard.try_attach()); // first Loaded → attach
        assert!(guard.is_attached());
        assert!(!guard.try_attach()); // re-parent's second Loaded → no re-attach
    }

    #[test] // Covers R2/R9.
    fn guard_shutdown_is_idempotent_and_blocks_later_attach() {
        let mut guard = SurfaceLifecycleGuard::new();
        guard.try_attach();

        assert!(guard.try_shutdown()); // explicit teardown
        assert!(guard.is_disposed());
        assert!(!guard.try_shutdown()); // a second close is a no-op
        assert!(!guard.try_attach()); // and we never re-attach after shutdown
    }

    #[test] // Covers R10 — the canonical re-parent sequence.
    fn reparent_sequence_attaches_once_and_never_disposes() {
        // Simulates Loaded → Unloaded (which, post-U2, no longer shuts down) → Loaded.
        let mut guard = SurfaceLifecycleGuard::new();
        let mut attach_count = 0;

        if guard.try_attach() {
            attach_count += 1; // Loaded #1
        }
        // Unloaded fires here but the control no longer calls shutdown.
        if guard.try_attach() {
            attach_count += 1; // Loaded #2 from re-parent
        }

        assert_eq!(1, attach_count); // attached exactly once
        assert!(!guard.is_disposed()); // engine never disposed by the re-parent
    }

    // ---- SurfaceManagerCapacityTests -----------------------------------------------------------

    const GB: u64 = 1024 * 1024 * 1024;

    struct FakeCapacityProvider {
        total_phys: u64,
        available_phys: u64,
        commit_headroom: u64,
    }

    impl CapacityProvider for FakeCapacityProvider {
        fn total_phys_bytes(&self) -> u64 {
            self.total_phys
        }

        fn available_phys_bytes(&self) -> u64 {
            self.available_phys
        }

        fn commit_headroom_bytes(&self) -> u64 {
            self.commit_headroom
        }

        fn is_low_memory_signaled(&self) -> bool {
            false
        }

        fn subscribe_low_memory(&self, _listener: Box<dyn Fn() + Send + Sync>) {}

        fn measure_process_private_bytes(&self, _pid: i32) -> Option<u64> {
            None
        }
    }

    /// A model whose safe zone fits exactly `max_terminals` seed budgets.
    fn model_with_cap(max_terminals: i32) -> Arc<CapacityModel> {
        let safe_zone = max_terminals as u64 * CapacityModel::SEED_BUDGET_BYTES;
        let provider = FakeCapacityProvider {
            total_phys: 16 * GB,
            available_phys: safe_zone + CapacityModel::OS_RESERVE_BYTES,
            commit_headroom: safe_zone + CapacityModel::OS_RESERVE_BYTES,
        };
        Arc::new(CapacityModel::new(Arc::new(provider), None))
    }

    /// Capacity-test surface fake: no log, no counters — only identity matters.
    struct BareSurface {
        id: SurfaceId,
    }

    impl Surface for BareSurface {
        fn id(&self) -> SurfaceId {
            self.id
        }

        fn set_active(&self, _active: bool) {}
        fn focus_surface(&self) {}
        fn send_text(&self, _text: &str) {}
        fn send_key(&self, _virtual_key: u32, _modifiers: u32) {}
        fn shutdown(&self) {}
    }

    type OnCreate = Box<dyn Fn(SurfaceId) -> Result<Arc<dyn Surface>, String> + Send + Sync>;

    struct CountingFactory {
        create_calls: Mutex<i32>,
        on_create: Mutex<Option<OnCreate>>,
    }

    impl CountingFactory {
        fn new() -> Self {
            Self {
                create_calls: Mutex::new(0),
                on_create: Mutex::new(None),
            }
        }

        fn create_calls(&self) -> i32 {
            *self.create_calls.lock().unwrap()
        }

        fn set_on_create(&self, on_create: Option<OnCreate>) {
            *self.on_create.lock().unwrap() = on_create;
        }
    }

    impl SurfaceFactory for CountingFactory {
        fn create(
            &self,
            id: SurfaceId,
            _cwd: Option<&str>,
            _cmdline: Option<&str>,
        ) -> Result<Arc<dyn Surface>, String> {
            *self.create_calls.lock().unwrap() += 1;
            match &*self.on_create.lock().unwrap() {
                Some(on_create) => on_create(id),
                None => Ok(Arc::new(BareSurface { id })),
            }
        }
    }

    #[test]
    fn third_create_at_cap_is_refused_and_factory_is_not_invoked() {
        let factory = Arc::new(CountingFactory::new());
        let capacity = model_with_cap(2);
        let mut manager = SurfaceManager::new(
            Arc::clone(&factory) as Arc<dyn SurfaceFactory>,
            Some(Arc::clone(&capacity)),
        );

        let first = manager
            .try_create_surface(SurfaceId(1), None, None)
            .unwrap();
        let second = manager
            .try_create_surface(SurfaceId(2), None, None)
            .unwrap();
        let third = manager
            .try_create_surface(SurfaceId(3), None, None)
            .unwrap();

        assert!(first.is_some());
        assert!(second.is_some());
        assert!(third.is_none()); // graceful typed refusal, no error
        assert_eq!(2, factory.create_calls()); // factory never invoked for the refused id
        assert_eq!(2, manager.count());
        assert_eq!(CapacityLevel::Cap, capacity.state().level);
    }

    #[test]
    fn recreating_an_existing_surface_at_cap_returns_it_without_consuming_a_slot() {
        let factory = Arc::new(CountingFactory::new());
        let capacity = model_with_cap(2);
        let mut manager = SurfaceManager::new(
            Arc::clone(&factory) as Arc<dyn SurfaceFactory>,
            Some(Arc::clone(&capacity)),
        );

        let first = manager
            .try_create_surface(SurfaceId(1), None, None)
            .unwrap()
            .unwrap();
        manager
            .try_create_surface(SurfaceId(2), None, None)
            .unwrap(); // now at cap

        let first_again = manager
            .try_create_surface(SurfaceId(1), None, None) // duplicate at cap
            .unwrap()
            .unwrap();

        assert!(Arc::ptr_eq(&first, &first_again)); // not refused, same instance
        assert_eq!(2, factory.create_calls()); // no second build
        let state = capacity.state();
        assert_eq!(2, state.used + state.reserved); // slot count unchanged
    }

    #[test]
    fn disposing_a_surface_releases_its_slot_so_the_next_create_succeeds() {
        let factory = Arc::new(CountingFactory::new());
        let capacity = model_with_cap(2);
        let mut manager = SurfaceManager::new(factory as Arc<dyn SurfaceFactory>, Some(capacity));
        manager
            .try_create_surface(SurfaceId(1), None, None)
            .unwrap();
        manager
            .try_create_surface(SurfaceId(2), None, None)
            .unwrap();
        // Proves we were at cap.
        assert!(manager
            .try_create_surface(SurfaceId(3), None, None)
            .unwrap()
            .is_none());

        manager.dispose_surface(SurfaceId(1));

        // Freed slot is reusable.
        assert!(manager
            .try_create_surface(SurfaceId(3), None, None)
            .unwrap()
            .is_some());
        assert_eq!(2, manager.count());
    }

    #[test]
    fn dispose_all_releases_every_slot() {
        let factory = Arc::new(CountingFactory::new());
        let capacity = model_with_cap(2);
        let mut manager = SurfaceManager::new(
            factory as Arc<dyn SurfaceFactory>,
            Some(Arc::clone(&capacity)),
        );
        manager
            .try_create_surface(SurfaceId(1), None, None)
            .unwrap();
        manager
            .try_create_surface(SurfaceId(2), None, None)
            .unwrap();

        manager.dispose_all();

        let state = capacity.state();
        assert_eq!(0, state.used + state.reserved);
        assert!(manager
            .try_create_surface(SurfaceId(3), None, None)
            .unwrap()
            .is_some());
        assert!(manager
            .try_create_surface(SurfaceId(4), None, None)
            .unwrap()
            .is_some());
    }

    #[test]
    fn factory_exception_releases_the_reservation_so_the_slot_is_reusable() {
        let factory = Arc::new(CountingFactory::new());
        factory.set_on_create(Some(Box::new(|_| Err("spawn failed".to_string()))));
        let capacity = model_with_cap(1);
        let mut manager = SurfaceManager::new(
            Arc::clone(&factory) as Arc<dyn SurfaceFactory>,
            Some(Arc::clone(&capacity)),
        );

        let result = manager.try_create_surface(SurfaceId(1), None, None);
        assert_eq!(
            Some(CreateSurfaceError::Spawn("spawn failed".to_string())),
            result.err()
        );

        let state = capacity.state();
        assert_eq!(0, state.used + state.reserved); // slot freed
        factory.set_on_create(None);
        // Retry succeeds.
        assert!(manager
            .try_create_surface(SurfaceId(1), None, None)
            .unwrap()
            .is_some());
    }

    #[test]
    fn null_capacity_model_leaves_the_manager_ungoverned() {
        let factory = Arc::new(CountingFactory::new());
        let mut manager = SurfaceManager::new(factory as Arc<dyn SurfaceFactory>, None); // no model

        for i in 1..=50 {
            assert!(manager
                .try_create_surface(SurfaceId(i), None, None)
                .unwrap()
                .is_some());
        }
        assert_eq!(50, manager.count());
    }
}
