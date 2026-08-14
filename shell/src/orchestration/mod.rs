//! Desktop-owned agent session and Git worktree orchestration.

pub mod capture;
pub mod git;
pub mod model;
pub mod runtime;
pub mod store;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::control_plane::memory::{MemoryStore, RecordKind, TrustedRecord};
use git::GitWorktrees;
use model::{Agent, AgentId, AgentKind, FileScope, Session, SessionId};
use store::{OrchestrationError, OrchestrationStore};

#[derive(Clone, Debug)]
pub struct PrepareAgent {
    pub parent: Option<AgentId>,
    pub name: String,
    pub task: String,
    pub command: String,
    pub branch: String,
    pub kind: AgentKind,
    pub file_scope: Vec<FileScope>,
}

/// Coordinates durable intent with Git side effects. The database records a planned agent before
/// `git worktree add`; a failed Git command transitions that row to `failed`, so the dashboard
/// never loses the reason a requested spawn did not appear.
pub struct SessionOrchestrator<G: GitWorktrees> {
    store: Arc<OrchestrationStore>,
    memory: Option<Arc<MemoryStore>>,
    git: G,
}

impl<G: GitWorktrees> SessionOrchestrator<G> {
    pub fn new(store: Arc<OrchestrationStore>, memory: Option<Arc<MemoryStore>>, git: G) -> Self {
        Self { store, memory, git }
    }

    pub fn start_session(
        &self,
        repo_root: &Path,
        name: &str,
        ts: i64,
    ) -> Result<Session, OrchestrationError> {
        let repo_root = repo_root.canonicalize().map_err(OrchestrationError::Io)?;
        let base_commit = self.git.resolve_main(&repo_root)?;
        let workspace_name = repo_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace");
        let workspace = self
            .store
            .get_or_create_workspace(workspace_name, &repo_root, ts)?;
        self.store
            .create_session(workspace.id, name, &base_commit, ts)
    }

    pub fn prepare_agent(
        &self,
        session_id: SessionId,
        request: PrepareAgent,
        ts: i64,
    ) -> Result<Agent, OrchestrationError> {
        let session = self.store.session(session_id)?.ok_or_else(|| {
            OrchestrationError::Invariant(format!("unknown session {session_id}"))
        })?;
        if request
            .file_scope
            .iter()
            .any(|scope| !valid_file_scope(&scope.pattern))
        {
            return Err(OrchestrationError::Invariant(
                "agent file scope is not repository-relative".to_string(),
            ));
        }
        for sibling in self.store.agents_for_session(session_id)? {
            if !sibling.state.is_terminal()
                && file_scope_sets_overlap(&request.file_scope, &sibling.file_scope)
            {
                return Err(OrchestrationError::Invariant(format!(
                    "agent file scope overlaps {}",
                    sibling.agent_key
                )));
            }
        }
        let mut parent_spawn_record = None;
        if let Some(parent) = request.parent {
            let parent_agent = self
                .store
                .agent(parent)?
                .ok_or_else(|| OrchestrationError::Invariant(format!("unknown parent {parent}")))?;
            if parent_agent.session_id != session_id {
                return Err(OrchestrationError::Invariant(
                    "parent agent belongs to another session".to_string(),
                ));
            }
            parent_spawn_record = parent_agent.spawn_record_id;
        }

        let planned = self.store.plan_agent(
            session_id,
            request.parent,
            &request.name,
            &request.task,
            &request.command,
            &request.branch,
            request.kind,
            &request.file_scope,
            ts,
        )?;
        let path = worktree_path(&session.repo_root, session_id, planned.id);
        self.store.set_worktree_path(planned.id, &path)?;

        match self.git.create_worktree(
            &session.repo_root,
            &path,
            &planned.branch,
            &session.base_commit,
        ) {
            Ok(()) => {
                let ready = self.store.mark_ready(planned.id, ts)?;
                if let Some(memory) = &self.memory {
                    let fact = format!(
                        "Spawned {} in {} from pinned base {}",
                        ready.agent_key,
                        ready.branch,
                        short_commit(&ready.base_commit)
                    );
                    if let Ok(stored) = memory.append_trusted(TrustedRecord {
                        fact,
                        why: Some(ready.task.clone()),
                        agent_id: ready.agent_key.clone(),
                        kind: RecordKind::Spawn,
                        file_key: None,
                        branch: Some(ready.branch.clone()),
                        workspace_id: session.workspace_id.0,
                        reverses: None,
                        parent_record: parent_spawn_record,
                        symbols: Vec::new(),
                        ts: Some(ts),
                    }) {
                        let _ = self.store.set_spawn_record(ready.id, stored.id);
                    }
                }
                Ok(ready)
            }
            Err(error) => {
                self.store.mark_failed(planned.id, &error.to_string(), ts)?;
                if let Err(cleanup_error) =
                    self.git.cleanup_failed_worktree(&session.repo_root, &path)
                {
                    return Err(OrchestrationError::Invariant(format!(
                        "worktree preparation failed: {error}; cleanup failed: {cleanup_error}"
                    )));
                }
                self.store.clear_worktree_path(planned.id)?;
                Err(error.into())
            }
        }
    }

