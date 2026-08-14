use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::model::RelayConfig;
use super::{remote_snapshot, RelayCommandExecutor, RelayError, RelayTransport, SnapshotSource};
use crate::orchestration::store::OrchestrationStore;

const POLL_INTERVAL: Duration = Duration::from_millis(750);
const PRESENCE_INTERVAL: Duration = Duration::from_secs(15);
const MAX_BACKOFF: Duration = Duration::from_secs(8);
const STOP_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

pub struct RelayBridge {
    stop: Arc<(Mutex<bool>, Condvar)>,
    worker: Mutex<Option<JoinHandle<()>>>,
    worker_done: Mutex<Option<Receiver<()>>>,
}

impl RelayBridge {
    pub fn disabled() -> Self {
        Self {
            stop: Arc::new((Mutex::new(true), Condvar::new())),
            worker: Mutex::new(None),
            worker_done: Mutex::new(None),
        }
    }

    pub fn start(
        config: RelayConfig,
        transport: Arc<dyn RelayTransport>,
        source: Arc<dyn SnapshotSource>,
        store: Arc<OrchestrationStore>,
        executor: Arc<dyn RelayCommandExecutor>,
    ) -> Result<Self, RelayError> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_stop = Arc::clone(&stop);
        let (done_sender, done_receiver) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("optimus-relay".to_string())
            .spawn(move || {
                run_loop(config, transport, source, store, executor, worker_stop);
                let _ = done_sender.send(());
            })
            .map_err(|error| RelayError::Config(format!("failed to start worker: {error}")))?;
        Ok(Self {
            stop,
            worker: Mutex::new(Some(worker)),
            worker_done: Mutex::new(Some(done_receiver)),
        })
    }

    pub fn stop(&self) {
        let (closed, wake) = &*self.stop;
        *closed.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let done = self
            .worker_done
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let (Some(worker), Some(done)) = (worker, done) {
            if done.recv_timeout(STOP_JOIN_TIMEOUT).is_ok() {
                let _ = worker.join();
            } else {
                eprintln!("Optimus relay worker did not stop within 500 ms; detaching shutdown");
            }
        }
    }
}

