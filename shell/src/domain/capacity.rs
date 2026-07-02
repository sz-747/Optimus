//! Port of core/Capacity — the RAM safe-zone capacity governor (plan U2), the
//! calibration store contracts, and the chrome indicator view-model (U6).
//!
//! Tier-1 soft admission control: owns the safe-zone math, the per-terminal budget
//! calibration, and the reserve-then-commit ledger that `SurfaceManager` gates on.
//! Pure domain — all memory facts come from an injected [`CapacityProvider`], all
//! persistence from an injected [`CalibrationStore`], so the whole policy is
//! unit-testable without Win32.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::domain::ids::SurfaceId;

/// Recover deliberately from a poisoned mutex: the protected state is a plain
/// ledger with no invariants that a panicking reader could have broken mid-write
/// beyond what the caller re-derives anyway.
fn lock_recover<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- CapacityState ---------------------------------------------------------------------------

/// Escalation level of the capacity indicator (DESIGN.md: calm → amber → red).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CapacityLevel {
    /// Below 75% of the safe-zone cap.
    Calm,
    /// At or above 75% of the cap.
    Warn,
    /// At (or, after a pressure-tightening, above) the cap. No new spawns.
    Cap,
}

/// Immutable snapshot of the capacity ledger that the chrome indicator (U6) binds to.
/// `used` is committed terminals, `reserved` is in-flight reservations; the level is
/// computed on the `used + reserved` basis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CapacityState {
    pub used: i32,
    pub reserved: i32,
    pub max: i32,
    pub level: CapacityLevel,
}

impl CapacityState {
    pub fn new(used: i32, reserved: i32, max: i32, level: CapacityLevel) -> Self {
        Self {
            used,
            reserved,
            max,
            level,
        }
    }
}

// ---- Provider / store contracts --------------------------------------------------------------

/// Memory facts the capacity governor needs, abstracted so [`CapacityModel`] stays pure
/// domain code. The production binding is the Win32 provider in the app layer (U3:
/// `GlobalMemoryStatusEx` / `GetPerformanceInfo` / `QueryMemoryResourceNotification`);
/// tests use a deterministic fake.
pub trait CapacityProvider: Send + Sync {
    /// Total installed physical RAM. Feeds the calibration hardware fingerprint.
    fn total_phys_bytes(&self) -> u64;

    /// Physical RAM currently available (`ullAvailPhys`).
    fn available_phys_bytes(&self) -> u64;

    /// Commit headroom: `CommitLimit − CommitTotal`.
    fn commit_headroom_bytes(&self) -> u64;

    /// Whether the OS low-memory resource notification is currently signaled. Polled at
    /// tick time; while true, the cap may tighten but never relax.
    fn is_low_memory_signaled(&self) -> bool;

    /// Register a listener fired when the OS raises the low-memory resource notification.
    fn subscribe_low_memory(&self, listener: Box<dyn Fn() + Send + Sync>);

    /// Private bytes (`PrivateUsage`) of the process with `pid`, or `None` if it cannot
    /// be measured (exited, access denied).
    fn measure_process_private_bytes(&self, pid: i32) -> Option<u64>;
}

/// Persisted calibration record — the JSON shape of `capacity.json`:
/// `{ budgetBytes, hardwareFingerprintGb }`. The fingerprint is total physical RAM
/// rounded to whole GB, so a budget calibrated on one machine never poisons another.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CapacityCalibration {
    pub budget_bytes: u64,
    pub hardware_fingerprint_gb: i32,
}

/// Load/save abstraction for [`CapacityCalibration`]. The domain stays IO-free; the app
/// layer binds this to a JSON file next to existing app state.
pub trait CalibrationStore: Send + Sync {
    /// The persisted calibration, or `None` if none exists or it is unreadable.
    fn load(&self) -> Option<CapacityCalibration>;

    fn save(&self, calibration: CapacityCalibration);
}

// ---- Reservation token -----------------------------------------------------------------------

/// Two-phase ticket handed out by [`CapacityModel::try_reserve`] and redeemed by
/// [`CapacityModel::commit`]. Holding a token means a safe-zone slot is held for `id` —
/// parallel spawns cannot race past the cap (TOCTOU guard).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReservationToken {
    pub id: SurfaceId,
}

// ---- CapacityModel ---------------------------------------------------------------------------

/// Handle for a state-changed listener registration; pass back to
/// [`CapacityModel::unsubscribe_state_changed`] to remove the listener.
#[derive(Debug)]
pub struct Subscription(u64);

type StateListener = Box<dyn FnMut(CapacityState) + Send>;

#[derive(Default)]
struct ListenerRegistry {
    next_id: u64,
    entries: Vec<(u64, StateListener)>,
}

/// State guarded by the model's single gate (the C# `lock(_gate)` region).
struct Inner {
    committed: HashMap<SurfaceId, i32>, // surface → pid
    reserved: HashSet<SurfaceId>,
    samples: HashMap<SurfaceId, u64>, // latest measurement per surface
    safe_zone_bytes: u64,
    budget_bytes: u64,
    max_terminals: i32,
    recovery_streak: i32,
}

impl Inner {
    fn snapshot(&self) -> CapacityState {
        let used = self.committed.len() as i32;
        let reserved = self.reserved.len() as i32;
        CapacityState::new(
            used,
            reserved,
            self.max_terminals,
            level(used + reserved, self.max_terminals),
        )
    }

    /// Apply a new budget; only ever tightens the cap (relaxation goes through `on_tick`).
    fn apply_budget(&mut self, budget: u64) -> Option<CapacityState> {
        if budget == self.budget_bytes {
            return None;
        }
        let before = self.snapshot();
        self.budget_bytes = budget;
        self.max_terminals = self
            .max_terminals
            .min(floor_terminals(self.safe_zone_bytes, self.budget_bytes));
        let after = self.snapshot();
        if after != before {
            Some(after)
        } else {
            None
        }
    }
}

fn level(total: i32, max: i32) -> CapacityLevel {
    if total >= max {
        return CapacityLevel::Cap;
    }
    if i64::from(total) * 4 >= i64::from(max) * 3 {
        CapacityLevel::Warn
    } else {
        CapacityLevel::Calm
    }
}

