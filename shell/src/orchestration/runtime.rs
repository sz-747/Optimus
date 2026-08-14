//! Supervised agent processes. The production runtime uses the existing ConPTY engine and Job
//! Object teardown; the supervisor owns lifecycle state and never exposes PTY bytes to the control
//! plane.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use optimus_engine::{Engine, EngineEvent, EngineOptions};
use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};

use crate::child_environment::{self, FILE_SCOPE_ENV, MODEL_API_CREDENTIALS};
use crate::domain::capacity::CapacityModel;
use crate::domain::ids::SurfaceId;
use crate::orchestration::capture::CompletionCapture;
use crate::orchestration::model::{Agent, AgentId, AgentState};
use crate::orchestration::store::{AgentModelAuth, OrchestrationError, OrchestrationStore};

const ORCHESTRATED_SURFACE_OFFSET: i64 = 1_000_000;
const OUTPUT_BUFFER_BYTES: usize = 256 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeSpec {
    pub agent_id: AgentId,
    pub surface_id: SurfaceId,
    pub session_id: String,
    pub workspace_id: String,
    pub branch: String,
    pub command: String,
    pub cwd: PathBuf,
    pub pipe_name: String,
    pub caller_capability: String,
    pub environment_overrides: Vec<(String, String)>,
}

