use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::control_plane::memory::{MemoryStore, RecordKind, TrustedRecord};
use crate::orchestration::git::{GitChange, GitWorktrees};
use crate::orchestration::model::{Agent, AgentId};
use crate::orchestration::store::{OrchestrationError, OrchestrationStore};

const CAPTURE_BATCH: usize = 100;
const MAX_CAPTURE_ERROR_CHARS: usize = 1_024;

pub trait CompletionCapture: Send + Sync {
    fn enqueue(&self, id: AgentId);
}

pub struct GitCompletionCapture<G: GitWorktrees> {
    store: Arc<OrchestrationStore>,
    memory: Arc<MemoryStore>,
    git: Arc<G>,
    worker: Arc<Mutex<()>>,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl<G: GitWorktrees> Clone for GitCompletionCapture<G> {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            memory: Arc::clone(&self.memory),
            git: Arc::clone(&self.git),
            worker: Arc::clone(&self.worker),
            now_ms: Arc::clone(&self.now_ms),
        }
    }
}

impl<G: GitWorktrees + 'static> GitCompletionCapture<G> {
    pub fn new(store: Arc<OrchestrationStore>, memory: Arc<MemoryStore>, git: G) -> Self {
        Self {
            store,
            memory,
            git: Arc::new(git),
            worker: Arc::new(Mutex::new(())),
            now_ms: Arc::new(now_ms),
        }
    }

    pub fn retry_pending(&self) -> Result<(), OrchestrationError> {
        self.store.retry_failed_git_captures((self.now_ms)())?;
        self.store
            .enqueue_missing_terminal_git_captures((self.now_ms)())?;
        self.spawn_worker();
        Ok(())
    }

    fn queue(&self, id: AgentId) -> Result<(), OrchestrationError> {
        self.store.enqueue_git_capture(id, (self.now_ms)())?;
        self.spawn_worker();
        Ok(())
    }

    fn spawn_worker(&self) {
        let capture = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("optimus-git-capture".to_string())
            .spawn(move || capture.process_pending_once())
        {
            eprintln!("failed to start Git capture worker: {error}");
        }
    }

    fn process_pending_once(&self) {
        let _worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let pending = match self.store.pending_git_captures(CAPTURE_BATCH) {
                Ok(pending) => pending,
                Err(error) => {
                    eprintln!("failed to load Git capture jobs: {error}");
                    return;
                }
            };
            if pending.is_empty() {
                return;
            }
            for id in pending {
                if let Err(error) = self.capture_agent(id) {
                    let message = error
                        .to_string()
                        .chars()
                        .take(MAX_CAPTURE_ERROR_CHARS)
                        .collect::<String>();
                    if let Err(store_error) =
                        self.store
                            .mark_git_capture_failed(id, &message, (self.now_ms)())
                    {
                        eprintln!("failed to persist Git capture error for {id}: {store_error}");
                        return;
                    }
                }
            }
        }
    }

    fn capture_agent(&self, id: AgentId) -> Result<(), OrchestrationError> {
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| OrchestrationError::Invariant(format!("unknown capture agent {id}")))?;
        if !agent.state.is_terminal() {
            return Err(OrchestrationError::Invariant(format!(
                "capture agent {id} is not terminal"
            )));
        }
        let worktree = agent.worktree_path.as_deref().ok_or_else(|| {
            OrchestrationError::Invariant(format!("capture agent {id} has no worktree"))
        })?;
        let changes = self.git.changes_since(worktree, &agent.base_commit)?;
        for change in changes {
            self.append_change(&agent, change)?;
        }
        self.store.mark_git_capture_succeeded(id, (self.now_ms)())
    }

    fn append_change(&self, agent: &Agent, change: GitChange) -> Result<(), OrchestrationError> {
        let source_key = capture_source_key(agent, &change.file_key);
        let action = if change.untracked {
            "Created"
        } else if change.lines_added == Some(0) && change.lines_deleted.unwrap_or(0) > 0 {
            "Deleted"
        } else {
            "Changed"
        };
        let counts = if change.binary {
            "binary".to_string()
        } else {
            format!(
                "+{} -{}",
                change.lines_added.unwrap_or(0),
                change.lines_deleted.unwrap_or(0)
            )
        };
        self.memory.append_trusted_once(
            &source_key,
            TrustedRecord {
                fact: format!("{action} {} ({counts})", change.file_key),
                why: Some("Captured after the agent process stopped".to_string()),
                agent_id: agent.agent_key.clone(),
                kind: RecordKind::Change,
                file_key: Some(change.file_key),
                branch: Some(agent.branch.clone()),
                workspace_id: agent.workspace_id.0,
                reverses: None,
                parent_record: None,
                symbols: Vec::new(),
                ts: agent.ended_ts,
            },
        )?;
        Ok(())
    }
}