impl Drop for RelayBridge {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_loop(
    config: RelayConfig,
    transport: Arc<dyn RelayTransport>,
    source: Arc<dyn SnapshotSource>,
    store: Arc<OrchestrationStore>,
    executor: Arc<dyn RelayCommandExecutor>,
    stop: Arc<(Mutex<bool>, Condvar)>,
) {
    let mut last_revision = None;
    let mut last_presence = Instant::now() - PRESENCE_INTERVAL;
    let mut backoff = POLL_INTERVAL;
    loop {
        if is_stopped(&stop) {
            break;
        }
        let mut transport_failed = false;
        match source.snapshot().and_then(|snapshot| {
            let revision = snapshot.revision;
            let envelope = remote_snapshot(&snapshot, &store, &config.instance_id, unix_ms())?;
            Ok((revision, envelope))
        }) {
            Ok((revision, envelope))
                if last_revision != Some(revision)
                    || last_presence.elapsed() >= PRESENCE_INTERVAL =>
            {
                match transport.publish(&envelope) {
                    Ok(()) => {
                        last_revision = Some(revision);
                        last_presence = Instant::now();
                    }
                    Err(error) => {
                        eprintln!("Optimus relay publish deferred: {error}");
                        transport_failed = true;
                    }
                }
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("Optimus relay snapshot deferred: {error}");
            }
        }

        match transport.poll() {
            Ok(commands) => {
                for command in commands {
                    if transport.acknowledge(&command, "running", None).is_err() {
                        transport_failed = true;
                        continue;
                    }
                    let result = executor.execute(&command, unix_ms());
                    if transport
                        .acknowledge(&command, result.status, result.error_code.as_deref())
                        .is_err()
                    {
                        transport_failed = true;
                    }
                }
            }
            Err(error) => {
                eprintln!("Optimus relay command poll deferred: {error}");
                transport_failed = true;
            }
        }

        backoff = if transport_failed {
            (backoff * 2).min(MAX_BACKOFF)
        } else {
            POLL_INTERVAL
        };
        wait_or_stop(&stop, backoff);
    }
}

fn is_stopped(stop: &Arc<(Mutex<bool>, Condvar)>) -> bool {
    *stop.0.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait_or_stop(stop: &Arc<(Mutex<bool>, Condvar)>, duration: Duration) {
    let (closed, wake) = &**stop;
    let guard = closed.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = wake
        .wait_timeout_while(guard, duration, |value| !*value)
        .unwrap_or_else(PoisonError::into_inner);
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::read_model::{CapacityNode, ControlPlaneSnapshot, TotalsNode};
    use crate::orchestration::store::OrchestrationStore;
    use crate::relay::model::{
        RelayCommand, RelayCommandKind, RelayConfig, RelayProviderConfig, RelayWorkspaceConfig,
        SnapshotEnvelope,
    };
    use crate::relay::RelayExecutionResult;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeSource;

    impl SnapshotSource for FakeSource {
        fn snapshot(&self) -> Result<ControlPlaneSnapshot, RelayError> {
            Ok(ControlPlaneSnapshot {
                revision: 1,
                generated_at: 1,
                workspaces: Vec::new(),
                sessions: Vec::new(),
                agents: Vec::new(),
                activity: Vec::new(),
                contradictions: Vec::new(),
                capacity: CapacityNode {
                    used: 0,
                    reserved: 0,
                    max: 8,
                    level: "calm",
                    fraction: 0.0,
                    at_cap: false,
                },
                totals: TotalsNode {
                    running: 0,
                    stalled: 0,
                    waiting: 0,
                    done: 0,
                    failed: 0,
                },
            })
        }
    }

    struct FakeTransport {
        published: AtomicUsize,
        commands: Mutex<Vec<RelayCommand>>,
        acknowledgements: Mutex<Vec<String>>,
    }

    impl RelayTransport for FakeTransport {
        fn publish(&self, _snapshot: &SnapshotEnvelope) -> Result<(), RelayError> {
            self.published.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn poll(&self) -> Result<Vec<RelayCommand>, RelayError> {
            Ok(self
                .commands
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain(..)
                .collect())
        }

        fn acknowledge(
            &self,
            _command: &RelayCommand,
            status: &str,
            _error_code: Option<&str>,
        ) -> Result<(), RelayError> {
            self.acknowledgements
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(status.to_string());
            Ok(())
        }
    }

    struct FakeExecutor(AtomicUsize);

    impl RelayCommandExecutor for FakeExecutor {
        fn execute(&self, _command: &RelayCommand, _now: i64) -> RelayExecutionResult {
            self.0.fetch_add(1, Ordering::SeqCst);
            RelayExecutionResult {
                status: "succeeded",
                error_code: None,
            }
        }
    }

    #[test]
    fn worker_publishes_polls_acknowledges_and_stops_quiescently() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let workspace = store
            .get_or_create_workspace("repo", Path::new("C:/relay-test"), 1)
            .unwrap();
        store
            .register_relay_workspace("workspace-primary", workspace.id, "Primary")
            .unwrap();
        store
            .register_relay_provider(
                "codex-subscription",
                "Codex subscription",
                "codex",
                "prompt_arg",
                "subscription",
                None,
            )
            .unwrap();
        let transport = Arc::new(FakeTransport {
            published: AtomicUsize::new(0),
            commands: Mutex::new(vec![RelayCommand {
                command_id: "12345678-90ab-4000-8000-000000000000".to_string(),
                device_id: "primary".to_string(),
                kind: RelayCommandKind::StopAgent,
                payload: serde_json::json!({ "agentId": "agent-1" }),
                status: "leased".to_string(),
                issued_at: 1,
                expires_at: i64::MAX,
                lease_id: "11111111-1111-4111-8111-111111111111".to_string(),
                lease_owner: "22222222-2222-4222-8222-222222222222".to_string(),
                lease_until: i64::MAX,
                error_code: None,
                updated_at: 1,
            }]),
            acknowledgements: Mutex::new(Vec::new()),
        });
        let executor = Arc::new(FakeExecutor(AtomicUsize::new(0)));
        let config = RelayConfig {
            base_url: "http://127.0.0.1:4174".to_string(),
            desktop_token: "desktop-token-at-least-24-characters".to_string(),
            device_id: "primary".to_string(),
            instance_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            consumer_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".to_string(),
            workspaces: vec![RelayWorkspaceConfig {
                id: "workspace-primary".to_string(),
                label: "Primary".to_string(),
                repo_root: "C:/relay-test".to_string(),
            }],
            providers: vec![RelayProviderConfig {
                id: "codex-subscription".to_string(),
                label: "Codex subscription".to_string(),
                command: "codex".to_string(),
                auth_mode: "subscription".to_string(),
                credential_env: None,
            }],
        };
        let bridge = RelayBridge::start(
            config,
            transport.clone(),
            Arc::new(FakeSource),
            store,
            executor.clone(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while transport
            .acknowledgements
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
            < 2
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        bridge.stop();
        assert!(transport.published.load(Ordering::SeqCst) >= 1);
        assert_eq!(1, executor.0.load(Ordering::SeqCst));
        assert_eq!(
            vec!["running".to_string(), "succeeded".to_string()],
            *transport
                .acknowledgements
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
        );
    }

    struct SlowTransport {
        entered: Arc<std::sync::Barrier>,
    }

    impl RelayTransport for SlowTransport {
        fn publish(&self, _snapshot: &SnapshotEnvelope) -> Result<(), RelayError> {
            Ok(())
        }

        fn poll(&self) -> Result<Vec<RelayCommand>, RelayError> {
            self.entered.wait();
            std::thread::sleep(Duration::from_secs(2));
            Ok(Vec::new())
        }

        fn acknowledge(
            &self,
            _command: &RelayCommand,
            _status: &str,
            _error_code: Option<&str>,
        ) -> Result<(), RelayError> {
            Ok(())
        }
    }

    #[test]
    fn stop_is_bounded_when_transport_work_cannot_be_cancelled() {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let entered = Arc::new(std::sync::Barrier::new(2));
        let bridge = RelayBridge::start(
            RelayConfig {
                base_url: "http://127.0.0.1:4174".to_string(),
                desktop_token: "desktop-token-at-least-24-characters".to_string(),
                device_id: "primary".to_string(),
                instance_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
                consumer_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".to_string(),
                workspaces: Vec::new(),
                providers: Vec::new(),
            },
            Arc::new(SlowTransport {
                entered: Arc::clone(&entered),
            }),
            Arc::new(FakeSource),
            store,
            Arc::new(FakeExecutor(AtomicUsize::new(0))),
        )
        .unwrap();
        entered.wait();
        let started = Instant::now();
        bridge.stop();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "relay stop took {:?}",
            started.elapsed()
        );
    }
}