impl std::fmt::Debug for RuntimeSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeSpec")
            .field("agent_id", &self.agent_id)
            .field("surface_id", &self.surface_id)
            .field("session_id", &self.session_id)
            .field("workspace_id", &self.workspace_id)
            .field("branch", &self.branch)
            .field("command", &self.command)
            .field("cwd", &self.cwd)
            .field("pipe_name", &self.pipe_name)
            .field("caller_capability", &"[redacted]")
            .field(
                "environment_overrides",
                &self
                    .environment_overrides
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeEvent {
    Output { byte_count: usize },
    Toast { title: String, body: String },
    Exit { code: i32 },
}

pub type RuntimeSink = Arc<dyn Fn(RuntimeEvent) + Send + Sync>;
pub type RuntimeObserver = Arc<dyn Fn(AgentId, RuntimeEvent) + Send + Sync>;

pub trait RunningAgent: Send + Sync {
    fn stop(&self);
    fn pid(&self) -> u32;
    fn output_snapshot(&self) -> Vec<u8>;
}

pub trait AgentRuntime: Send + Sync {
    fn start(&self, spec: RuntimeSpec, sink: RuntimeSink) -> Result<Arc<dyn RunningAgent>, String>;
}

#[derive(Clone, Copy)]
pub struct EngineRuntime {
    job_memory_limit_bytes: usize,
}

impl EngineRuntime {
    pub fn new(job_memory_limit_bytes: usize) -> Self {
        Self {
            job_memory_limit_bytes,
        }
    }
}

struct EngineAgent {
    engine: Mutex<Option<Engine>>,
    output: Arc<Mutex<VecDeque<u8>>>,
}

impl RunningAgent for EngineAgent {
    fn stop(&self) {
        let _ = self
            .engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    fn pid(&self) -> u32 {
        self.engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(0, Engine::child_pid)
    }

    fn output_snapshot(&self) -> Vec<u8> {
        self.output
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .copied()
            .collect()
    }
}

impl AgentRuntime for EngineRuntime {
    fn start(&self, spec: RuntimeSpec, sink: RuntimeSink) -> Result<Arc<dyn RunningAgent>, String> {
        let output = Arc::new(Mutex::new(VecDeque::with_capacity(OUTPUT_BUFFER_BYTES)));
        let output_buffer = Arc::clone(&output);
        let output_sink = Arc::clone(&sink);
        let event_sink = Arc::clone(&sink);
        let mut engine = Engine::new(
            EngineOptions {
                job_memory_limit_bytes: self.job_memory_limit_bytes,
                ..EngineOptions::default()
            },
            Box::new(move |bytes| {
                let mut buffer = output_buffer.lock().unwrap_or_else(PoisonError::into_inner);
                let overflow = buffer
                    .len()
                    .saturating_add(bytes.len())
                    .saturating_sub(OUTPUT_BUFFER_BYTES);
                let drain_len = overflow.min(buffer.len());
                buffer.drain(..drain_len);
                buffer.extend(bytes);
                drop(buffer);
                output_sink(RuntimeEvent::Output {
                    byte_count: bytes.len(),
                });
            }),
            Box::new(move |event| match event {
                EngineEvent::Toast { title, body } => {
                    event_sink(RuntimeEvent::Toast { title, body });
                }
                EngineEvent::ChildExit { code } => event_sink(RuntimeEvent::Exit { code }),
            }),
        )
        .map_err(|error| error.to_string())?;
        let agent_id = spec.agent_id.to_string();
        let worktree = spec.cwd.to_string_lossy();
        let environment = child_environment::for_agent(child_environment::AgentEnvironment {
            surface: spec.surface_id,
            pipe_name: &spec.pipe_name,
            agent_id: &agent_id,
            session_id: &spec.session_id,
            workspace_id: &spec.workspace_id,
            branch: &spec.branch,
            worktree: &worktree,
            caller_capability: &spec.caller_capability,
        });
        let mut environment = environment;
        environment.extend(spec.environment_overrides);
        engine
            .spawn_shell_with_environment(&spec.command, spec.cwd.to_str(), &environment)
            .map_err(|error| error.to_string())?;
        Ok(Arc::new(EngineAgent {
            engine: Mutex::new(Some(engine)),
            output,
        }))
    }
}

pub struct RuntimeSupervisor<R: AgentRuntime> {
    store: Arc<OrchestrationStore>,
    runtime: R,
    pipe_name: String,
    capacity: Option<Arc<CapacityModel>>,
    handles: Arc<Mutex<HashMap<AgentId, Arc<dyn RunningAgent>>>>,
    lifecycle: Mutex<()>,
    observer: RuntimeObserver,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    completion_capture: Option<Arc<dyn CompletionCapture>>,
}

#[derive(Default)]
struct StartupGate {
    live: bool,
    pending: Vec<RuntimeEvent>,
}

impl<R: AgentRuntime> RuntimeSupervisor<R> {
    pub fn new(
        store: Arc<OrchestrationStore>,
        runtime: R,
        pipe_name: String,
        capacity: Option<Arc<CapacityModel>>,
        observer: RuntimeObserver,
    ) -> Self {
        Self {
            store,
            runtime,
            pipe_name,
            capacity,
            handles: Arc::new(Mutex::new(HashMap::new())),
            lifecycle: Mutex::new(()),
            observer,
            now_ms: Arc::new(now_ms),
            completion_capture: None,
        }
    }

    #[cfg(test)]
    fn with_clock(
        store: Arc<OrchestrationStore>,
        runtime: R,
        pipe_name: String,
        observer: RuntimeObserver,
        now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            store,
            runtime,
            pipe_name,
            capacity: None,
            handles: Arc::new(Mutex::new(HashMap::new())),
            lifecycle: Mutex::new(()),
            observer,
            now_ms,
            completion_capture: None,
        }
    }

    pub fn with_completion_capture(mut self, capture: Arc<dyn CompletionCapture>) -> Self {
        self.completion_capture = Some(capture);
        self
    }

    pub fn start_agent(&self, id: AgentId) -> Result<Agent, OrchestrationError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| OrchestrationError::Invariant(format!("unknown agent {id}")))?;
        if agent.state != AgentState::Ready {
            return Err(OrchestrationError::Invariant(format!(
                "agent {id} is {}, not ready",
                agent.state.as_str()
            )));
        }
        let cwd = agent
            .worktree_path
            .clone()
            .ok_or_else(|| OrchestrationError::Invariant(format!("agent {id} has no worktree")))?;
        let mut environment_overrides =
            model_environment(self.store.agent_model_auth(id)?, |name| {
                std::env::var(name).ok()
            })?;
        environment_overrides.push((
            FILE_SCOPE_ENV.to_string(),
            serde_json::to_string(
                &agent
                    .file_scope
                    .iter()
                    .map(|scope| scope.pattern.as_str())
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| OrchestrationError::Runtime(error.to_string()))?,
        ));
        let surface = surface_for_agent(id)?;
        let reservation = if let Some(capacity) = &self.capacity {
            Some(capacity.try_reserve(surface).ok_or_else(|| {
                OrchestrationError::Runtime(
                    "safe-zone full; stop an agent or terminal before starting another".to_string(),
                )
            })?)
        } else {
            None
        };
        let started = match self.store.mark_running(id, surface, (self.now_ms)()) {
            Ok(started) => started,
            Err(error) => {
                if let Some(capacity) = &self.capacity {
                    capacity.release(surface);
                }
                return Err(error);
            }
        };
        let caller_capability = match generate_caller_capability().and_then(|capability| {
            self.store
                .set_agent_capability(id, &capability, (self.now_ms)())?;
            Ok(capability)
        }) {
            Ok(capability) => capability,
            Err(error) => {
                if let Some(capacity) = &self.capacity {
                    capacity.release(surface);
                }
                let _ = self
                    .store
                    .mark_failed(id, &error.to_string(), (self.now_ms)());
                return Err(error);
            }
        };

        let event_store = Arc::clone(&self.store);
        let observer = Arc::clone(&self.observer);
        let now = Arc::clone(&self.now_ms);
        let event_capacity = self.capacity.clone();
        let event_handles = Arc::clone(&self.handles);
        let event_completion = self.completion_capture.clone();
        let startup = Arc::new(Mutex::new(StartupGate::default()));
        let event_startup = Arc::clone(&startup);
        let sink: RuntimeSink = Arc::new(move |event| {
            let mut startup = event_startup.lock().unwrap_or_else(PoisonError::into_inner);
            if !startup.live {
                startup.pending.push(event);
                return;
            }
            drop(startup);
            process_runtime_event(
                &event_store,
                &event_capacity,
                &event_handles,
                &observer,
                &now,
                &event_completion,
                id,
                surface,
                event,
            );
        });
        let spec = RuntimeSpec {
            agent_id: id,
            surface_id: surface,
            session_id: started.session_id.to_string(),
            workspace_id: started.workspace_id.to_string(),
            branch: started.branch.clone(),
            command: started.command.clone(),
            cwd,
            pipe_name: self.pipe_name.clone(),
            caller_capability,
            environment_overrides,
        };
        match self.runtime.start(spec, sink) {
            Ok(handle) => {
                let mut startup = startup.lock().unwrap_or_else(PoisonError::into_inner);
                let exited_during_start = startup
                    .pending
                    .iter()
                    .any(|event| matches!(event, RuntimeEvent::Exit { .. }));
                if exited_during_start {
                    startup.live = true;
                    let pending = std::mem::take(&mut startup.pending);
                    drop(startup);
                    for event in pending {
                        process_runtime_event(
                            &self.store,
                            &self.capacity,
                            &self.handles,
                            &self.observer,
                            &self.now_ms,
                            &self.completion_capture,
                            id,
                            surface,
                            event,
                        );
                    }
                    handle.stop();
                    if let Some(capture) = &self.completion_capture {
                        capture.enqueue(id);
                    }
                    return self.store.agent(id)?.ok_or_else(|| {
                        OrchestrationError::Invariant(format!(
                            "agent {id} disappeared during start"
                        ))
                    });
                }
                self.handles
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(id, handle);
                if let (Some(capacity), Some(reservation)) = (&self.capacity, &reservation) {
                    let pid = self
                        .handles
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get(&id)
                        .map_or(0, |handle| handle.pid());
                    capacity.commit(reservation, pid as i32);
                }
                startup.live = true;
                let pending = std::mem::take(&mut startup.pending);
                drop(startup);
                for event in pending {
                    process_runtime_event(
                        &self.store,
                        &self.capacity,
                        &self.handles,
                        &self.observer,
                        &self.now_ms,
                        &self.completion_capture,
                        id,
                        surface,
                        event,
                    );
                }
                self.store.agent(id)?.ok_or_else(|| {
                    OrchestrationError::Invariant(format!("agent {id} disappeared after start"))
                })
            }
            Err(error) => {
                if let Some(capacity) = &self.capacity {
                    capacity.release(surface);
                }
                self.store.mark_failed(id, &error, (self.now_ms)())?;
                Err(OrchestrationError::Runtime(error))
            }
        }
    }

    pub fn stop_agent(&self, id: AgentId) -> Result<Agent, OrchestrationError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| OrchestrationError::Invariant(format!("unknown agent {id}")))?;
        if agent.state.is_terminal() {
            if let Some(handle) = self
                .handles
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id)
            {
                handle.stop();
            }
            if let Some(capture) = &self.completion_capture {
                capture.enqueue(id);
            }
            return Ok(agent);
        }
        if agent.state == AgentState::Running {
            self.store.mark_stopping(id, (self.now_ms)())?;
        } else if agent.state != AgentState::Stopping {
            return Err(OrchestrationError::Invariant(format!(
                "agent {id} cannot stop from {}",
                agent.state.as_str()
            )));
        }

        let stopped = self
            .store
            .mark_interrupted(id, "Stopped by user", (self.now_ms)())?;
        if let Some(handle) = self
            .handles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
        {
            handle.stop();
        }
        if let Some(capture) = &self.completion_capture {
            capture.enqueue(id);
        }
        if let Some(capacity) = &self.capacity {
            if let Some(surface) = agent.surface_id.map(SurfaceId) {
                capacity.release(surface);
            }
        }
        Ok(stopped)
    }

    pub fn output_snapshot(&self, id: AgentId) -> Vec<u8> {
        self.handles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .map_or_else(Vec::new, |handle| handle.output_snapshot())
    }

    pub fn active_count(&self) -> usize {
        self.handles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn stop_all(&self) {
        let ids = self
            .handles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.stop_agent(id);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_runtime_event(
    store: &Arc<OrchestrationStore>,
    capacity: &Option<Arc<CapacityModel>>,
    handles: &Arc<Mutex<HashMap<AgentId, Arc<dyn RunningAgent>>>>,
    observer: &RuntimeObserver,
    now: &Arc<dyn Fn() -> i64 + Send + Sync>,
    completion_capture: &Option<Arc<dyn CompletionCapture>>,
    id: AgentId,
    surface: SurfaceId,
    event: RuntimeEvent,
) {
    let timestamp = now();
    match event {
        RuntimeEvent::Output { .. } => {
            let _ = store.touch_output(id, timestamp);
        }
        RuntimeEvent::Exit { code } => {
            persist_runtime_exit(store, id, code, timestamp);
            let handles = Arc::clone(handles);
            let completion_capture = completion_capture.clone();
            if let Err(error) = std::thread::Builder::new()
                .name(format!("optimus-agent-reaper-{}", id.0))
                .spawn(move || {
                    let handle = handles
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&id);
                    if let Some(handle) = handle {
                        drop(handle);
                        if let Some(capture) = completion_capture {
                            capture.enqueue(id);
                        }
                    }
                })
            {
                eprintln!("failed to start runtime reaper for {id}: {error}");
            }
            if let Some(capacity) = capacity {
                capacity.release(surface);
            }
        }
        RuntimeEvent::Toast { .. } => {}
    }
    observer(id, event);
}

fn persist_runtime_exit(store: &Arc<OrchestrationStore>, id: AgentId, code: i32, ts: i64) {
    let mut last_error = None;
    for attempt in 0..3 {
        match store.mark_exited(id, code, ts) {
            Ok(_) => return,
            Err(error) => {
                if store
                    .agent(id)
                    .ok()
                    .flatten()
                    .is_some_and(|agent| agent.state.is_terminal())
                {
                    return;
                }
                last_error = Some(error);
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(25 << attempt));
                }
            }
        }
    }
    if let Some(error) = last_error {
        eprintln!("failed to persist terminal state for {id}: {error}");
    }
}