    pub fn rollback_agent(
        &self,
        id: AgentId,
        reason: &str,
        ts: i64,
    ) -> Result<(), OrchestrationError> {
        let agent = self
            .store
            .agent(id)?
            .ok_or_else(|| OrchestrationError::Invariant(format!("unknown agent {id}")))?;
        if !agent.state.is_terminal() {
            self.store.mark_failed(id, reason, ts)?;
        }
        if let Some(path) = agent.worktree_path.as_deref() {
            let session = self.store.session(agent.session_id)?.ok_or_else(|| {
                OrchestrationError::Invariant(format!("unknown session {}", agent.session_id))
            })?;
            self.git
                .remove_worktree(&session.repo_root, path, &agent.branch)?;
            self.store.clear_worktree_path(id)?;
        }
        self.store.cancel_git_capture(id)?;
        Ok(())
    }

    pub fn store(&self) -> &Arc<OrchestrationStore> {
        &self.store
    }
}

fn worktree_path(repo_root: &Path, session: SessionId, agent: AgentId) -> PathBuf {
    repo_root
        .join(".worktrees")
        .join("optimus")
        .join(session.to_string())
        .join(agent.to_string())
}

fn short_commit(commit: &str) -> &str {
    commit.get(..commit.len().min(12)).unwrap_or(commit)
}

pub(crate) fn valid_file_scope(value: &str) -> bool {
    canonical_file_scope(value).is_some()
}

pub(crate) fn canonical_file_scope(value: &str) -> Option<String> {
    let normalized = value.trim().replace('\\', "/");
    if normalized.is_empty()
        || normalized.len() > 160
        || normalized.starts_with('/')
        || normalized
            .as_bytes()
            .get(1)
            .is_some_and(|value| *value == b':')
        || normalized
            .split('/')
            .any(|part| part == ".." || (part != "." && part.ends_with(['.', ' '])))
        || normalized.contains(['\r', '\n', '\0'])
    {
        return None;
    }
    let canonical = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/");
    (!canonical.is_empty()).then(|| canonical.to_ascii_lowercase())
}

pub(crate) fn file_scope_sets_overlap(left: &[FileScope], right: &[FileScope]) -> bool {
    if left.is_empty() || right.is_empty() {
        return true;
    }
    left.iter().any(|left| {
        right
            .iter()
            .any(|right| file_scopes_overlap(&left.pattern, &right.pattern))
    })
}

