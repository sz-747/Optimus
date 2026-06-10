//! Spike 5 (plan §8): FFI / host-callback thread-safety smoke test.
//!
//! This drives the real `#[no_mangle] extern "C"` ABI surface from a host's perspective —
//! the same entry points the C# WinUI shell calls — and validates the parts of the callback
//! contract that can only break across threads:
//!
//!   1. The host callback is invoked from an engine **worker thread** (the render thread,
//!      where `AlertHandler` fires synchronously inside `advance_bytes`), not the thread that
//!      registered it. The C# side relies on this — it must `DispatcherQueue.TryEnqueue` back
//!      to the UI thread — so the test asserts the callback genuinely runs off the registering
//!      thread and can safely read its `user_data` from there.
//!   2. `optimus_engine_destroy` performs **ordered teardown** (stop the render thread → join the
//!      reader → drop the PTY). No callback may run after destroy returns, so the `user_data`
//!      the host frees afterwards is never touched by a straggler worker.
//!   3. Rapid create/destroy churn — including destroying while a child + reader thread are
//!      still live — never crashes, deadlocks, or leaks a worker.
//!   4. Multiple **independent engines** (each with its own worker threads and callback) run
//!      concurrently without interfering through shared/static state.
//!
//! No GPU or `SwapChainPanel` is involved — the renderer attach is skipped, exactly as in the
//! U4 engine-core test.
//!
//! ## Why a TITLE event, not CHILD_EXIT
//! The cross-thread trigger is a child-emitted OSC 0 set-title sequence: `cmd.exe` echoes raw
//! `ESC ] 0 ; <marker> BEL` bytes, ConPTY forwards the title change, `wezterm-term` parses it
//! and the `AlertHandler` emits `event_kind::TITLE` on the render thread. This is deterministic
//! and fast. `CHILD_EXIT` is *not* used here: under ConPTY the output pipe stays open after the
//! child exits (the pseudoconsole closes only during teardown), so a child-exit event cannot be
//! observed on a live engine in Phase 1 — and Phase 1 does not surface events anyway (plan §7).
//! TITLE and CHILD_EXIT travel the identical `EventSink` → C-callback → `user_data` path, so
//! exercising TITLE fully validates the thread-safety property this spike exists to de-risk.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use optimus_engine::event_kind;
use optimus_engine::ffi::events::HostEvent;
use optimus_engine::{
    optimus_engine_create, optimus_engine_destroy, optimus_engine_resize, optimus_engine_send_text,
    optimus_engine_set_event_callback, optimus_engine_spawn_shell,
};

/// Shared, `Sync` state the C-ABI callback writes into via its opaque `user_data` token.
/// The host (this test) owns it behind an `Arc` and keeps it alive past `optimus_engine_destroy`,
/// mirroring the C# contract that the `GCHandle` stays rooted for the engine's lifetime.
struct Counters {
    /// Total callbacks observed (any kind).
    events: AtomicUsize,
    /// Set once a `TITLE` event arrives (the sequence we deliberately trigger).
    title_seen: AtomicBool,
    /// Set if any callback ran on a thread other than `main_thread` (the cross-thread hop).
    foreign_thread: AtomicBool,
    /// The thread that registered the callback; callbacks must arrive on a *different* one.
    main_thread: ThreadId,
}

impl Counters {
    fn new(main_thread: ThreadId) -> Arc<Self> {
        Arc::new(Self {
            events: AtomicUsize::new(0),
            title_seen: AtomicBool::new(false),
            foreign_thread: AtomicBool::new(false),
            main_thread,
        })
    }

    /// True once at least one callback has arrived on a worker (non-registering) thread.
    fn observed_cross_thread_event(&self) -> bool {
        self.events.load(Ordering::SeqCst) > 0 && self.foreign_thread.load(Ordering::SeqCst)
    }
}

/// The C-ABI host callback. Invoked from the engine's render thread with a borrowed
/// `HostEvent`; `user_data` is the `*const Counters` the test registered.
extern "C" fn on_event(user_data: *mut c_void, ev: *const HostEvent) {
    if user_data.is_null() || ev.is_null() {
        return;
    }
    // SAFETY: `user_data` is the `Arc<Counters>` pointer registered below; the test keeps the
    // Arc alive until after `optimus_engine_destroy`, so it is valid for every callback.
    let counters = unsafe { &*(user_data as *const Counters) };
    // SAFETY: the event pointer is valid for the duration of this call (the engine owns the
    // backing storage and only borrows it to us). We read scalars/flags; we copy nothing.
    let ev = unsafe { &*ev };

    counters.events.fetch_add(1, Ordering::SeqCst);
    if thread::current().id() != counters.main_thread {
        counters.foreign_thread.store(true, Ordering::SeqCst);
    }
    if ev.kind == event_kind::TITLE {
        counters.title_seen.store(true, Ordering::SeqCst);
    }
}