fn surface_for_agent(id: AgentId) -> Result<SurfaceId, OrchestrationError> {
    let value = ORCHESTRATED_SURFACE_OFFSET
        .checked_add(id.0)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| {
            OrchestrationError::Invariant("agent id exceeds surface range".to_string())
        })?;
    Ok(SurfaceId(value))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

fn generate_caller_capability() -> Result<String, OrchestrationError> {
    let mut bytes = [0_u8; 32];
    // SAFETY: BCrypt fills the provided initialized buffer and retains no pointer.
    let status = unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if !status.is_ok() {
        return Err(OrchestrationError::Runtime(
            "secure caller capability generation failed".to_string(),
        ));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn model_environment(
    policy: Option<AgentModelAuth>,
    read: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(String, String)>, OrchestrationError> {
    let Some(policy) = policy else {
        return Ok(Vec::new());
    };
    let mut environment = MODEL_API_CREDENTIALS
        .iter()
        .map(|name| ((*name).to_string(), String::new()))
        .collect::<Vec<_>>();
    match policy.auth_mode.as_str() {
        "subscription" => Ok(environment),
        "api" => {
            let name = policy
                .credential_env
                .as_deref()
                .filter(|name| MODEL_API_CREDENTIALS.contains(name))
                .ok_or_else(|| {
                    OrchestrationError::Runtime(
                        "API model profile has no approved credential environment".to_string(),
                    )
                })?;
            let value = read(name)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    OrchestrationError::Runtime(format!(
                        "API model profile requires {name} in the desktop environment"
                    ))
                })?;
            if let Some((_, current)) = environment.iter_mut().find(|(key, _)| key == name) {
                *current = value;
            }
            Ok(environment)
        }
        _ => Err(OrchestrationError::Runtime(
            "model profile has an unsupported authentication mode".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::model::{AgentKind, FileScope};
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeRuntime {
        specs: Mutex<Vec<RuntimeSpec>>,
        sinks: Mutex<HashMap<AgentId, RuntimeSink>>,
        stop_count: Arc<AtomicUsize>,
        fail: bool,
        exit_during_start: bool,
    }

    struct FakeHandle {
        stop_count: Arc<AtomicUsize>,
    }

    struct RecordingCompletionCapture {
        stop_count: Arc<AtomicUsize>,
        observations: Mutex<Vec<(AgentId, usize)>>,
    }

    impl CompletionCapture for RecordingCompletionCapture {
        fn enqueue(&self, id: AgentId) {
            self.observations
                .lock()
                .unwrap()
                .push((id, self.stop_count.load(Ordering::Relaxed)));
        }
    }

    impl RunningAgent for FakeHandle {
        fn stop(&self) {
            self.stop_count.fetch_add(1, Ordering::Relaxed);
        }

        fn pid(&self) -> u32 {
            4242
        }

        fn output_snapshot(&self) -> Vec<u8> {
            b"buffered output".to_vec()
        }
    }

    impl AgentRuntime for Arc<FakeRuntime> {
        fn start(
            &self,
            spec: RuntimeSpec,
            sink: RuntimeSink,
        ) -> Result<Arc<dyn RunningAgent>, String> {
            if self.fail {
                return Err("simulated runtime failure".to_string());
            }
            self.specs.lock().unwrap().push(spec.clone());
            self.sinks.lock().unwrap().insert(spec.agent_id, sink);
            if self.exit_during_start {
                self.sinks.lock().unwrap().get(&spec.agent_id).unwrap()(RuntimeEvent::Exit {
                    code: 0,
                });
            }
            Ok(Arc::new(FakeHandle {
                stop_count: Arc::clone(&self.stop_count),
            }))
        }
    }

    impl FakeRuntime {
        fn emit(&self, id: AgentId, event: RuntimeEvent) {
            self.sinks.lock().unwrap().get(&id).unwrap()(event);
        }
    }

    fn ready_agent(store: &OrchestrationStore) -> Agent {
        let workspace = store
            .get_or_create_workspace("repo", PathBuf::from("C:/repo").as_path(), 1)
            .unwrap();
        let session = store
            .create_session(workspace.id, "run", "abc123", 2)
            .unwrap();
        let agent = store
            .plan_agent(
                session.id,
                None,
                "writer",
                "build it",
                "codex --full-auto",
                "feat/writer",
                AgentKind::Writer,
                &[FileScope::new("shell/src")],
                3,
            )
            .unwrap();
        store
            .set_worktree_path(agent.id, PathBuf::from("C:/repo/writer").as_path())
            .unwrap();
        store.mark_ready(agent.id, 4).unwrap()
    }

    fn supervisor(
        store: Arc<OrchestrationStore>,
        runtime: Arc<FakeRuntime>,
        clock: Arc<AtomicI64>,
    ) -> RuntimeSupervisor<Arc<FakeRuntime>> {
        let clock_reader = Arc::clone(&clock);
        RuntimeSupervisor::with_clock(
            store,
            runtime,
            r"\\.\pipe\optimus-test".to_string(),
            Arc::new(|_, _| {}),
            Arc::new(move || clock_reader.load(Ordering::Relaxed)),
        )
    }

    #[test]
    fn start_binds_the_ready_agent_to_a_surface_and_tracks_activity_and_exit() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let agent = ready_agent(&store);
        let runtime = Arc::new(FakeRuntime::default());
        let clock = Arc::new(AtomicI64::new(10));
        let supervisor = supervisor(Arc::clone(&store), Arc::clone(&runtime), Arc::clone(&clock));

        let running = supervisor.start_agent(agent.id).unwrap();
        assert_eq!(AgentState::Running, running.state);
        assert_eq!(Some(1_000_001), running.surface_id);
        let spec = runtime.specs.lock().unwrap()[0].clone();
        assert_eq!("codex --full-auto", spec.command);
        assert_eq!("feat/writer", spec.branch);
        assert_eq!(
            b"buffered output",
            supervisor.output_snapshot(agent.id).as_slice()
        );

        clock.store(20, Ordering::Relaxed);
        runtime.emit(agent.id, RuntimeEvent::Output { byte_count: 12 });
        assert_eq!(
            Some(20),
            store.agent(agent.id).unwrap().unwrap().last_output_ts
        );
        clock.store(30, Ordering::Relaxed);
        runtime.emit(agent.id, RuntimeEvent::Exit { code: 0 });
        let done = store.agent(agent.id).unwrap().unwrap();
        assert_eq!(AgentState::Done, done.state);
        assert_eq!(Some(0), done.exit_code);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while supervisor.active_count() != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(0, supervisor.active_count());
    }

    #[test]
    fn stop_is_idempotent_and_tears_down_the_runtime_once() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let agent = ready_agent(&store);
        let runtime = Arc::new(FakeRuntime::default());
        let clock = Arc::new(AtomicI64::new(10));
        let supervisor = supervisor(Arc::clone(&store), Arc::clone(&runtime), clock);
        supervisor.start_agent(agent.id).unwrap();

        assert_eq!(
            AgentState::Interrupted,
            supervisor.stop_agent(agent.id).unwrap().state
        );
        assert_eq!(
            AgentState::Interrupted,
            supervisor.stop_agent(agent.id).unwrap().state
        );
        assert_eq!(1, runtime.stop_count.load(Ordering::Relaxed));
    }

    #[test]
    fn completion_capture_runs_only_after_explicit_stop_is_quiescent() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let agent = ready_agent(&store);
        let runtime = Arc::new(FakeRuntime::default());
        let capture = Arc::new(RecordingCompletionCapture {
            stop_count: Arc::clone(&runtime.stop_count),
            observations: Mutex::new(Vec::new()),
        });
        let capture_trait: Arc<dyn CompletionCapture> = capture.clone();
        let supervisor = supervisor(
            Arc::clone(&store),
            Arc::clone(&runtime),
            Arc::new(AtomicI64::new(10)),
        )
        .with_completion_capture(capture_trait);
        supervisor.start_agent(agent.id).unwrap();

        supervisor.stop_agent(agent.id).unwrap();
        assert_eq!(vec![(agent.id, 1)], *capture.observations.lock().unwrap());
        assert_eq!(0, supervisor.active_count());
    }

    #[test]
    fn natural_exit_removes_the_handle_before_completion_capture() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let agent = ready_agent(&store);
        let runtime = Arc::new(FakeRuntime::default());
        let capture = Arc::new(RecordingCompletionCapture {
            stop_count: Arc::clone(&runtime.stop_count),
            observations: Mutex::new(Vec::new()),
        });
        let capture_trait: Arc<dyn CompletionCapture> = capture.clone();
        let supervisor = supervisor(
            Arc::clone(&store),
            Arc::clone(&runtime),
            Arc::new(AtomicI64::new(10)),
        )
        .with_completion_capture(capture_trait);
        supervisor.start_agent(agent.id).unwrap();
        runtime.emit(agent.id, RuntimeEvent::Exit { code: 0 });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while capture.observations.lock().unwrap().is_empty()
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(vec![(agent.id, 0)], *capture.observations.lock().unwrap());
        assert_eq!(0, supervisor.active_count());
    }

    #[test]
    fn nonzero_process_exit_is_failed_with_the_real_code() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let agent = ready_agent(&store);
        let runtime = Arc::new(FakeRuntime::default());
        let supervisor = supervisor(
            Arc::clone(&store),
            Arc::clone(&runtime),
            Arc::new(AtomicI64::new(10)),
        );
        supervisor.start_agent(agent.id).unwrap();

        runtime.emit(agent.id, RuntimeEvent::Exit { code: 17 });
        let failed = store.agent(agent.id).unwrap().unwrap();
        assert_eq!(AgentState::Failed, failed.state);
        assert_eq!(Some(17), failed.exit_code);
        assert!(failed
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("17"));
    }

    #[test]
    fn runtime_start_failure_becomes_a_durable_failed_agent() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let agent = ready_agent(&store);
        let runtime = Arc::new(FakeRuntime {
            fail: true,
            ..FakeRuntime::default()
        });
        let supervisor = supervisor(Arc::clone(&store), runtime, Arc::new(AtomicI64::new(10)));
        assert!(supervisor.start_agent(agent.id).is_err());
        let failed = store.agent(agent.id).unwrap().unwrap();
        assert_eq!(AgentState::Failed, failed.state);
        assert!(failed
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("simulated"));
    }

    #[test]
    fn subscription_profile_clears_all_supported_api_credentials() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let agent = ready_agent(&store);
        store
            .set_agent_model_auth(agent.id, "subscription", None)
            .unwrap();
        let runtime = Arc::new(FakeRuntime::default());
        let supervisor = supervisor(
            Arc::clone(&store),
            Arc::clone(&runtime),
            Arc::new(AtomicI64::new(10)),
        );
        supervisor.start_agent(agent.id).unwrap();
        let environment = &runtime.specs.lock().unwrap()[0].environment_overrides;
        assert_eq!(MODEL_API_CREDENTIALS.len() + 1, environment.len());
        assert!(environment
            .iter()
            .filter(|(name, _)| name != FILE_SCOPE_ENV)
            .all(|(_, value)| value.is_empty()));
        assert!(environment
            .iter()
            .any(|(name, value)| name == FILE_SCOPE_ENV && value.contains("shell/src")));
    }

    #[test]
    fn api_profile_injects_only_its_selected_credential_and_redacts_debug() {
        let secret = "TOP_SECRET_API_CREDENTIAL";
        let environment = model_environment(
            Some(AgentModelAuth {
                auth_mode: "api".to_string(),
                credential_env: Some("OPENAI_API_KEY".to_string()),
            }),
            |name| (name == "OPENAI_API_KEY").then(|| secret.to_string()),
        )
        .unwrap();
        assert_eq!(
            Some(secret),
            environment
                .iter()
                .find(|(name, _)| name == "OPENAI_API_KEY")
                .map(|(_, value)| value.as_str())
        );
        assert!(environment
            .iter()
            .filter(|(name, _)| name != "OPENAI_API_KEY")
            .all(|(_, value)| value.is_empty()));

        let mut spec = RuntimeSpec {
            agent_id: AgentId(1),
            surface_id: SurfaceId(1_000_001),
            session_id: "session-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            branch: "feat/test".to_string(),
            command: "codex".to_string(),
            cwd: PathBuf::from("C:/repo"),
            pipe_name: "optimus-test".to_string(),
            caller_capability: secret.to_string(),
            environment_overrides: environment,
        };
        assert!(!format!("{spec:?}").contains(secret));
        spec.environment_overrides.clear();
    }

    #[test]
    fn api_profile_without_its_desktop_credential_fails_before_spawn() {
        let result = model_environment(
            Some(AgentModelAuth {
                auth_mode: "api".to_string(),
                credential_env: Some("OPENAI_API_KEY".to_string()),
            }),
            |_| None,
        );
        assert!(result.unwrap_err().to_string().contains("OPENAI_API_KEY"));
    }

    #[test]
    fn exit_during_runtime_start_never_retains_a_terminal_handle() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let agent = ready_agent(&store);
        let runtime = Arc::new(FakeRuntime {
            exit_during_start: true,
            ..FakeRuntime::default()
        });
        let supervisor = supervisor(
            Arc::clone(&store),
            Arc::clone(&runtime),
            Arc::new(AtomicI64::new(10)),
        );

        let result = supervisor.start_agent(agent.id).unwrap();
        assert_eq!(AgentState::Done, result.state);
        assert_eq!(Some(0), result.exit_code);
        assert_eq!(0, supervisor.active_count());
        assert_eq!(1, runtime.stop_count.load(Ordering::Relaxed));
    }

    struct BlockingRuntime {
        entered: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
        stop_count: Arc<AtomicUsize>,
    }

    impl AgentRuntime for Arc<BlockingRuntime> {
        fn start(
            &self,
            _spec: RuntimeSpec,
            _sink: RuntimeSink,
        ) -> Result<Arc<dyn RunningAgent>, String> {
            self.entered.wait();
            self.release.wait();
            Ok(Arc::new(FakeHandle {
                stop_count: Arc::clone(&self.stop_count),
            }))
        }
    }

    #[test]
    fn stop_waits_until_an_inflight_start_owns_its_handle() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let agent = ready_agent(&store);
        let runtime = Arc::new(BlockingRuntime {
            entered: Arc::new(std::sync::Barrier::new(2)),
            release: Arc::new(std::sync::Barrier::new(2)),
            stop_count: Arc::new(AtomicUsize::new(0)),
        });
        let supervisor = Arc::new(RuntimeSupervisor::with_clock(
            Arc::clone(&store),
            Arc::clone(&runtime),
            r"\\.\pipe\optimus-test".to_string(),
            Arc::new(|_, _| {}),
            Arc::new(|| 10),
        ));

        let starter = {
            let supervisor = Arc::clone(&supervisor);
            std::thread::spawn(move || supervisor.start_agent(agent.id))
        };
        runtime.entered.wait();

        let (sender, receiver) = std::sync::mpsc::channel();
        let stopper = {
            let supervisor = Arc::clone(&supervisor);
            std::thread::spawn(move || {
                sender.send(supervisor.stop_agent(agent.id)).unwrap();
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(25));
        assert!(matches!(
            receiver.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        runtime.release.wait();
        assert_eq!(AgentState::Running, starter.join().unwrap().unwrap().state);
        assert_eq!(
            AgentState::Interrupted,
            receiver.recv().unwrap().unwrap().state
        );
        stopper.join().unwrap();
        assert_eq!(0, supervisor.active_count());
        assert_eq!(1, runtime.stop_count.load(Ordering::Relaxed));
    }

    #[cfg(windows)]
    #[test]
    fn real_short_lived_engine_exit_is_reaped_off_the_engine_thread() {
        let root = std::env::temp_dir().join(format!(
            "optimus-runtime-exit-{}-{}",
            std::process::id(),
            crate::control_plane::memory::test_timestamp()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let workspace = store
            .get_or_create_workspace("runtime-exit", &root, 1)
            .unwrap();
        let session = store
            .create_session(workspace.id, "run", "abc123", 2)
            .unwrap();
        let planned = store
            .plan_agent(
                session.id,
                None,
                "short lived",
                "exit immediately",
                "cmd.exe /d /c exit 0",
                "test/short-lived",
                AgentKind::Writer,
                &[FileScope::new("shell")],
                3,
            )
            .unwrap();
        store.set_worktree_path(planned.id, &root).unwrap();
        store.mark_ready(planned.id, 4).unwrap();
        let supervisor = RuntimeSupervisor::new(
            Arc::clone(&store),
            EngineRuntime::new(0),
            r"\\.\pipe\optimus-test".to_string(),
            None,
            Arc::new(|_, _| {}),
        );

        let _ = supervisor.start_agent(planned.id).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let agent = store.agent(planned.id).unwrap().unwrap();
            if agent.state == AgentState::Done && supervisor.active_count() == 0 {
                assert_eq!(Some(0), agent.exit_code);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "short-lived engine was not reaped: state={:?}, handles={}",
                agent.state,
                supervisor.active_count()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