fn file_scopes_overlap(left: &str, right: &str) -> bool {
    let left = canonical_file_scope(left).unwrap_or_else(|| "*".to_string());
    let right = canonical_file_scope(right).unwrap_or_else(|| "*".to_string());
    let ambiguous = |value: &str| value.contains(['?', '*', '[', ']', '{', '}', '!']);
    ambiguous(&left)
        || ambiguous(&right)
        || left == "."
        || right == "."
        || left == right
        || left.starts_with(&format!("{right}/"))
        || right.starts_with(&format!("{left}/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::memory::AgentIdentityResolver;
    use crate::orchestration::git::GitError;
    use crate::orchestration::model::AgentState;
    use std::sync::{Mutex, PoisonError};

    #[derive(Clone)]
    struct FakeGit {
        main: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<(PathBuf, String, String)>>>,
        removals: Arc<Mutex<Vec<(PathBuf, String)>>>,
        fail: bool,
        fail_remove: bool,
    }

    impl GitWorktrees for FakeGit {
        fn resolve_main(&self, _repo_root: &Path) -> Result<String, GitError> {
            Ok(self
                .main
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone())
        }

        fn create_worktree(
            &self,
            _repo_root: &Path,
            path: &Path,
            branch: &str,
            base_commit: &str,
        ) -> Result<(), GitError> {
            if self.fail {
                return Err(GitError::Command("simulated create failure".to_string()));
            }
            self.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((
                    path.to_path_buf(),
                    branch.to_string(),
                    base_commit.to_string(),
                ));
            Ok(())
        }

        fn remove_worktree(
            &self,
            _repo_root: &Path,
            path: &Path,
            branch: &str,
        ) -> Result<(), GitError> {
            if self.fail_remove {
                return Err(GitError::Command("simulated remove failure".to_string()));
            }
            self.removals
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((path.to_path_buf(), branch.to_string()));
            Ok(())
        }
    }

    fn temp_repo(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "optimus-orchestration-{name}-{}-{}",
            std::process::id(),
            crate::control_plane::memory::test_timestamp()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    fn request(branch: &str) -> PrepareAgent {
        PrepareAgent {
            parent: None,
            name: branch.to_string(),
            task: "Implement an independent unit".to_string(),
            command: "codex".to_string(),
            branch: branch.to_string(),
            kind: AgentKind::Writer,
            file_scope: vec![FileScope::new(branch)],
        }
    }

    #[test]
    fn sibling_agents_keep_the_session_base_after_main_moves() {
        let repo = temp_repo("stable-base");
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let main = Arc::new(Mutex::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let git = FakeGit {
            main: Arc::clone(&main),
            calls: Arc::clone(&calls),
            removals: Arc::new(Mutex::new(Vec::new())),
            fail: false,
            fail_remove: false,
        };
        let orchestrator = SessionOrchestrator::new(store, None, git);
        let session = orchestrator.start_session(&repo, "fan-out", 10).unwrap();
        orchestrator
            .prepare_agent(session.id, request("feat/a"), 11)
            .unwrap();
        *main.lock().unwrap() = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
        orchestrator
            .prepare_agent(session.id, request("feat/b"), 12)
            .unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(2, calls.len());
        assert!(calls.iter().all(|call| call.2 == session.base_commit));
        assert_eq!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            session.base_commit
        );

        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn failed_worktree_creation_is_durable_and_not_reported_ready() {
        let repo = temp_repo("failed-create");
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let removals = Arc::new(Mutex::new(Vec::new()));
        let git = FakeGit {
            main: Arc::new(Mutex::new(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            )),
            calls: Arc::new(Mutex::new(Vec::new())),
            removals: Arc::clone(&removals),
            fail: true,
            fail_remove: false,
        };
        let orchestrator = SessionOrchestrator::new(Arc::clone(&store), None, git);
        let session = orchestrator.start_session(&repo, "failure", 10).unwrap();
        assert!(orchestrator
            .prepare_agent(session.id, request("feat/fail"), 11)
            .is_err());

        let agents = store.agents_for_session(session.id).unwrap();
        assert_eq!(1, agents.len());
        assert_eq!(AgentState::Failed, agents[0].state);
        assert!(agents[0]
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("simulated"));
        assert!(store.resolve(crate::domain::ids::SurfaceId(1)).is_none());
        assert!(removals.lock().unwrap().is_empty());

        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn active_siblings_require_provably_disjoint_file_scopes() {
        let repo = temp_repo("scope-overlap");
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let git = FakeGit {
            main: Arc::new(Mutex::new(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            )),
            calls: Arc::new(Mutex::new(Vec::new())),
            removals: Arc::new(Mutex::new(Vec::new())),
            fail: false,
            fail_remove: false,
        };
        let orchestrator = SessionOrchestrator::new(store, None, git);
        let session = orchestrator.start_session(&repo, "fan-out", 10).unwrap();
        let mut first = request("feat/shell");
        first.file_scope = vec![FileScope::new("shell")];
        orchestrator.prepare_agent(session.id, first, 11).unwrap();

        for scope in ["shell/src", "SHELL", "shell/.", "*"] {
            let mut overlapping = request(&format!("feat/{scope}"));
            overlapping.file_scope = vec![FileScope::new(scope)];
            assert!(orchestrator
                .prepare_agent(session.id, overlapping, 12)
                .unwrap_err()
                .to_string()
                .contains("overlaps"));
        }
        for scope in ["ui.", "ui /src"] {
            let mut alias = request(&format!("feat/{scope}"));
            alias.file_scope = vec![FileScope::new(scope)];
            assert!(orchestrator.prepare_agent(session.id, alias, 12).is_err());
        }
        let mut disjoint = request("feat/ui");
        disjoint.file_scope = vec![FileScope::new("ui")];
        assert!(orchestrator.prepare_agent(session.id, disjoint, 13).is_ok());
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn rollback_removes_the_worktree_and_clears_durable_path_state() {
        let repo = temp_repo("rollback");
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let removals = Arc::new(Mutex::new(Vec::new()));
        let git = FakeGit {
            main: Arc::new(Mutex::new(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            )),
            calls: Arc::new(Mutex::new(Vec::new())),
            removals: Arc::clone(&removals),
            fail: false,
            fail_remove: false,
        };
        let orchestrator = SessionOrchestrator::new(Arc::clone(&store), None, git);
        let session = orchestrator.start_session(&repo, "rollback", 10).unwrap();
        let agent = orchestrator
            .prepare_agent(session.id, request("feat/rollback"), 11)
            .unwrap();
        orchestrator
            .rollback_agent(agent.id, "batch failed", 12)
            .unwrap();
        let rolled_back = store.agent(agent.id).unwrap().unwrap();
        assert_eq!(AgentState::Failed, rolled_back.state);
        assert!(rolled_back.worktree_path.is_none());
        assert_eq!(1, removals.lock().unwrap().len());
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn failed_rollback_keeps_the_worktree_path_for_durable_cleanup_retry() {
        let repo = temp_repo("rollback-failure");
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let git = FakeGit {
            main: Arc::new(Mutex::new(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            )),
            calls: Arc::new(Mutex::new(Vec::new())),
            removals: Arc::new(Mutex::new(Vec::new())),
            fail: false,
            fail_remove: true,
        };
        let orchestrator = SessionOrchestrator::new(Arc::clone(&store), None, git);
        let session = orchestrator.start_session(&repo, "rollback", 10).unwrap();
        let agent = orchestrator
            .prepare_agent(session.id, request("feat/rollback-failure"), 11)
            .unwrap();

        assert!(orchestrator
            .rollback_agent(agent.id, "batch failed", 12)
            .is_err());
        let retained = store.agent(agent.id).unwrap().unwrap();
        assert_eq!(AgentState::Failed, retained.state);
        assert!(retained.worktree_path.is_some());
        std::fs::remove_dir_all(repo).unwrap();
    }
}
