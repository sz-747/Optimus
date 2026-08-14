use std::sync::Arc;

use regex::Regex;
use sha2::{Digest, Sha256};

use super::model::{RelayCommand, RelayCommandKind, StartRunPayload, StopAgentPayload};
use crate::orchestration::git::GitWorktrees;
use crate::orchestration::model::{AgentId, AgentKind, FileScope};
use crate::orchestration::runtime::{AgentRuntime, RuntimeSupervisor};
use crate::orchestration::store::{
    OrchestrationError, OrchestrationStore, RelayCommandClaim, RelayCommandState,
};
use crate::orchestration::{
    file_scope_sets_overlap, valid_file_scope, PrepareAgent, SessionOrchestrator,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayExecutionResult {
    pub status: &'static str,
    pub error_code: Option<String>,
}

impl RelayExecutionResult {
    fn succeeded() -> Self {
        Self {
            status: "succeeded",
            error_code: None,
        }
    }

    fn failed(code: &str) -> Self {
        Self {
            status: "failed",
            error_code: Some(code.to_string()),
        }
    }

    fn rejected(code: &str) -> Self {
        Self {
            status: "rejected",
            error_code: Some(code.to_string()),
        }
    }
}

pub trait RelayCommandExecutor: Send + Sync {
    fn execute(&self, command: &RelayCommand, now: i64) -> RelayExecutionResult;
}

pub struct DesktopRelayExecutor<G: GitWorktrees, R: AgentRuntime> {
    device_id: String,
    store: Arc<OrchestrationStore>,
    orchestrator: Arc<SessionOrchestrator<G>>,
    runtime: Arc<RuntimeSupervisor<R>>,
}

impl<G: GitWorktrees, R: AgentRuntime> DesktopRelayExecutor<G, R> {
    pub fn new(
        device_id: impl Into<String>,
        store: Arc<OrchestrationStore>,
        orchestrator: Arc<SessionOrchestrator<G>>,
        runtime: Arc<RuntimeSupervisor<R>>,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            store,
            orchestrator,
            runtime,
        }
    }

    fn execute_fresh(
        &self,
        command: &RelayCommand,
        now: i64,
    ) -> Result<RelayExecutionResult, ExecutionFailure> {
        match command.kind {
            RelayCommandKind::StartRun => {
                let payload = command
                    .start_payload()
                    .map_err(|_| ExecutionFailure::rejected("invalid_payload"))?;
                self.start_run(command, payload, now)
            }
            RelayCommandKind::StopAgent => {
                let payload = command
                    .stop_payload()
                    .map_err(|_| ExecutionFailure::rejected("invalid_payload"))?;
                self.stop_agent(command, payload, now)
            }
        }
    }

    fn start_run(
        &self,
        command: &RelayCommand,
        payload: StartRunPayload,
        now: i64,
    ) -> Result<RelayExecutionResult, ExecutionFailure> {
        validate_start(&payload)?;
        let workspace = self
            .store
            .relay_workspace(&payload.workspace_id)
            .map_err(ExecutionFailure::store)?
            .ok_or_else(|| ExecutionFailure::rejected("unknown_workspace"))?;
        let provider = self
            .store
            .relay_provider(&payload.profile_id)
            .map_err(ExecutionFailure::store)?
            .ok_or_else(|| ExecutionFailure::rejected("unknown_profile"))?;
        if provider.argument_mode != "prompt_arg" {
            return Err(ExecutionFailure::rejected("unsupported_provider"));
        }
        let session = self
            .orchestrator
            .start_session(&workspace.workspace.repo_root, &payload.session_name, now)
            .map_err(|_| ExecutionFailure::failed("session_start_failed"))?;
        self.store
            .set_relay_session_label(session.id, &payload.session_name)
            .map_err(ExecutionFailure::store)?;
        self.store
            .link_relay_command(&command.command_id, Some(session.id), None)
            .map_err(ExecutionFailure::store)?;
        let agent_count = payload.agents.len();
        let mut prepared = Vec::with_capacity(agent_count);
        for (index, specification) in payload.agents.into_iter().enumerate() {
            let display_name = specification.agent_name.clone();
            let kind = match specification.kind.as_str() {
                "writer" => AgentKind::Writer,
                "recon" => AgentKind::Recon,
                _ => return Err(ExecutionFailure::rejected("invalid_agent_kind")),
            };
            let prompt = scoped_prompt(&specification.task, &specification.file_scope);
            let command_line = format!(
                "{} {}",
                provider.command_prefix,
                quote_windows_argument(&prompt)
            );
            let request = PrepareAgent {
                parent: None,
                name: specification.agent_name,
                task: specification.task,
                command: command_line,
                branch: remote_branch(&command.command_id, index, agent_count)?,
                kind,
                file_scope: specification
                    .file_scope
                    .into_iter()
                    .map(FileScope::new)
                    .collect(),
            };
            let agent = match self.orchestrator.prepare_agent(session.id, request, now) {
                Ok(agent) => agent,
                Err(_) => {
                    let session_agents = self
                        .store
                        .agents_for_session(session.id)
                        .map_err(ExecutionFailure::store)?
                        .into_iter()
                        .map(|agent| agent.id)
                        .collect::<Vec<_>>();
                    self.abort_agents(&session_agents, now)?;
                    return Err(ExecutionFailure::failed("worktree_create_failed"));
                }
            };
            if self
                .store
                .set_agent_model_auth(
                    agent.id,
                    &provider.auth_mode,
                    provider.credential_env.as_deref(),
                )
                .is_err()
            {
                prepared.push(agent.id);
                self.abort_agents(&prepared, now)?;
                return Err(ExecutionFailure::failed("local_store_failed"));
            }
            if self
                .store
                .set_relay_agent_label(agent.id, &display_name)
                .is_err()
            {
                prepared.push(agent.id);
                self.abort_agents(&prepared, now)?;
                return Err(ExecutionFailure::failed("local_store_failed"));
            }
            prepared.push(agent.id);
        }
        if self
            .store
            .link_relay_command(
                &command.command_id,
                Some(session.id),
                prepared.first().copied(),
            )
            .is_err()
        {
            self.abort_agents(&prepared, now)?;
            return Err(ExecutionFailure::failed("local_store_failed"));
        }
        let mut started = Vec::new();
        for (index, id) in prepared.iter().copied().enumerate() {
            if self.runtime.start_agent(id).is_err() {
                let mut cleanup_failed = false;
                for started_id in started {
                    if self.runtime.stop_agent(started_id).is_err() {
                        cleanup_failed = true;
                    } else {
                        cleanup_failed |= self.abort_agents(&[started_id], now).is_err();
                    }
                }
                cleanup_failed |= self.abort_agents(&prepared[index..], now).is_err();
                if cleanup_failed {
                    return Err(ExecutionFailure::failed("worktree_cleanup_failed"));
                }
                return Err(ExecutionFailure::failed("agent_start_failed"));
            }
            started.push(id);
        }
        Ok(RelayExecutionResult::succeeded())
    }

    fn abort_agents(&self, agents: &[AgentId], now: i64) -> Result<(), ExecutionFailure> {
        let mut cleanup_failed = false;
        for id in agents.iter().rev() {
            if let Err(error) = self.orchestrator.rollback_agent(
                *id,
                "Parallel batch aborted before all agents started",
                now,
            ) {
                cleanup_failed = true;
                eprintln!("failed to roll back agent {id}: {error}");
            }
        }
        if cleanup_failed {
            Err(ExecutionFailure::failed("worktree_cleanup_failed"))
        } else {
            Ok(())
        }
    }

    fn stop_agent(
        &self,
        command: &RelayCommand,
        payload: StopAgentPayload,
        _now: i64,
    ) -> Result<RelayExecutionResult, ExecutionFailure> {
        let agent_id = parse_agent_id(&payload.agent_id)?;
        let agent = self
            .store
            .agent(agent_id)
            .map_err(ExecutionFailure::store)?
            .ok_or_else(|| ExecutionFailure::rejected("unknown_agent"))?;
        if self
            .store
            .relay_workspace_for_key(agent.workspace_id)
            .map_err(ExecutionFailure::store)?
            .is_none()
        {
            return Err(ExecutionFailure::rejected("agent_not_relay_enabled"));
        }
        self.store
            .link_relay_command(&command.command_id, None, Some(agent.id))
            .map_err(ExecutionFailure::store)?;
        self.runtime
            .stop_agent(agent.id)
            .map_err(|_| ExecutionFailure::failed("agent_stop_failed"))?;
        Ok(RelayExecutionResult::succeeded())
    }

    fn finish(
        &self,
        command: &RelayCommand,
        result: &RelayExecutionResult,
        now: i64,
    ) -> Result<(), OrchestrationError> {
        self.store.finish_relay_command(
            &command.command_id,
            result.status,
            None,
            None,
            result.error_code.as_deref(),
            now,
        )
    }
}