fn compute_safe_zone(avail_phys: u64, commit_headroom: u64) -> u64 {
    let tighter = avail_phys.min(commit_headroom);
    tighter.saturating_sub(CapacityModel::OS_RESERVE_BYTES)
}

fn floor_terminals(safe_zone: u64, budget: u64) -> i32 {
    (safe_zone / budget).min(i32::MAX as u64) as i32
}

/// Nearest-rank P75. ACCEPTED AS DESIGNED: with the minimum 3 samples the nearest rank
/// is the maximum — deliberately conservative (a high budget tightens the cap; the safe
/// zone must never be sized off an optimistic per-terminal cost).
fn percentile_75(samples: impl Iterator<Item = u64>) -> u64 {
    let mut sorted: Vec<u64> = samples.collect();
    sorted.sort_unstable();
    let rank = (sorted.len() as f64 * 0.75).ceil() as usize - 1;
    sorted[rank]
}

fn fingerprint_gb(total_phys_bytes: u64) -> i32 {
    const GB: u64 = 1024 * 1024 * 1024;
    ((total_phys_bytes + GB / 2) / GB) as i32
}

/// Tier-1 soft admission governor: owns the safe-zone math, the per-terminal budget
/// calibration, and the reserve-then-commit ledger that `SurfaceManager::create_surface`
/// gates on. Thread-safe: parallel spawns hit [`Self::try_reserve`] concurrently.
///
/// Cap policy: [`Self::max_terminals`] is monotonically non-increasing under pressure
/// within a session (we tighten, never silently relax). Relaxation requires the OS
/// low-memory signal to be clear AND available RAM ≥ [`Self::RECOVERY_HEADROOM_FACTOR`] ×
/// the tightened safe zone for [`Self::RECOVERY_TICKS_REQUIRED`] consecutive ticks. Live
/// terminals are never reaped: a shrink can leave `used > max`, which only blocks new
/// spawns.
pub struct CapacityModel {
    provider: Arc<dyn CapacityProvider>,
    store: Option<Arc<dyn CalibrationStore>>,
    /// Set by the provider's low-memory signal; consumed at tick time under the gate.
    low_signal_pending: Arc<AtomicBool>,
    gate: Mutex<Inner>,
    listeners: Mutex<ListenerRegistry>,
}

impl CapacityModel {
    /// Headroom always left for the OS — never handed to terminals.
    pub const OS_RESERVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

    /// Conservative first-launch per-terminal budget; calibration can only raise it.
    pub const SEED_BUDGET_BYTES: u64 = 200 * 1024 * 1024;

    /// Samples required before measured budgets replace the seed.
    pub const CALIBRATION_MIN_SAMPLES: usize = 3;

    /// Available RAM must reach this multiple of the safe zone before the cap relaxes.
    pub const RECOVERY_HEADROOM_FACTOR: f64 = 1.25;

    /// Consecutive recovered ticks required before the cap relaxes.
    pub const RECOVERY_TICKS_REQUIRED: i32 = 2;

    pub fn new(
        provider: Arc<dyn CapacityProvider>,
        store: Option<Arc<dyn CalibrationStore>>,
    ) -> Self {
        let low_signal_pending = Arc::new(AtomicBool::new(false));
        let pending = Arc::clone(&low_signal_pending);
        provider.subscribe_low_memory(Box::new(move || pending.store(true, Ordering::SeqCst)));
        let safe_zone_bytes = compute_safe_zone(
            provider.available_phys_bytes(),
            provider.commit_headroom_bytes(),
        );
        let budget_bytes = Self::SEED_BUDGET_BYTES;
        Self {
            provider,
            store,
            low_signal_pending,
            gate: Mutex::new(Inner {
                committed: HashMap::new(),
                reserved: HashSet::new(),
                samples: HashMap::new(),
                safe_zone_bytes,
                budget_bytes,
                max_terminals: floor_terminals(safe_zone_bytes, budget_bytes),
                recovery_streak: 0,
            }),
            listeners: Mutex::new(ListenerRegistry::default()),
        }
    }

    pub fn safe_zone_bytes(&self) -> u64 {
        lock_recover(&self.gate).safe_zone_bytes
    }

    pub fn per_terminal_budget_bytes(&self) -> u64 {
        lock_recover(&self.gate).budget_bytes
    }

    pub fn max_terminals(&self) -> i32 {
        lock_recover(&self.gate).max_terminals
    }

    /// Snapshot for the chrome indicator.
    pub fn state(&self) -> CapacityState {
        lock_recover(&self.gate).snapshot()
    }

    /// Register a listener fired (outside the internal lock) whenever [`Self::state`]
    /// changes. Listeners must not call back into the model.
    pub fn subscribe_state_changed(&self, listener: StateListener) -> Subscription {
        let mut registry = lock_recover(&self.listeners);
        registry.next_id += 1;
        let id = registry.next_id;
        registry.entries.push((id, listener));
        Subscription(id)
    }

    /// Remove a listener registered via [`Self::subscribe_state_changed`].
    pub fn unsubscribe_state_changed(&self, subscription: Subscription) {
        lock_recover(&self.listeners)
            .entries
            .retain(|(id, _)| *id != subscription.0);
    }

    // ---- Reservation ledger ------------------------------------------------------------------

    /// Hold a safe-zone slot for `id`, or `None` when committed + reserved already fills
    /// the cap. Idempotent per id (mirrors `SurfaceManager` R1): a duplicate request for
    /// an id that is already reserved or committed succeeds without consuming a second
    /// slot.
    pub fn try_reserve(&self, id: SurfaceId) -> Option<ReservationToken> {
        let changed;
        {
            let mut inner = lock_recover(&self.gate);
            if inner.committed.contains_key(&id) || inner.reserved.contains(&id) {
                return Some(ReservationToken { id });
            }
            if inner.committed.len() + inner.reserved.len() >= inner.max_terminals.max(0) as usize {
                return None;
            }
            inner.reserved.insert(id);
            changed = Some(inner.snapshot());
        }
        self.raise(changed);
        Some(ReservationToken { id })
    }