/// Spin until `pred` holds or `timeout` elapses. Returns whether the predicate became true.
fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        if pred() {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// Register `counters` as the callback target on `engine`. The Arc must stay alive past destroy.
fn register(engine: *mut optimus_engine::engine::Engine, counters: &Arc<Counters>) {
    let user_data = Arc::as_ptr(counters) as *mut c_void;
    // SAFETY: `engine` is a live handle; `on_event` is a valid C-ABI fn; `user_data` stays
    // valid for the engine's lifetime because the caller holds the Arc.
    unsafe { optimus_engine_set_event_callback(engine, on_event, user_data) };
}

/// Spawn `cmd.exe` echoing a raw `ESC ] 0 ; <marker> BEL` OSC set-title sequence, which
/// propagates through ConPTY → `wezterm-term` → `AlertHandler` as a `TITLE` event on the
/// render thread. Panics if the spawn fails.
fn spawn_title_emitter(engine: *mut optimus_engine::engine::Engine, marker: &str) {
    let esc = '\u{1b}';
    let bel = '\u{7}';
    let cmdline = format!("cmd.exe /c echo {esc}]0;{marker}{bel}");
    // SAFETY: `engine` live; `cmdline` is valid UTF-8 for its length; cwd is null/0 (inherit).
    let rc = unsafe {
        optimus_engine_spawn_shell(
            engine,
            cmdline.as_ptr(),
            cmdline.len(),
            std::ptr::null(),
            0,
        )
    };
    assert_eq!(rc, 0, "spawn_shell returned {rc} for the title emitter");
}

/// Full happy-path round trip over the C ABI: the callback must fire from a worker thread with
/// a `TITLE` event, and no callback may run after `optimus_engine_destroy`.
#[test]
fn callback_fires_from_worker_thread_and_teardown_is_clean() {
    let counters = Counters::new(thread::current().id());

    // SAFETY: null opts → defaults; returned handle is freed once via destroy below.
    let engine = unsafe { optimus_engine_create(std::ptr::null()) };
    assert!(!engine.is_null(), "engine create returned null");

    register(engine, &counters);

    // Grid dims via the cols/rows path (no surface) — mirrors the U4 test's headless resize.
    // SAFETY: live engine.
    let rc = unsafe { optimus_engine_resize(engine, 80, 24, 0, 0, 1.0) };
    assert_eq!(rc, 0, "resize returned {rc}");

    spawn_title_emitter(engine, "spike5_title");

    let saw_title = wait_until(Duration::from_secs(20), || {
        counters.title_seen.load(Ordering::SeqCst)
    });
    assert!(saw_title, "the TITLE event never reached the host callback");
    assert!(
        counters.foreign_thread.load(Ordering::SeqCst),
        "callback ran only on the registering thread — the cross-thread hop the C# host \
         depends on did not happen"
    );

    // Teardown contract: destroy stops + joins the workers. Snapshot the event count, destroy,
    // then confirm no straggler callback bumped it after the threads were joined.
    let before = counters.events.load(Ordering::SeqCst);
    // SAFETY: a live handle, destroyed exactly once.
    unsafe { optimus_engine_destroy(engine) };
    thread::sleep(Duration::from_millis(100));
    let after = counters.events.load(Ordering::SeqCst);
    assert_eq!(
        before, after,
        "a callback fired after destroy — a worker outlived ordered teardown"
    );

    // The Arc lived for the whole engine lifetime; only now is it safe to release.
    drop(counters);
}

/// Rapid create → configure → spawn → destroy churn, destroying *while* the child and reader
/// thread are still live. Ordered teardown must never crash, deadlock, or leave a worker.
#[test]
fn rapid_create_destroy_churn_is_safe() {
    for i in 0..16 {
        let counters = Counters::new(thread::current().id());
        // SAFETY: null opts → defaults; freed once below.
        let engine = unsafe { optimus_engine_create(std::ptr::null()) };
        assert!(!engine.is_null(), "create returned null on iteration {i}");

        register(engine, &counters);
        // SAFETY: live engine.
        unsafe { optimus_engine_resize(engine, 40, 12, 0, 0, 1.0) };

        // Sending text before a shell exists must be a harmless no-op (no PTY yet).
        let text = "noop";
        // SAFETY: live engine; valid UTF-8 ptr+len.
        unsafe { optimus_engine_send_text(engine, text.as_ptr(), text.len()) };

        spawn_title_emitter(engine, "churn");
        // Deliberately do NOT wait: destroy races the live reader/child to stress the shutdown
        // ordering (close pseudoconsole → join reader → drop terminal).
        // SAFETY: live handle, destroyed once.
        unsafe { optimus_engine_destroy(engine) };
        drop(counters);
    }
}

/// Many independent engines, each with its own worker threads and callback, running at once.
/// Validates there is no interference through shared/static state (e.g. the thread-local last
/// error) and that every engine independently delivers its event off its own threads.
#[test]
fn concurrent_independent_engines_dont_interfere() {
    const ENGINES: usize = 6;

    let handles: Vec<_> = (0..ENGINES)
        .map(|n| {
            thread::spawn(move || {
                let counters = Counters::new(thread::current().id());
                // SAFETY: null opts → defaults; freed once below.
                let engine = unsafe { optimus_engine_create(std::ptr::null()) };
                assert!(!engine.is_null(), "engine {n} create returned null");

                register(engine, &counters);
                // SAFETY: live engine.
                unsafe { optimus_engine_resize(engine, 80, 24, 0, 0, 1.0) };

                spawn_title_emitter(engine, &format!("engine_{n}"));

                let ok = wait_until(Duration::from_secs(20), || {
                    counters.observed_cross_thread_event()
                });
                assert!(
                    ok,
                    "engine {n}: no callback arrived from a worker thread within the timeout"
                );

                // SAFETY: live handle, destroyed once.
                unsafe { optimus_engine_destroy(engine) };
                drop(counters);
            })
        })
        .collect();

    for (n, h) in handles.into_iter().enumerate() {
        h.join().unwrap_or_else(|_| panic!("engine thread {n} panicked"));
    }
}