impl<G, R> RelayCommandExecutor for DesktopRelayExecutor<G, R>
where
    G: GitWorktrees + 'static,
    R: AgentRuntime + 'static,
{
    fn execute(&self, command: &RelayCommand, now: i64) -> RelayExecutionResult {
        if command.device_id != self.device_id {
            return RelayExecutionResult::rejected("wrong_device");
        }
        if command.expires_at <= now || command.issued_at > now.saturating_add(30_000) {
            return RelayExecutionResult::rejected("expired_command");
        }
        let payload_hash = command_hash(command);
        match self.store.claim_relay_command(
            &command.command_id,
            &payload_hash,
            command_kind(command),
            now,
        ) {
            Ok(RelayCommandClaim::Existing(state)) => return existing_result(&state),
            Ok(RelayCommandClaim::Fresh) => {}
            Err(_) => return RelayExecutionResult::rejected("command_id_conflict"),
        }
        let result = match self.execute_fresh(command, now) {
            Ok(result) => result,
            Err(failure) => failure.result,
        };
        if self.finish(command, &result, now).is_err() {
            return RelayExecutionResult::failed("command_persistence_failed");
        }
        result
    }
}

#[derive(Debug)]
struct ExecutionFailure {
    result: RelayExecutionResult,
}

impl ExecutionFailure {
    fn failed(code: &str) -> Self {
        Self {
            result: RelayExecutionResult::failed(code),
        }
    }