    /// Convert a reservation into a committed terminal with `pid`. Idempotent.
    pub fn commit(&self, token: &ReservationToken, pid: i32) {
        let mut changed = None;
        {
            let mut inner = lock_recover(&self.gate);
            if !inner.committed.contains_key(&token.id) {
                inner.reserved.remove(&token.id);
                inner.committed.insert(token.id, pid);
                changed = Some(inner.snapshot());
            }
        }
        self.raise(changed);
    }

    /// Free the slot for `id`, reserved or committed. No-op otherwise.
    pub fn release(&self, id: SurfaceId) {
        let mut changed = None;
        {
            let mut inner = lock_recover(&self.gate);
            let removed = inner.reserved.remove(&id) | inner.committed.remove(&id).is_some();
            inner.samples.remove(&id);
            if removed {
                changed = Some(inner.snapshot());
            }
        }
        self.raise(changed);
    }

    // ---- Calibration ---------------------------------------------------------------------------

    /// Record a measured private-bytes sample for a live terminal (latest sample per
    /// surface wins). Once ≥ [`Self::CALIBRATION_MIN_SAMPLES`] surfaces have samples, the
    /// budget becomes `max(seed, P75(samples))`; a budget rise tightens
    /// [`Self::max_terminals`] immediately, a fall only relaxes via the tick hysteresis.
    pub fn record_measurement(&self, id: SurfaceId, bytes: u64) {
        let mut changed = None;
        {
            let mut inner = lock_recover(&self.gate);
            inner.samples.insert(id, bytes);
            if inner.samples.len() >= Self::CALIBRATION_MIN_SAMPLES {
                let p75 = percentile_75(inner.samples.values().copied());
                changed = inner.apply_budget(Self::SEED_BUDGET_BYTES.max(p75));
            }
        }
        self.raise(changed);
    }

    /// Persist the current budget plus the hardware fingerprint of this machine.
    pub fn save_calibration(&self) {
        let Some(store) = &self.store else {
            return;
        };
        let budget = lock_recover(&self.gate).budget_bytes;
        store.save(CapacityCalibration {
            budget_bytes: budget,
            hardware_fingerprint_gb: fingerprint_gb(self.provider.total_phys_bytes()),
        });
    }

    /// Adopt a persisted budget so the next session starts already-calibrated. Ignored
    /// when no calibration exists or its hardware fingerprint does not match this machine.
    pub fn load_calibration(&self) {
        let calibration = match &self.store {
            Some(store) => store.load(),
            None => None,
        };
        let Some(calibration) = calibration else {
            return;
        };
        if calibration.hardware_fingerprint_gb != fingerprint_gb(self.provider.total_phys_bytes()) {
            return;
        }
        let changed;
        {
            let mut inner = lock_recover(&self.gate);
            changed = inner.apply_budget(Self::SEED_BUDGET_BYTES.max(calibration.budget_bytes));
        }
        self.raise(changed);
    }

    // ---- Tick ----------------------------------------------------------------------------------

    /// Re-read the provider and update the safe zone per the cap policy. Called at 1 Hz
    /// and on the low-memory signal (U3).
    pub fn on_tick(&self) {
        let avail = self.provider.available_phys_bytes();
        let headroom = self.provider.commit_headroom_bytes();
        let mut low_now = self.provider.is_low_memory_signaled();

        let mut changed = None;
        {
            let mut inner = lock_recover(&self.gate);
            low_now |= self.low_signal_pending.swap(false, Ordering::SeqCst);

            let candidate_zone = compute_safe_zone(avail, headroom);
            let candidate_max = floor_terminals(candidate_zone, inner.budget_bytes);
            let before = inner.snapshot();

            if candidate_max < inner.max_terminals {
                inner.safe_zone_bytes = candidate_zone;
                inner.max_terminals = candidate_max;
                inner.recovery_streak = 0;
            } else if candidate_max > inner.max_terminals {
                let recovered = !low_now
                    && avail
                        >= (inner.safe_zone_bytes as f64 * Self::RECOVERY_HEADROOM_FACTOR) as u64;
                inner.recovery_streak = if recovered {
                    inner.recovery_streak + 1
                } else {
                    0
                };
                if inner.recovery_streak >= Self::RECOVERY_TICKS_REQUIRED {
                    inner.safe_zone_bytes = candidate_zone;
                    inner.max_terminals = candidate_max;
                    inner.recovery_streak = 0;
                }
            } else {
                inner.safe_zone_bytes = candidate_zone;
                inner.recovery_streak = 0;
            }

            let after = inner.snapshot();
            if after != before {
                changed = Some(after);
            }
        }
        self.raise(changed);
    }

    // ---- Internals -----------------------------------------------------------------------------

    /// Invoke state-changed listeners. Always called after the gate is released so a
    /// listener never observes the model mid-mutation.
    fn raise(&self, state: Option<CapacityState>) {
        if let Some(s) = state {
            let mut registry = lock_recover(&self.listeners);
            for (_, listener) in registry.entries.iter_mut() {
                listener(s);
            }
        }
    }
}

// ---- CapacityIndicatorViewModel ---------------------------------------------------------------

/// Marshals a queued update onto the UI thread; [`CapacityModel`] state changes fire on
/// background/timer threads.
pub type Dispatcher = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

type PropertyListener = Box<dyn FnMut(&str) + Send>;

/// State shared between the view-model and its dispatched update closures.
struct VmShared {
    state: Mutex<Option<CapacityState>>,
    disposed: AtomicBool,
    property_listeners: Mutex<Vec<PropertyListener>>,
}

impl VmShared {
    fn raise_all(&self) {
        let mut listeners = lock_recover(&self.property_listeners);
        for name in [
            "label_text",
            "fraction_used",
            "level",
            "is_at_cap",
            "hint_text",
        ] {
            for listener in listeners.iter_mut() {
                listener(name);
            }
        }
    }
}