impl<G: GitWorktrees + 'static> CompletionCapture for GitCompletionCapture<G> {
    fn enqueue(&self, id: AgentId) {
        if let Err(error) = self.queue(id) {
            eprintln!("failed to queue Git capture for {id}: {error}");
        }
    }
}

fn capture_source_key(agent: &Agent, file_key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(agent.id.0.to_le_bytes());
    hash.update(agent.base_commit.as_bytes());
    hash.update([0]);
    hash.update(file_key.as_bytes());
    format!("git:{}:{:x}", agent.id.0, hash.finalize())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::control_plane::memory::{AgentIdentityResolver, TrustLevel};
    use crate::domain::ids::SurfaceId;
    use crate::orchestration::git::GitError;
    use crate::orchestration::model::{AgentKind, AgentState, FileScope};

    struct FakeGit {
        failures_remaining: AtomicUsize,
        changes: Vec<GitChange>,
    }

    impl GitWorktrees for FakeGit {
        fn resolve_main(&self, _repo_root: &Path) -> Result<String, GitError> {
            Ok("abc123".to_string())
        }

        fn create_worktree(
            &self,
            _repo_root: &Path,
            _path: &Path,
            _branch: &str,
            _base_commit: &str,
        ) -> Result<(), GitError> {
            Ok(())
        }

        fn changes_since(
            &self,
            _worktree: &Path,
            _base_commit: &str,
        ) -> Result<Vec<GitChange>, GitError> {
            if self
                .failures_remaining
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                    (remaining > 0).then(|| remaining.saturating_sub(1))
                })
                .is_ok()
            {
                return Err(GitError::Command("simulated capture failure".to_string()));
            }
            Ok(self.changes.clone())
        }
    }

    fn terminal_agent(store: &OrchestrationStore, name: &str, ts: i64) -> Agent {
        let workspace = store
            .get_or_create_workspace("repo", Path::new("C:/repo"), ts)
            .unwrap();
        let session = store
            .create_session(workspace.id, name, "abc123", ts + 1)
            .unwrap();
        let agent = store
            .plan_agent(
                session.id,
                None,
                name,
                "build it",
                "codex",
                &format!("feat/{name}"),
                AgentKind::Writer,
                &[FileScope::new("shell/src")],
                ts + 2,
            )
            .unwrap();
        store
            .set_worktree_path(agent.id, &PathBuf::from(format!("C:/repo/{name}")))
            .unwrap();
        store.mark_ready(agent.id, ts + 3).unwrap();
        store
            .mark_running(agent.id, SurfaceId(1_000_000 + agent.id.0 as i32), ts + 4)
            .unwrap();
        store.mark_exited(agent.id, 0, ts + 5).unwrap()
    }

    fn fixture(
        fail: bool,
    ) -> (
        Arc<OrchestrationStore>,
        Arc<MemoryStore>,
        GitCompletionCapture<FakeGit>,
        Agent,
    ) {
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let mut agent = terminal_agent(&store, "writer", 1);
        let resolver: Arc<dyn AgentIdentityResolver> = store.clone();
        let memory = Arc::new(MemoryStore::in_memory(resolver).unwrap());
        let spawn = memory
            .append_trusted(TrustedRecord {
                fact: "Spawned writer".to_string(),
                why: None,
                agent_id: agent.agent_key.clone(),
                kind: RecordKind::Spawn,
                file_key: None,
                branch: Some(agent.branch.clone()),
                workspace_id: agent.workspace_id.0,
                reverses: None,
                parent_record: None,
                symbols: Vec::new(),
                ts: Some(1),
            })
            .unwrap();
        store.set_spawn_record(agent.id, spawn.id).unwrap();
        agent = store.agent(agent.id).unwrap().unwrap();
        let capture = GitCompletionCapture::new(
            store.clone(),
            memory.clone(),
            FakeGit {
                failures_remaining: AtomicUsize::new(if fail { usize::MAX } else { 0 }),
                changes: vec![
                    GitChange {
                        file_key: "shell/src/main.rs".to_string(),
                        lines_added: Some(4),
                        lines_deleted: Some(2),
                        binary: false,
                        untracked: false,
                    },
                    GitChange {
                        file_key: "ui/new.js".to_string(),
                        lines_added: Some(8),
                        lines_deleted: Some(0),
                        binary: false,
                        untracked: true,
                    },
                ],
            },
        );
        (store, memory, capture, agent)
    }

    #[test]
    fn repeated_capture_is_idempotent_and_records_no_file_contents() {
        let (store, memory, capture, agent) = fixture(false);
        store.enqueue_git_capture(agent.id, 10).unwrap();
        capture.process_pending_once();
        store.enqueue_git_capture(agent.id, 11).unwrap();
        capture.process_pending_once();

        let records = memory.records(agent.workspace_id.0, 10).unwrap();
        let changes = records
            .iter()
            .filter(|record| record.kind == RecordKind::Change)
            .collect::<Vec<_>>();
        assert_eq!(2, changes.len());
        assert!(changes.iter().all(|record| {
            record.kind == RecordKind::Change && record.trust == TrustLevel::Trusted
        }));
        assert!(changes
            .iter()
            .all(|record| !record.fact.contains("source code")));
        assert_eq!(
            "succeeded",
            store.git_capture_job(agent.id).unwrap().unwrap().state
        );
    }

    #[test]
    fn failed_capture_preserves_terminal_state_and_retries_once() {
        let (store, memory, capture, agent) = fixture(false);
        capture.git.failures_remaining.store(1, Ordering::Relaxed);
        store.enqueue_git_capture(agent.id, 10).unwrap();
        capture.process_pending_once();
        let completed = store.git_capture_job(agent.id).unwrap().unwrap();
        assert_eq!("succeeded", completed.state);
        assert_eq!(1, completed.attempts);
        assert!(completed.last_error.is_none());
        assert_eq!(
            AgentState::Done,
            store.agent(agent.id).unwrap().unwrap().state
        );
        assert_eq!(
            2,
            memory
                .records(agent.workspace_id.0, 10)
                .unwrap()
                .iter()
                .filter(|record| record.kind == RecordKind::Change)
                .count()
        );
        assert_eq!(
            "succeeded",
            store.git_capture_job(agent.id).unwrap().unwrap().state
        );
    }

    #[test]
    fn startup_discovery_queues_terminal_agents_without_jobs() {
        let (store, _memory, capture, agent) = fixture(false);
        assert!(store.git_capture_job(agent.id).unwrap().is_none());
        store.enqueue_missing_terminal_git_captures(10).unwrap();
        capture.process_pending_once();
        assert_eq!(
            "succeeded",
            store.git_capture_job(agent.id).unwrap().unwrap().state
        );
    }

    #[test]
    fn worker_drains_more_than_one_batch_and_caps_failed_retries() {
        let (store, _memory, capture, first) = fixture(false);
        store.enqueue_git_capture(first.id, 10).unwrap();
        let mut agents = vec![first];
        for index in 0..CAPTURE_BATCH + 5 {
            let agent = terminal_agent(&store, &format!("writer-{index}"), 100 + index as i64 * 10);
            store
                .enqueue_git_capture(agent.id, 1_000 + index as i64)
                .unwrap();
            agents.push(agent);
        }
        capture.process_pending_once();
        assert!(agents.iter().all(|agent| {
            store
                .git_capture_job(agent.id)
                .unwrap()
                .is_some_and(|job| job.state == "succeeded")
        }));

        let failing = terminal_agent(&store, "always-fails", 10_000);
        store.enqueue_git_capture(failing.id, 11_000).unwrap();
        capture
            .git
            .failures_remaining
            .store(usize::MAX, Ordering::Relaxed);
        capture.process_pending_once();
        let job = store.git_capture_job(failing.id).unwrap().unwrap();
        assert_eq!(2, job.attempts);
        assert_eq!("failed", job.state);
    }
}