    fn rejected(code: &str) -> Self {
        Self {
            result: RelayExecutionResult::rejected(code),
        }
    }

    fn store(_error: OrchestrationError) -> Self {
        Self::failed("local_store_failed")
    }
}

fn validate_start(payload: &StartRunPayload) -> Result<(), ExecutionFailure> {
    validate_id(&payload.workspace_id)?;
    validate_id(&payload.profile_id)?;
    validate_text(&payload.session_name, 80)?;
    if payload.agents.is_empty() || payload.agents.len() > 8 {
        return Err(ExecutionFailure::rejected("invalid_agent_count"));
    }
    let mut scope_sets: Vec<Vec<FileScope>> = Vec::new();
    for agent in &payload.agents {
        validate_text(&agent.agent_name, 80)?;
        validate_text(&agent.task, 4_000)?;
        if !matches!(agent.kind.as_str(), "writer" | "recon") {
            return Err(ExecutionFailure::rejected("invalid_agent_kind"));
        }
        if payload.agents.len() > 1 && agent.file_scope.is_empty() {
            return Err(ExecutionFailure::rejected("missing_file_scope"));
        }
        for scope in &agent.file_scope {
            if !valid_file_scope(scope) {
                return Err(ExecutionFailure::rejected("invalid_file_scope"));
            }
        }
        let current = agent
            .file_scope
            .iter()
            .cloned()
            .map(FileScope::new)
            .collect::<Vec<_>>();
        if scope_sets
            .iter()
            .any(|previous| file_scope_sets_overlap(previous, &current))
        {
            return Err(ExecutionFailure::rejected("scope_overlap"));
        }
        scope_sets.push(current);
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), ExecutionFailure> {
    let pattern = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$").expect("static regex");
    if pattern.is_match(value) {
        Ok(())
    } else {
        Err(ExecutionFailure::rejected("invalid_identifier"))
    }
}

fn validate_text(value: &str, maximum: usize) -> Result<(), ExecutionFailure> {
    if value.trim().is_empty() || value.len() > maximum || value.contains(['\r', '\0']) {
        return Err(ExecutionFailure::rejected("invalid_text"));
    }
    Ok(())
}

fn parse_agent_id(value: &str) -> Result<AgentId, ExecutionFailure> {
    value
        .strip_prefix("agent-")
        .and_then(|suffix| suffix.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .map(AgentId)
        .ok_or_else(|| ExecutionFailure::rejected("invalid_agent_id"))
}

fn remote_branch(
    command_id: &str,
    index: usize,
    agent_count: usize,
) -> Result<String, ExecutionFailure> {
    let slug = command_id
        .chars()
        .filter(|value| value.is_ascii_hexdigit())
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    if slug.len() != 12 {
        return Err(ExecutionFailure::rejected("invalid_command_id"));
    }
    if agent_count == 1 {
        Ok(format!("optimus/remote-{slug}"))
    } else {
        Ok(format!("optimus/remote-{slug}-{}", index + 1))
    }
}

fn quote_windows_argument(value: &str) -> String {
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for character in value.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                quoted.push(character);
                backslashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

fn scoped_prompt(task: &str, scopes: &[String]) -> String {
    if scopes.is_empty() {
        return task.to_string();
    }
    format!(
        "Repository ownership boundary: modify only [{}]. Do not edit files outside these scopes.\n\n{}",
        scopes.join(", "),
        task
    )
}

fn command_hash(command: &RelayCommand) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(command.device_id.as_bytes());
    hasher.update(command_kind(command).as_bytes());
    hasher.update(serde_json::to_vec(&command.payload).unwrap_or_default());
    hasher.finalize().to_vec()
}

fn command_kind(command: &RelayCommand) -> &'static str {
    match command.kind {
        RelayCommandKind::StartRun => "start_run",
        RelayCommandKind::StopAgent => "stop_agent",
    }
}

fn existing_result(state: &RelayCommandState) -> RelayExecutionResult {
    match state.state.as_str() {
        "succeeded" => RelayExecutionResult::succeeded(),
        "rejected" => RelayExecutionResult::rejected(
            state.error_code.as_deref().unwrap_or("command_rejected"),
        ),
        "failed" => {
            RelayExecutionResult::failed(state.error_code.as_deref().unwrap_or("command_failed"))
        }
        _ => RelayExecutionResult {
            status: "running",
            error_code: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::git::GitError;
    use crate::orchestration::runtime::{RunningAgent, RuntimeEvent, RuntimeSink, RuntimeSpec};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn windows_argument_quoting_keeps_metacharacters_inside_one_argument() {
        assert_eq!(r#""simple""#, quote_windows_argument("simple"));
        assert_eq!(r#""a & b %PATH%""#, quote_windows_argument("a & b %PATH%"));
        assert_eq!(
            r#""say \"hello\"""#,
            quote_windows_argument("say \"hello\"")
        );
    }

    #[test]
    fn branch_is_derived_only_from_the_server_command_id() {
        assert_eq!(
            "optimus/remote-1234567890ab",
            remote_branch("12345678-90ab-4000-8000-000000000000", 0, 1).unwrap()
        );
    }

    #[derive(Clone)]
    struct FakeGit {
        creates: Arc<AtomicUsize>,
    }

    impl GitWorktrees for FakeGit {
        fn resolve_main(&self, _repo_root: &Path) -> Result<String, GitError> {
            Ok("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string())
        }

        fn create_worktree(
            &self,
            _repo_root: &Path,
            path: &Path,
            _branch: &str,
            _base_commit: &str,
        ) -> Result<(), GitError> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            std::fs::create_dir_all(path).map_err(GitError::Io)
        }
    }

    #[derive(Default)]
    struct FakeRuntime {
        starts: AtomicUsize,
        specs: Mutex<Vec<RuntimeSpec>>,
    }

    struct FakeHandle;

    impl RunningAgent for FakeHandle {
        fn stop(&self) {}
        fn pid(&self) -> u32 {
            42
        }
        fn output_snapshot(&self) -> Vec<u8> {
            Vec::new()
        }
    }

    impl AgentRuntime for Arc<FakeRuntime> {
        fn start(
            &self,
            spec: RuntimeSpec,
            _sink: RuntimeSink,
        ) -> Result<Arc<dyn RunningAgent>, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.specs.lock().unwrap().push(spec);
            Ok(Arc::new(FakeHandle))
        }
    }

    fn executor_fixture() -> (
        DesktopRelayExecutor<FakeGit, Arc<FakeRuntime>>,
        Arc<FakeRuntime>,
        Arc<AtomicUsize>,
        PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!(
            "optimus-relay-executor-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let workspace = store.get_or_create_workspace("repo", &root, 1).unwrap();
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
        let creates = Arc::new(AtomicUsize::new(0));
        let orchestrator = Arc::new(SessionOrchestrator::new(
            store.clone(),
            None,
            FakeGit {
                creates: creates.clone(),
            },
        ));
        let runtime = Arc::new(FakeRuntime::default());
        let supervisor = Arc::new(RuntimeSupervisor::new(
            store.clone(),
            runtime.clone(),
            r"\\.\pipe\optimus-relay-test".to_string(),
            None,
            Arc::new(|_, _event: RuntimeEvent| {}),
        ));
        (
            DesktopRelayExecutor::new("primary", store, orchestrator, supervisor),
            runtime,
            creates,
            root,
        )
    }

    fn start_command(task: &str) -> RelayCommand {
        RelayCommand {
            command_id: "12345678-90ab-4000-8000-000000000000".to_string(),
            device_id: "primary".to_string(),
            kind: RelayCommandKind::StartRun,
            payload: serde_json::json!({
                "workspaceId": "workspace-primary",
                "profileId": "codex-subscription",
                "sessionName": "Remote run",
                "agents": [{
                    "agentName": "Remote agent",
                    "task": task,
                    "kind": "writer",
                    "fileScope": []
                }]
            }),
            status: "leased".to_string(),
            issued_at: 10,
            expires_at: 10_000,
            lease_id: "11111111-1111-4111-8111-111111111111".to_string(),
            lease_owner: "22222222-2222-4222-8222-222222222222".to_string(),
            lease_until: 1_000,
            error_code: None,
            updated_at: 10,
        }
    }

    #[test]
    fn duplicate_delivery_starts_exactly_one_worktree_and_process() {
        let (executor, runtime, creates, root) = executor_fixture();
        let command = start_command("build it & do not invoke a shell");
        assert_eq!("succeeded", executor.execute(&command, 100).status);
        assert_eq!("succeeded", executor.execute(&command, 101).status);
        assert_eq!(1, creates.load(Ordering::SeqCst));
        assert_eq!(1, runtime.starts.load(Ordering::SeqCst));
        let command_line = &runtime.specs.lock().unwrap()[0].command;
        assert_eq!("codex \"build it & do not invoke a shell\"", command_line);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_command_id_with_different_payload_is_rejected_without_more_side_effects() {
        let (executor, runtime, creates, root) = executor_fixture();
        let first = start_command("first task");
        assert_eq!("succeeded", executor.execute(&first, 100).status);
        let changed = start_command("different task");
        let result = executor.execute(&changed, 101);
        assert_eq!("rejected", result.status);
        assert_eq!(Some("command_id_conflict"), result.error_code.as_deref());
        assert_eq!(1, creates.load(Ordering::SeqCst));
        assert_eq!(1, runtime.starts.load(Ordering::SeqCst));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parallel_delivery_uses_one_session_and_one_pinned_base_for_sibling_worktrees() {
        let (executor, runtime, creates, root) = executor_fixture();
        let mut command = start_command("shell task");
        command.payload["sessionName"] = serde_json::json!("Parallel release");
        command.payload["agents"] = serde_json::json!([
            {
                "agentName": "Shell",
                "task": "shell task",
                "kind": "writer",
                "fileScope": ["shell/src"]
            },
            {
                "agentName": "Web",
                "task": "web task",
                "kind": "writer",
                "fileScope": ["ui"]
            }
        ]);

        assert_eq!("succeeded", executor.execute(&command, 100).status);
        assert_eq!(2, creates.load(Ordering::SeqCst));
        assert_eq!(2, runtime.starts.load(Ordering::SeqCst));
        let sessions = executor.store.sessions().unwrap();
        let agents = executor.store.agents().unwrap();
        assert_eq!(1, sessions.len());
        assert_eq!(2, agents.len());
        assert!(agents
            .iter()
            .all(|agent| agent.session_id == sessions[0].id));
        assert!(agents
            .iter()
            .all(|agent| agent.base_commit == sessions[0].base_commit));
        assert_eq!("optimus/remote-1234567890ab-1", agents[0].branch);
        assert_eq!("optimus/remote-1234567890ab-2", agents[1].branch);
        assert_eq!(
            Some(&"Parallel release".to_string()),
            executor
                .store
                .relay_session_labels()
                .unwrap()
                .get(&sessions[0].id.to_string())
        );
        let labels = executor.store.relay_agent_labels().unwrap();
        assert_eq!(
            Some(&"Shell".to_string()),
            labels.get(&agents[0].id.to_string())
        );
        assert_eq!(
            Some(&"Web".to_string()),
            labels.get(&agents[1].id.to_string())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parallel_delivery_rejects_hierarchical_and_case_alias_scope_overlap() {
        for scopes in [["shell", "shell/src"], ["UI", "ui"], ["*", "docs"]] {
            let (executor, runtime, creates, root) = executor_fixture();
            let mut command = start_command("first");
            command.payload["agents"] = serde_json::json!([
                { "agentName": "One", "task": "first", "kind": "writer", "fileScope": [scopes[0]] },
                { "agentName": "Two", "task": "second", "kind": "writer", "fileScope": [scopes[1]] }
            ]);
            let result = executor.execute(&command, 100);
            assert_eq!("rejected", result.status);
            assert_eq!(Some("scope_overlap"), result.error_code.as_deref());
            assert_eq!(0, creates.load(Ordering::SeqCst));
            assert_eq!(0, runtime.starts.load(Ordering::SeqCst));
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