fn queue_state_update(shared: &Arc<VmShared>, dispatch: &Dispatcher, state: CapacityState) {
    let shared = Arc::clone(shared);
    (dispatch.as_ref())(Box::new(move || {
        if shared.disposed.load(Ordering::SeqCst) {
            return;
        }
        *lock_recover(&shared.state) = Some(state);
        shared.raise_all();
    }));
}

/// View-model behind the always-visible chrome capacity indicator (U6): "X / Y terminals"
/// plus a thin level-colored bar above the New-Workspace button. Lives in the domain (not
/// the UI layer) so it is unit-testable without a dispatcher: UI-thread marshalling is an
/// injected [`Dispatcher`], and the view maps [`Self::level`] to design tokens.
///
/// Trusts [`CapacityState::level`] as computed by [`CapacityModel`] — never recomputes
/// thresholds. A `None` model (governor failed to start) renders as the dashed
/// placeholder and never blocks spawning from the chrome side.
pub struct CapacityIndicatorViewModel {
    model: Option<Arc<CapacityModel>>,
    dispatch: Dispatcher,
    shared: Arc<VmShared>,
    /// `Some` while subscribed to the model's state-changed listeners (C# `_attached`).
    subscription: Mutex<Option<Subscription>>,
}

impl CapacityIndicatorViewModel {
    /// One-line reason shown under the disabled New-Workspace button at cap.
    pub const CAP_HINT: &'static str = "Safe-zone full — close a workspace to spawn more";

    /// `model` is the capacity governor, or `None` when it failed to start. `dispatch`
    /// marshals to the UI thread.
    pub fn new(model: Option<Arc<CapacityModel>>, dispatch: Dispatcher) -> Self {
        let shared = Arc::new(VmShared {
            state: Mutex::new(None),
            disposed: AtomicBool::new(false),
            property_listeners: Mutex::new(Vec::new()),
        });
        let mut subscription = None;
        if let Some(model) = &model {
            *lock_recover(&shared.state) = Some(model.state());
            subscription = Some(Self::subscribe(model, &shared, &dispatch));
        }
        Self {
            model,
            dispatch,
            shared,
            subscription: Mutex::new(subscription),
        }
    }

    fn subscribe(
        model: &Arc<CapacityModel>,
        shared: &Arc<VmShared>,
        dispatch: &Dispatcher,
    ) -> Subscription {
        let shared = Arc::clone(shared);
        let dispatch = Arc::clone(dispatch);
        model.subscribe_state_changed(Box::new(move |state| {
            queue_state_update(&shared, &dispatch, state)
        }))
    }

    /// Re-subscribe to the model's state-changed listeners after a [`Self::detach`] (the
    /// owning view re-loaded) and refresh from the model's current state so nothing
    /// missed while detached is lost. No-op when already attached, disposed, or
    /// model-less.
    pub fn attach(&self) {
        if self.shared.disposed.load(Ordering::SeqCst) {
            return;
        }
        let Some(model) = &self.model else {
            return;
        };
        {
            let mut subscription = lock_recover(&self.subscription);
            if subscription.is_some() {
                return;
            }
            *subscription = Some(Self::subscribe(model, &self.shared, &self.dispatch));
        }
        queue_state_update(&self.shared, &self.dispatch, model.state());
    }

    /// Unsubscribe from the app-lifetime model while the owning view is unloaded (the
    /// subscription would otherwise root the view forever). Symmetric with
    /// [`Self::attach`] so unload/load re-parent cycles are safe. Idempotent.
    pub fn detach(&self) {
        let Some(model) = &self.model else {
            return;
        };
        if let Some(subscription) = lock_recover(&self.subscription).take() {
            model.unsubscribe_state_changed(subscription);
        }
    }

    /// Register a listener fired with the snake_case property name whenever a bound
    /// property changes (INotifyPropertyChanged equivalent).
    pub fn subscribe_property_changed(&self, listener: PropertyListener) {
        lock_recover(&self.shared.property_listeners).push(listener);
    }

    /// "X / Y terminals" where X = used + reserved; "— / — terminals" with no model.
    pub fn label_text(&self) -> String {
        match *lock_recover(&self.shared.state) {
            Some(s) => format!("{} / {} terminals", s.used + s.reserved, s.max),
            None => "— / — terminals".to_string(),
        }
    }

    /// Fill fraction for the bar, clamped to [0, 1]; 0 when max is 0 or no model.
    pub fn fraction_used(&self) -> f64 {
        match *lock_recover(&self.shared.state) {
            Some(s) if s.max > 0 => {
                (f64::from(s.used + s.reserved) / f64::from(s.max)).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

    /// Escalation level straight from the model; Calm when no model.
    pub fn level(&self) -> CapacityLevel {
        lock_recover(&self.shared.state).map_or(CapacityLevel::Calm, |s| s.level)
    }

    /// True at the safe-zone cap — the chrome disables the New-Workspace affordance.
    pub fn is_at_cap(&self) -> bool {
        self.level() == CapacityLevel::Cap
    }

    /// The cap hint, or `None` below the cap (the view hides the line).
    pub fn hint_text(&self) -> Option<&'static str> {
        if self.is_at_cap() {
            Some(Self::CAP_HINT)
        } else {
            None
        }
    }

    /// Detach and drop all late-dispatched updates. Idempotent.
    pub fn dispose(&self) {
        if self.shared.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.detach();
    }
}

impl Drop for CapacityIndicatorViewModel {
    fn drop(&mut self) {
        self.dispose();
    }
}

// ---- Tests ------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;

    struct FakeCapacityProvider {
        total_phys: Mutex<u64>,
        available_phys: Mutex<u64>,
        commit_headroom: Mutex<u64>,
        low_signaled: Mutex<bool>,
        listeners: Mutex<Vec<Box<dyn Fn() + Send + Sync>>>,
        process_private: Mutex<HashMap<i32, u64>>,
    }

    impl FakeCapacityProvider {
        fn new() -> Self {
            Self {
                total_phys: Mutex::new(16 * GB),
                available_phys: Mutex::new(10 * GB),
                commit_headroom: Mutex::new(10 * GB),
                low_signaled: Mutex::new(false),
                listeners: Mutex::new(Vec::new()),
                process_private: Mutex::new(HashMap::new()),
            }
        }

        fn set_total_phys(&self, bytes: u64) {
            *self.total_phys.lock().unwrap() = bytes;
        }

        fn set_available_phys(&self, bytes: u64) {
            *self.available_phys.lock().unwrap() = bytes;
        }

        fn set_commit_headroom(&self, bytes: u64) {
            *self.commit_headroom.lock().unwrap() = bytes;
        }

        fn set_low_signaled(&self, value: bool) {
            *self.low_signaled.lock().unwrap() = value;
        }

        fn raise_low_memory(&self) {
            self.set_low_signaled(true);
            for listener in self.listeners.lock().unwrap().iter() {
                listener();
            }
        }
    }

    impl CapacityProvider for FakeCapacityProvider {
        fn total_phys_bytes(&self) -> u64 {
            *self.total_phys.lock().unwrap()
        }

        fn available_phys_bytes(&self) -> u64 {
            *self.available_phys.lock().unwrap()
        }

        fn commit_headroom_bytes(&self) -> u64 {
            *self.commit_headroom.lock().unwrap()
        }

        fn is_low_memory_signaled(&self) -> bool {
            *self.low_signaled.lock().unwrap()
        }

        fn subscribe_low_memory(&self, listener: Box<dyn Fn() + Send + Sync>) {
            self.listeners.lock().unwrap().push(listener);
        }

        fn measure_process_private_bytes(&self, pid: i32) -> Option<u64> {
            self.process_private.lock().unwrap().get(&pid).copied()
        }
    }

    struct FakeCalibrationStore {
        stored: Mutex<Option<CapacityCalibration>>,
    }

    impl FakeCalibrationStore {
        fn new(stored: Option<CapacityCalibration>) -> Self {
            Self {
                stored: Mutex::new(stored),
            }
        }

        fn stored(&self) -> Option<CapacityCalibration> {
            *self.stored.lock().unwrap()
        }
    }

    impl CalibrationStore for FakeCalibrationStore {
        fn load(&self) -> Option<CapacityCalibration> {
            *self.stored.lock().unwrap()
        }

        fn save(&self, calibration: CapacityCalibration) {
            *self.stored.lock().unwrap() = Some(calibration);
        }
    }

    /// Avail/commit such that the safe zone is exactly `safe_zone`.
    fn provider_with_safe_zone(safe_zone: u64) -> Arc<FakeCapacityProvider> {
        let provider = FakeCapacityProvider::new();
        provider.set_available_phys(safe_zone + CapacityModel::OS_RESERVE_BYTES);
        provider.set_commit_headroom(safe_zone + CapacityModel::OS_RESERVE_BYTES);
        Arc::new(provider)
    }

    // ---- Safe-zone math (CapacityModelTests) --------------------------------------------------

    #[test]
    fn happy_path_max_terminals_is_floor_of_safe_zone_over_seed_budget() {
        // 8 GB safe zone / 200 MB seed = 40.96 → 40.
        let model = CapacityModel::new(provider_with_safe_zone(8 * GB), None);

        assert_eq!(8 * GB, model.safe_zone_bytes());
        assert_eq!(
            CapacityModel::SEED_BUDGET_BYTES,
            model.per_terminal_budget_bytes()
        );
        assert_eq!(40, model.max_terminals());
    }

    #[test]
    fn safe_zone_takes_the_min_of_phys_and_commit_headroom() {
        let provider = Arc::new(FakeCapacityProvider::new());
        provider.set_available_phys(10 * GB);
        provider.set_commit_headroom(4 * GB); // tighter side
        let model = CapacityModel::new(provider, None);

        assert_eq!(
            4 * GB - CapacityModel::OS_RESERVE_BYTES,
            model.safe_zone_bytes()
        );
    }

    #[test]
    fn safe_zone_saturates_at_zero_when_below_the_os_reserve() {
        let provider = Arc::new(FakeCapacityProvider::new());
        provider.set_available_phys(GB); // < 2 GB reserve
        provider.set_commit_headroom(GB);
        let model = CapacityModel::new(provider, None);

        assert_eq!(0, model.safe_zone_bytes());
        assert_eq!(0, model.max_terminals());
        assert!(model.try_reserve(SurfaceId(1)).is_none());
    }

    // ---- Reservation ledger --------------------------------------------------------------------

    #[test]
    fn reserve_then_commit_toctou_at_cap_minus_one_only_first_reserve_wins() {
        // Safe zone 400 MB / 200 MB budget → cap 2; one committed → at cap−1.
        let model = CapacityModel::new(provider_with_safe_zone(400 * MB), None);
        assert_eq!(2, model.max_terminals());
        let first = model.try_reserve(SurfaceId(1));
        assert!(first.is_some());
        model.commit(first.as_ref().unwrap(), 100);

        let second = model.try_reserve(SurfaceId(2));
        let third = model.try_reserve(SurfaceId(3));

        assert!(second.is_some()); // slot 2 of 2
        assert!(third.is_none()); // reserved-but-uncommitted still counts — no TOCTOU window
    }

    #[test]
    fn release_frees_a_slot() {
        let model = CapacityModel::new(provider_with_safe_zone(400 * MB), None); // cap 2
        let a = model.try_reserve(SurfaceId(1)).unwrap();
        let b = model.try_reserve(SurfaceId(2)).unwrap();
        model.commit(&a, 100);
        model.commit(&b, 101);
        assert!(model.try_reserve(SurfaceId(3)).is_none());

        model.release(SurfaceId(1));

        assert!(model.try_reserve(SurfaceId(3)).is_some());
    }

    #[test]
    fn release_of_an_uncommitted_reservation_frees_the_slot() {
        let model = CapacityModel::new(provider_with_safe_zone(400 * MB), None); // cap 2
        model.try_reserve(SurfaceId(1));
        model.try_reserve(SurfaceId(2));
        assert!(model.try_reserve(SurfaceId(3)).is_none());

        model.release(SurfaceId(2)); // e.g. factory threw

        assert!(model.try_reserve(SurfaceId(3)).is_some());
    }

    #[test]
    fn reserve_and_commit_are_idempotent_per_surface_id() {
        let model = CapacityModel::new(provider_with_safe_zone(400 * MB), None); // cap 2

        let first = model.try_reserve(SurfaceId(1));
        let again = model.try_reserve(SurfaceId(1)); // duplicate request
        model.commit(first.as_ref().unwrap(), 100);
        model.commit(first.as_ref().unwrap(), 100); // duplicate commit
        let after_commit = model.try_reserve(SurfaceId(1));

        assert!(first.is_some());
        assert!(again.is_some());
        assert!(after_commit.is_some()); // re-reserving a committed id is a no-op success
        let state = model.state();
        assert_eq!(1, state.used + state.reserved); // exactly one slot consumed
        assert!(model.try_reserve(SurfaceId(2)).is_some()); // second slot still free
    }

    // ---- Calibration ---------------------------------------------------------------------------

    #[test]
    fn three_measurements_above_seed_raise_budget_to_p75_and_drop_max() {
        let model = CapacityModel::new(provider_with_safe_zone(8 * GB), None); // max 40 at seed

        model.record_measurement(SurfaceId(1), 300 * MB);
        model.record_measurement(SurfaceId(2), 300 * MB);
        assert_eq!(
            CapacityModel::SEED_BUDGET_BYTES,
            model.per_terminal_budget_bytes()
        ); // < 3 samples
        model.record_measurement(SurfaceId(3), 300 * MB);

        assert_eq!(300 * MB, model.per_terminal_budget_bytes());
        assert_eq!(27, model.max_terminals()); // floor(8192/300)
    }

    #[test]
    fn calibration_never_drops_below_the_seed() {
        let model = CapacityModel::new(provider_with_safe_zone(8 * GB), None);

        model.record_measurement(SurfaceId(1), 50 * MB);
        model.record_measurement(SurfaceId(2), 50 * MB);
        model.record_measurement(SurfaceId(3), 50 * MB);

        assert_eq!(
            CapacityModel::SEED_BUDGET_BYTES,
            model.per_terminal_budget_bytes()
        ); // max(seed, P75)
        assert_eq!(40, model.max_terminals());
    }

    #[test]
    fn remeasuring_the_same_surface_replaces_its_sample_instead_of_adding_one() {
        let model = CapacityModel::new(provider_with_safe_zone(8 * GB), None);

        model.record_measurement(SurfaceId(1), 300 * MB);
        model.record_measurement(SurfaceId(1), 320 * MB);
        model.record_measurement(SurfaceId(1), 340 * MB);

        assert_eq!(
            CapacityModel::SEED_BUDGET_BYTES,
            model.per_terminal_budget_bytes()
        ); // still 1 sample
    }

    #[test]
    fn calibration_round_trips_through_the_store_with_matching_fingerprint() {
        let store = Arc::new(FakeCalibrationStore::new(None));
        let provider = provider_with_safe_zone(8 * GB);
        provider.set_total_phys(16 * GB);
        let model = CapacityModel::new(provider.clone(), Some(store.clone()));
        model.record_measurement(SurfaceId(1), 300 * MB);
        model.record_measurement(SurfaceId(2), 300 * MB);
        model.record_measurement(SurfaceId(3), 300 * MB);

        model.save_calibration();

        let stored = store.stored().unwrap();
        assert_eq!(300 * MB, stored.budget_bytes);
        assert_eq!(16, stored.hardware_fingerprint_gb);

        let fresh = CapacityModel::new(provider, Some(store));
        fresh.load_calibration();
        assert_eq!(300 * MB, fresh.per_terminal_budget_bytes());
        assert_eq!(27, fresh.max_terminals());
    }

    #[test]
    fn calibration_from_a_different_machine_is_ignored() {
        let store = Arc::new(FakeCalibrationStore::new(Some(CapacityCalibration {
            budget_bytes: 300 * MB,
            hardware_fingerprint_gb: 32, // 32 GB box
        })));
        let provider = provider_with_safe_zone(8 * GB);
        provider.set_total_phys(16 * GB); // this is a 16 GB box

        let model = CapacityModel::new(provider, Some(store));
        model.load_calibration();

        assert_eq!(
            CapacityModel::SEED_BUDGET_BYTES,
            model.per_terminal_budget_bytes()
        );
        assert_eq!(40, model.max_terminals());
    }

    // ---- Pressure + hysteresis -------------------------------------------------------------------

    #[test]
    fn low_memory_tick_drops_max_but_keeps_committed_terminals() {
        let provider = provider_with_safe_zone(8 * GB);
        let model = CapacityModel::new(provider.clone(), None); // max 40
        for i in 1..=6 {
            let token = model.try_reserve(SurfaceId(i)).unwrap();
            model.commit(&token, 100 + i);
        }

        provider.set_available_phys(3 * GB); // safe zone → 1 GB
        provider.set_commit_headroom(3 * GB);
        provider.raise_low_memory();
        model.on_tick();

        assert_eq!(5, model.max_terminals()); // floor(1024/200)
        assert_eq!(6, model.state().used); // live terminals never reaped
        assert_eq!(CapacityLevel::Cap, model.state().level);
        assert!(model.try_reserve(SurfaceId(7)).is_none()); // but no new spawns
    }

    #[test]
    fn one_recovered_tick_does_not_relax_the_cap_two_consecutive_do() {
        let provider = provider_with_safe_zone(8 * GB);
        let model = CapacityModel::new(provider.clone(), None); // max 40

        provider.set_available_phys(3 * GB);
        provider.set_commit_headroom(3 * GB);
        provider.raise_low_memory();
        model.on_tick();
        assert_eq!(5, model.max_terminals());

        provider.set_low_signaled(false);
        provider.set_available_phys(10 * GB); // ≥ 1.25 × 1 GB safe zone
        provider.set_commit_headroom(10 * GB);
        model.on_tick();
        assert_eq!(5, model.max_terminals()); // first recovered tick: hold

        model.on_tick();
        assert_eq!(40, model.max_terminals()); // second consecutive: relax
        assert_eq!(8 * GB, model.safe_zone_bytes());
    }

    #[test]
    fn recovery_streak_resets_if_a_tick_is_not_recovered() {
        let provider = provider_with_safe_zone(8 * GB);
        let model = CapacityModel::new(provider.clone(), None);

        provider.set_available_phys(3 * GB);
        provider.set_commit_headroom(3 * GB);
        provider.raise_low_memory();
        model.on_tick();
        assert_eq!(5, model.max_terminals());

        provider.set_low_signaled(false);
        provider.set_available_phys(10 * GB);
        provider.set_commit_headroom(10 * GB);
        model.on_tick(); // recovered tick #1

        provider.set_low_signaled(true); // pressure returns
        provider.set_available_phys(3 * GB);
        provider.set_commit_headroom(3 * GB);
        model.on_tick(); // streak broken
        assert_eq!(5, model.max_terminals());

        provider.set_low_signaled(false);
        provider.set_available_phys(10 * GB);
        provider.set_commit_headroom(10 * GB);
        model.on_tick(); // recovered tick #1 again
        assert_eq!(5, model.max_terminals()); // still held — streak restarted
        model.on_tick();
        assert_eq!(40, model.max_terminals());
    }

    #[test]
    fn low_memory_signal_alone_blocks_recovery_even_with_high_avail() {
        let provider = provider_with_safe_zone(8 * GB);
        let model = CapacityModel::new(provider.clone(), None);

        provider.set_available_phys(3 * GB);
        provider.set_commit_headroom(3 * GB);
        provider.raise_low_memory();
        model.on_tick();
        assert_eq!(5, model.max_terminals());

        provider.set_available_phys(10 * GB); // numbers look fine…
        provider.set_commit_headroom(10 * GB); // …but the OS signal is still raised
        model.on_tick();
        model.on_tick();
        assert_eq!(5, model.max_terminals());
    }

    // ---- Level mapping + state event -------------------------------------------------------------

    #[test]
    fn level_is_calm_below_75_percent_warn_at_75_cap_at_max() {
        let model = CapacityModel::new(provider_with_safe_zone(800 * MB), None); // cap 4

        assert_eq!(CapacityLevel::Calm, model.state().level); // 0/4

        model.commit(&model.try_reserve(SurfaceId(1)).unwrap(), 101);
        model.commit(&model.try_reserve(SurfaceId(2)).unwrap(), 102);
        assert_eq!(CapacityLevel::Calm, model.state().level); // 2/4 = 50%

        model.try_reserve(SurfaceId(3)); // reserved counts toward level
        assert_eq!(CapacityLevel::Warn, model.state().level); // 3/4 = 75%

        model.commit(&model.try_reserve(SurfaceId(3)).unwrap(), 103);
        model.commit(&model.try_reserve(SurfaceId(4)).unwrap(), 104);
        assert_eq!(CapacityLevel::Cap, model.state().level); // 4/4
    }

    #[test]
    fn state_changed_fires_on_ledger_and_tick_transitions() {
        let provider = provider_with_safe_zone(8 * GB);
        let model = CapacityModel::new(provider.clone(), None);
        let seen = Arc::new(Mutex::new(Vec::<CapacityState>::new()));
        let sink = Arc::clone(&seen);
        model.subscribe_state_changed(Box::new(move |s| sink.lock().unwrap().push(s)));

        let token = model.try_reserve(SurfaceId(1)).unwrap();
        model.commit(&token, 100);
        model.release(SurfaceId(1));
        provider.set_available_phys(3 * GB);
        provider.set_commit_headroom(3 * GB);
        model.on_tick();

        let seen = seen.lock().unwrap();
        assert_eq!(4, seen.len());
        assert_eq!(CapacityState::new(0, 1, 40, CapacityLevel::Calm), seen[0]);
        assert_eq!(CapacityState::new(1, 0, 40, CapacityLevel::Calm), seen[1]);
        assert_eq!(CapacityState::new(0, 0, 40, CapacityLevel::Calm), seen[2]);
        assert_eq!(CapacityState::new(0, 0, 5, CapacityLevel::Calm), seen[3]);
    }

    #[test]
    fn unchanged_tick_does_not_fire_state_changed() {
        let model = CapacityModel::new(provider_with_safe_zone(8 * GB), None);
        let fired = Arc::new(Mutex::new(0));
        let sink = Arc::clone(&fired);
        model.subscribe_state_changed(Box::new(move |_| *sink.lock().unwrap() += 1));

        model.on_tick(); // same provider numbers → same state

        assert_eq!(0, *fired.lock().unwrap());
    }

    // ---- CapacityIndicatorViewModelTests --------------------------------------------------------

    /// Avail/commit such that the safe zone yields exactly `max_terminals` slots.
    fn model_with_max(max_terminals: i32) -> Arc<CapacityModel> {
        let safe_zone = max_terminals as u64 * CapacityModel::SEED_BUDGET_BYTES;
        let model = CapacityModel::new(provider_with_safe_zone(safe_zone), None);
        assert_eq!(max_terminals, model.max_terminals()); // guard the fixture
        Arc::new(model)
    }

    /// Records dispatched actions; runs them only when told to (proves marshalling).
    struct FakeDispatcher {
        pending: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    }

    impl FakeDispatcher {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                pending: Mutex::new(Vec::new()),
            })
        }

        fn dispatcher(self: &Arc<Self>) -> Dispatcher {
            let me = Arc::clone(self);
            Arc::new(move |action| me.pending.lock().unwrap().push(action))
        }

        fn pending_count(&self) -> usize {
            self.pending.lock().unwrap().len()
        }

        fn run_all(&self) {
            let drained: Vec<_> = self.pending.lock().unwrap().drain(..).collect();
            for action in drained {
                action();
            }
        }
    }

    fn inline() -> Dispatcher {
        Arc::new(|action: Box<dyn FnOnce() + Send>| action())
    }

    // ---- Null model ------------------------------------------------------------------------------

    #[test]
    fn null_model_shows_dash_placeholder_and_never_caps() {
        let vm = CapacityIndicatorViewModel::new(None, inline());

        assert_eq!("— / — terminals", vm.label_text());
        assert_eq!(0.0, vm.fraction_used());
        assert_eq!(CapacityLevel::Calm, vm.level());
        assert!(!vm.is_at_cap());
        assert!(vm.hint_text().is_none());
    }

    // C# `Null_dispatcher_throws` is not portable: `Dispatcher` cannot be null in Rust —
    // the type system enforces what the C# constructor guard checked at runtime.

    // ---- Label / fraction / level from a live model ------------------------------------------------

    #[test]
    fn label_counts_used_plus_reserved_over_max() {
        let model = model_with_max(10);
        let committed = model.try_reserve(SurfaceId(1)).unwrap();
        model.commit(&committed, 100);
        model.try_reserve(SurfaceId(2)); // stays reserved

        let vm = CapacityIndicatorViewModel::new(Some(model), inline());

        assert_eq!("2 / 10 terminals", vm.label_text());
        assert!((vm.fraction_used() - 0.2).abs() < 1e-10);
        assert_eq!(CapacityLevel::Calm, vm.level());
        assert!(!vm.is_at_cap());
    }

    #[test]
    fn level_is_passed_through_from_state_not_recomputed() {
        let model = model_with_max(4);
        for i in 1..=3 {
            // 3/4 = 75% → Warn per the model
            model.try_reserve(SurfaceId(i));
        }

        let vm = CapacityIndicatorViewModel::new(Some(Arc::clone(&model)), inline());

        assert_eq!(model.state().level, vm.level());
        assert_eq!(CapacityLevel::Warn, vm.level());
        assert!(!vm.is_at_cap());
        assert!(vm.hint_text().is_none());
    }

    #[test]
    fn at_cap_sets_is_at_cap_and_hint() {
        let model = model_with_max(2);
        model.try_reserve(SurfaceId(1));
        model.try_reserve(SurfaceId(2));

        let vm = CapacityIndicatorViewModel::new(Some(model), inline());

        assert!(vm.is_at_cap());
        assert_eq!(Some(CapacityIndicatorViewModel::CAP_HINT), vm.hint_text());
        assert_eq!(1.0, vm.fraction_used());
    }

    #[test]
    fn fraction_is_zero_when_max_is_zero() {
        // Starved machine: safe zone 0 → max 0 (and level == Cap, 0 ≥ 0).
        let provider = Arc::new(FakeCapacityProvider::new());
        provider.set_available_phys(CapacityModel::OS_RESERVE_BYTES); // safe zone exactly 0
        provider.set_commit_headroom(CapacityModel::OS_RESERVE_BYTES);
        let model = Arc::new(CapacityModel::new(provider, None));
        assert_eq!(0, model.max_terminals());

        let vm = CapacityIndicatorViewModel::new(Some(model), inline());

        assert_eq!(0.0, vm.fraction_used());
        assert_eq!("0 / 0 terminals", vm.label_text());
        assert!(vm.is_at_cap());
    }

    // ---- Marshalling -----------------------------------------------------------------------------

    #[test]
    fn state_changes_marshal_through_the_injected_dispatcher() {
        let model = model_with_max(10);
        let dispatcher = FakeDispatcher::new();
        let vm = CapacityIndicatorViewModel::new(Some(Arc::clone(&model)), dispatcher.dispatcher());
        let raised = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&raised);
        vm.subscribe_property_changed(Box::new(move |name| {
            sink.lock().unwrap().push(name.to_string())
        }));

        model.try_reserve(SurfaceId(1));

        // Until the dispatcher runs, the VM still shows the snapshot it was built with.
        assert_eq!(1, dispatcher.pending_count());
        assert_eq!("0 / 10 terminals", vm.label_text());
        assert!(raised.lock().unwrap().is_empty());

        dispatcher.run_all();

        assert_eq!("1 / 10 terminals", vm.label_text());
        let raised = raised.lock().unwrap();
        assert!(raised.iter().any(|n| n == "label_text"));
        assert!(raised.iter().any(|n| n == "fraction_used"));
        assert!(raised.iter().any(|n| n == "level"));
        assert!(raised.iter().any(|n| n == "is_at_cap"));
        assert!(raised.iter().any(|n| n == "hint_text"));
    }

    #[test]
    fn reaching_cap_via_state_change_flips_is_at_cap_and_hint() {
        let model = model_with_max(2);
        let vm = CapacityIndicatorViewModel::new(Some(Arc::clone(&model)), inline());
        assert!(!vm.is_at_cap());

        model.try_reserve(SurfaceId(1));
        model.try_reserve(SurfaceId(2));

        assert!(vm.is_at_cap());
        assert_eq!(Some(CapacityIndicatorViewModel::CAP_HINT), vm.hint_text());

        model.release(SurfaceId(2));

        assert!(!vm.is_at_cap());
        assert!(vm.hint_text().is_none());
    }

    // ---- Dispose ---------------------------------------------------------------------------------

    #[test]
    fn dispose_unsubscribes_from_the_model() {
        let model = model_with_max(10);
        let dispatcher = FakeDispatcher::new();
        let vm = CapacityIndicatorViewModel::new(Some(Arc::clone(&model)), dispatcher.dispatcher());

        vm.dispose();
        model.try_reserve(SurfaceId(1));

        assert_eq!(0, dispatcher.pending_count());
        assert_eq!("0 / 10 terminals", vm.label_text()); // frozen at the pre-dispose snapshot
    }

    #[test]
    fn dispose_is_idempotent_and_drops_late_dispatched_updates() {
        let model = model_with_max(10);
        let dispatcher = FakeDispatcher::new();
        let vm = CapacityIndicatorViewModel::new(Some(Arc::clone(&model)), dispatcher.dispatcher());

        model.try_reserve(SurfaceId(1)); // queued on the fake dispatcher
        vm.dispose();
        vm.dispose();
        dispatcher.run_all(); // the queued update arrives after dispose → ignored

        assert_eq!("0 / 10 terminals", vm.label_text());
    }
}
