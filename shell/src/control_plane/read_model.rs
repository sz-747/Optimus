//! M2 free-tier control-plane projection. This is a read model over the orchestration rows and
//! provenance log, not a second source of truth. Paid `why` text never enters these DTOs.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::control_plane::correlation::{CorrelationEngine, EdgeRelation};
use crate::control_plane::memory::{MemoryError, MemoryStore, RecordKind, StoredRecord};
use crate::domain::capacity::{CapacityLevel, CapacityModel};
use crate::orchestration::model::{Agent, AgentState, SessionState};
use crate::orchestration::store::{OrchestrationError, OrchestrationStore};

const DEFAULT_ACTIVITY_LIMIT: usize = 250;
const DEFAULT_STALLED_AFTER_MS: i64 = 5 * 60 * 1_000;

#[derive(Debug)]
pub enum ReadModelError {
    Memory(MemoryError),
    Orchestration(OrchestrationError),
}

impl fmt::Display for ReadModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Memory(error) => error.fmt(f),
            Self::Orchestration(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ReadModelError {}

impl From<MemoryError> for ReadModelError {
    fn from(value: MemoryError) -> Self {
        Self::Memory(value)
    }
}

impl From<OrchestrationError> for ReadModelError {
    fn from(value: OrchestrationError) -> Self {
        Self::Orchestration(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneSnapshot {
    pub revision: u64,
    pub generated_at: i64,
    pub workspaces: Vec<WorkspaceNode>,
    pub sessions: Vec<SessionNode>,
    pub agents: Vec<AgentNode>,
    pub activity: Vec<ActivityItem>,
    pub contradictions: Vec<ContradictionBadge>,
    pub capacity: CapacityNode,
    pub totals: TotalsNode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceNode {
    pub id: String,
    pub name: String,
    pub repo_root: String,
    pub session_count: usize,
    pub active_agents: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNode {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub base_commit: String,
    pub state: &'static str,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNode {
    pub id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub task: String,
    pub kind: &'static str,
    pub state: &'static str,
    pub status: String,
    pub branch: String,
    pub worktree: Option<String>,
    pub base_commit: String,
    pub base_drift: bool,
    pub started_at: Option<i64>,
    pub elapsed_ms: i64,
    pub last_activity_at: Option<i64>,
    pub contradiction_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityItem {
    pub id: i64,
    pub workspace_id: String,
    pub agent_id: String,
    pub kind: &'static str,
    pub fact: String,
    pub file_key: Option<String>,
    pub branch: Option<String>,
    pub trust: &'static str,
    pub timestamp: i64,
    pub has_provenance: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContradictionBadge {
    pub id: String,
    pub workspace_id: String,
    pub left_agent_id: String,
    pub right_agent_id: String,
    pub file_key: String,
    pub left_record_id: i64,
    pub right_record_id: i64,
    pub rule: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityNode {
    pub used: i32,
    pub reserved: i32,
    pub max: i32,
    pub level: &'static str,
    pub fraction: f64,
    pub at_cap: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotalsNode {
    pub running: usize,
    pub stalled: usize,
    pub waiting: usize,
    pub done: usize,
    pub failed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneUpdate {
    pub revision: u64,
}

type Subscriber = Arc<dyn Fn(u64) -> bool + Send + Sync>;

pub struct RevisionHub {
    revision: AtomicU64,
    subscribers: Mutex<Vec<Subscriber>>,
}

impl RevisionHub {
    fn new() -> Self {
        Self {
            revision: AtomicU64::new(1),
            subscribers: Mutex::new(Vec::new()),
        }
    }

    pub fn current(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn invalidate(&self) -> u64 {
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|subscriber| subscriber(revision));
        revision
    }

    pub fn subscribe(&self, subscriber: Subscriber) -> u64 {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(subscriber);
        self.current()
    }
}

pub struct ControlPlaneService {
    sessions: Arc<OrchestrationStore>,
    memory: Arc<MemoryStore>,
    correlations: CorrelationEngine,
    capacity: Arc<CapacityModel>,
    hub: Arc<RevisionHub>,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    stalled_after_ms: i64,
}

impl ControlPlaneService {
    pub fn start(
        sessions: Arc<OrchestrationStore>,
        memory: Arc<MemoryStore>,
        capacity: Arc<CapacityModel>,
    ) -> Arc<Self> {
        Self::start_with_clock(
            sessions,
            memory,
            capacity,
            Arc::new(now_ms),
            DEFAULT_STALLED_AFTER_MS,
        )
    }

    fn start_with_clock(
        sessions: Arc<OrchestrationStore>,
        memory: Arc<MemoryStore>,
        capacity: Arc<CapacityModel>,
        now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
        stalled_after_ms: i64,
    ) -> Arc<Self> {
        let hub = Arc::new(RevisionHub::new());
        let record_hub = Arc::clone(&hub);
        memory.subscribe_records(Arc::new(move |_| {
            record_hub.invalidate();
        }));
        let edge_hub = Arc::clone(&hub);
        memory.subscribe_edges(Arc::new(move |_| {
            edge_hub.invalidate();
        }));
        let capacity_hub = Arc::clone(&hub);
        capacity.subscribe_state_changed(Box::new(move |_| {
            capacity_hub.invalidate();
        }));
        Arc::new(Self {
            correlations: CorrelationEngine::new(Arc::clone(&memory)),
            sessions,
            memory,
            capacity,
            hub,
            now_ms,
            stalled_after_ms,
        })
    }

    pub fn invalidate(&self) -> u64 {
        self.hub.invalidate()
    }

    pub fn subscribe(&self, subscriber: Subscriber) -> u64 {
        self.hub.subscribe(subscriber)
    }

    pub fn snapshot(&self) -> Result<ControlPlaneSnapshot, ReadModelError> {
        self.snapshot_with_limit(DEFAULT_ACTIVITY_LIMIT)
    }

    pub fn snapshot_with_limit(
        &self,
        activity_limit: usize,
    ) -> Result<ControlPlaneSnapshot, ReadModelError> {
        let generated_at = (self.now_ms)();
        let workspaces = self.sessions.workspaces()?;
        let sessions = self.sessions.sessions()?;
        let agents = self.sessions.agents()?;
        let records = self.memory.recent_records(activity_limit)?;
        let records_by_id = records
            .iter()
            .cloned()
            .map(|record| (record.id, record))
            .collect::<HashMap<_, _>>();
        let latest_status = latest_statuses(&records);

        let mut contradictions = Vec::new();
        let mut contradiction_counts = HashMap::<String, usize>::new();
        let workspace_ids = workspaces
            .iter()
            .map(|workspace| workspace.id.0)
            .collect::<HashSet<_>>();
        for workspace_id in workspace_ids {
            for edge in self.correlations.edges_for_workspace(workspace_id)? {
                if edge.relation != EdgeRelation::Contradicts {
                    continue;
                }
                let left = records_by_id
                    .get(&edge.from_id)
                    .cloned()
                    .or_else(|| self.memory.record(edge.from_id).ok().flatten());
                let right = records_by_id
                    .get(&edge.to_id)
                    .cloned()
                    .or_else(|| self.memory.record(edge.to_id).ok().flatten());
                let (Some(left), Some(right)) = (left, right) else {
                    continue;
                };
                let Some(file_key) = left.file_key.clone().or(right.file_key.clone()) else {
                    continue;
                };
                *contradiction_counts
                    .entry(left.agent_id.clone())
                    .or_default() += 1;
                *contradiction_counts
                    .entry(right.agent_id.clone())
                    .or_default() += 1;
                contradictions.push(ContradictionBadge {
                    id: format!("edge-{}-{}-contradicts", edge.from_id, edge.to_id),
                    workspace_id: format!("ws-{workspace_id}"),
                    left_agent_id: left.agent_id,
                    right_agent_id: right.agent_id,
                    file_key,
                    left_record_id: edge.from_id,
                    right_record_id: edge.to_id,
                    rule: edge.rule,
                });
            }
        }

        let projected_agents = agents
            .iter()
            .map(|agent| {
                project_agent(
                    agent,
                    &agents,
                    generated_at,
                    self.stalled_after_ms,
                    latest_status.get(&agent.agent_key),
                    *contradiction_counts.get(&agent.agent_key).unwrap_or(&0),
                )
            })
            .collect::<Vec<_>>();
        let totals = totals(&projected_agents);
        let workspace_nodes = workspaces
            .into_iter()
            .map(|workspace| WorkspaceNode {
                id: workspace.id.to_string(),
                name: workspace.name,
                repo_root: workspace.repo_root.to_string_lossy().into_owned(),
                session_count: sessions
                    .iter()
                    .filter(|session| session.workspace_id == workspace.id)
                    .count(),
                active_agents: agents
                    .iter()
                    .filter(|agent| {
                        agent.workspace_id == workspace.id && !agent.state.is_terminal()
                    })
                    .count(),
            })
            .collect();
        let session_nodes = sessions
            .into_iter()
            .map(|session| SessionNode {
                id: session.id.to_string(),
                workspace_id: session.workspace_id.to_string(),
                name: session.name,
                base_commit: session.base_commit,
                state: match session.state {
                    SessionState::Active => "active",
                    SessionState::Completed => "completed",
                    SessionState::Failed => "failed",
                },
                created_at: session.created_ts,
            })
            .collect();
        let activity = records.into_iter().map(project_activity).collect();
        let state = self.capacity.state();
        let filled = state.used + state.reserved;
        let fraction = if state.max > 0 {
            (f64::from(filled) / f64::from(state.max)).clamp(0.0, 1.0)
        } else {
            0.0
        };

        Ok(ControlPlaneSnapshot {
            revision: self.hub.current(),
            generated_at,
            workspaces: workspace_nodes,
            sessions: session_nodes,
            agents: projected_agents,
            activity,
            contradictions,
            capacity: CapacityNode {
                used: state.used,
                reserved: state.reserved,
                max: state.max,
                level: match state.level {
                    CapacityLevel::Calm => "calm",
                    CapacityLevel::Warn => "warn",
                    CapacityLevel::Cap => "cap",
                },
                fraction,
                at_cap: state.level == CapacityLevel::Cap,
            },
            totals,
        })
    }
}

fn latest_statuses(records: &[StoredRecord]) -> HashMap<String, String> {
    let mut statuses = HashMap::new();
    for record in records {
        if record.kind == RecordKind::Status {
            statuses
                .entry(record.agent_id.clone())
                .or_insert_with(|| record.fact.clone());
        }
    }
    statuses
}

fn project_agent(
    agent: &Agent,
    agents: &[Agent],
    now: i64,
    stalled_after_ms: i64,
    status: Option<&String>,
    contradiction_count: usize,
) -> AgentNode {
    let last_activity = [agent.last_record_ts, agent.last_output_ts, agent.started_ts]
        .into_iter()
        .flatten()
        .max();
    let state = match agent.state {
        AgentState::Planned | AgentState::Ready => "waiting",
        AgentState::Running | AgentState::Stopping
            if last_activity.is_some_and(|activity| now - activity >= stalled_after_ms) =>
        {
            "stalled"
        }
        AgentState::Running | AgentState::Stopping => "running",
        AgentState::Done => "done",
        AgentState::Failed => "failed",
        AgentState::Interrupted => "interrupted",
    };
    let started = agent.started_ts;
    let end = agent.ended_ts.unwrap_or(now);
    AgentNode {
        id: agent.agent_key.clone(),
        session_id: agent.session_id.to_string(),
        workspace_id: agent.workspace_id.to_string(),
        parent_id: agent.parent_id.map(|id| id.to_string()),
        name: agent.name.clone(),
        task: agent.task.clone(),
        kind: agent.kind.as_str(),
        state,
        status: status.cloned().unwrap_or_else(|| agent.task.clone()),
        branch: agent.branch.clone(),
        worktree: agent
            .worktree_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        base_commit: agent.base_commit.clone(),
        base_drift: agents.iter().any(|sibling| {
            sibling.session_id == agent.session_id
                && sibling.id != agent.id
                && sibling.base_commit != agent.base_commit
        }),
        started_at: started,
        elapsed_ms: started.map_or(0, |start| end.saturating_sub(start)),
        last_activity_at: last_activity,
        contradiction_count,
    }
}

fn project_activity(record: StoredRecord) -> ActivityItem {
    ActivityItem {
        id: record.id,
        workspace_id: format!("ws-{}", record.workspace_id),
        agent_id: record.agent_id,
        kind: record.kind.as_str(),
        fact: record.fact,
        file_key: record.file_key,
        branch: record.branch,
        trust: record.trust.as_str(),
        timestamp: record.ts,
        has_provenance: record.why.is_some(),
    }
}

fn totals(agents: &[AgentNode]) -> TotalsNode {
    TotalsNode {
        running: agents
            .iter()
            .filter(|agent| agent.state == "running")
            .count(),
        stalled: agents
            .iter()
            .filter(|agent| agent.state == "stalled")
            .count(),
        waiting: agents
            .iter()
            .filter(|agent| agent.state == "waiting")
            .count(),
        done: agents.iter().filter(|agent| agent.state == "done").count(),
        failed: agents
            .iter()
            .filter(|agent| matches!(agent.state, "failed" | "interrupted"))
            .count(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::correlation::CorrelationWorker;
    use crate::control_plane::memory::{AgentIdentity, AgentIdentityResolver, TrustedRecord};
    use crate::domain::capacity::CapacityProvider;
    use crate::domain::ids::SurfaceId;
    use crate::orchestration::model::{AgentId, AgentKind, FileScope};
    use std::sync::atomic::{AtomicI64, Ordering};

    struct NoAgents;
    impl AgentIdentityResolver for NoAgents {
        fn resolve(&self, _surface: SurfaceId) -> Option<AgentIdentity> {
            None
        }
    }

    struct Provider;
    impl CapacityProvider for Provider {
        fn total_phys_bytes(&self) -> u64 {
            16 * 1024 * 1024 * 1024
        }
        fn available_phys_bytes(&self) -> u64 {
            8 * 1024 * 1024 * 1024
        }
        fn commit_headroom_bytes(&self) -> u64 {
            8 * 1024 * 1024 * 1024
        }
        fn is_low_memory_signaled(&self) -> bool {
            false
        }
        fn subscribe_low_memory(&self, _listener: Box<dyn Fn() + Send + Sync>) {}
        fn measure_process_private_bytes(&self, _pid: i32) -> Option<u64> {
            Some(200 * 1024 * 1024)
        }
    }

    fn trusted(
        agent: &str,
        kind: RecordKind,
        fact: &str,
        file: Option<&str>,
        ts: i64,
    ) -> TrustedRecord {
        TrustedRecord {
            fact: fact.to_string(),
            why: Some("paid provenance stays private".to_string()),
            agent_id: agent.to_string(),
            kind,
            file_key: file.map(str::to_string),
            branch: Some(format!("feat/{agent}")),
            workspace_id: 1,
            reverses: None,
            parent_record: None,
            symbols: Vec::new(),
            ts: Some(ts),
        }
    }

    fn fixture() -> (
        Arc<ControlPlaneService>,
        Arc<OrchestrationStore>,
        Arc<MemoryStore>,
        CorrelationWorker,
        Arc<AtomicI64>,
    ) {
        let sessions = Arc::new(OrchestrationStore::in_memory().unwrap());
        let workspace = sessions
            .get_or_create_workspace("optimus", std::path::Path::new("C:/dev/Optimus"), 1)
            .unwrap();
        let session = sessions
            .create_session(workspace.id, "control plane", "abc123", 2)
            .unwrap();
        let mut parent = None;
        for index in 1..=5 {
            let agent = sessions
                .plan_agent(
                    session.id,
                    parent,
                    &format!("Agent {index}"),
                    &format!("Task {index}"),
                    "codex",
                    &format!("feat/agent-{index}"),
                    AgentKind::Writer,
                    &[FileScope::new(format!("unit-{index}"))],
                    index + 2,
                )
                .unwrap();
            sessions.mark_ready(agent.id, 10).unwrap();
            sessions
                .mark_running(agent.id, SurfaceId(index as i32), 100)
                .unwrap();
            if index == 1 {
                parent = Some(agent.id);
            }
            if index == 5 {
                sessions.mark_done(agent.id, 0, 150).unwrap();
            }
        }
        let memory = Arc::new(MemoryStore::in_memory(Arc::new(NoAgents)).unwrap());
        let worker = CorrelationWorker::attach(Arc::clone(&memory));
        memory
            .append_trusted(trusted(
                "agent-1",
                RecordKind::Status,
                "Reviewing the data model",
                None,
                100,
            ))
            .unwrap();
        memory
            .append_trusted(trusted(
                "agent-2",
                RecordKind::Change,
                "Changed capacity",
                Some("capacity.rs"),
                110,
            ))
            .unwrap();
        memory
            .append_trusted(trusted(
                "agent-3",
                RecordKind::Change,
                "Also changed capacity",
                Some("capacity.rs"),
                120,
            ))
            .unwrap();
        worker.flush();
        let capacity = Arc::new(CapacityModel::new(Arc::new(Provider), None));
        let clock = Arc::new(AtomicI64::new(100 + DEFAULT_STALLED_AFTER_MS + 1));
        let clock_reader = Arc::clone(&clock);
        let service = ControlPlaneService::start_with_clock(
            Arc::clone(&sessions),
            Arc::clone(&memory),
            capacity,
            Arc::new(move || clock_reader.load(Ordering::Relaxed)),
            DEFAULT_STALLED_AFTER_MS,
        );
        (service, sessions, memory, worker, clock)
    }

    #[test]
    fn five_agent_snapshot_has_parentage_states_activity_and_bilateral_collision_badges() {
        let (service, _sessions, _memory, _worker, _clock) = fixture();
        let snapshot = service.snapshot().unwrap();
        assert_eq!(5, snapshot.agents.len());
        assert_eq!(Some("agent-1"), snapshot.agents[1].parent_id.as_deref());
        assert_eq!("stalled", snapshot.agents[0].state);
        assert_eq!("done", snapshot.agents[4].state);
        assert_eq!("Reviewing the data model", snapshot.agents[0].status);
        assert_eq!(
            vec![3, 2, 1],
            snapshot
                .activity
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(1, snapshot.contradictions.len());
        assert_eq!(1, snapshot.agents[1].contradiction_count);
        assert_eq!(1, snapshot.agents[2].contradiction_count);
        assert!(snapshot.activity.iter().all(|item| item.has_provenance));
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("paid provenance stays private"));
    }

    #[test]
    fn record_and_edge_commits_advance_revision_without_polling() {
        let (service, _sessions, memory, worker, _clock) = fixture();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&seen);
        service.subscribe(Arc::new(move |revision| {
            output.lock().unwrap().push(revision);
            true
        }));
        let before = service.snapshot().unwrap().revision;
        memory
            .append_trusted(trusted(
                "agent-4",
                RecordKind::Status,
                "Tests green",
                None,
                130,
            ))
            .unwrap();
        worker.flush();
        let revisions = seen.lock().unwrap();
        assert!(!revisions.is_empty());
        assert!(revisions.iter().all(|revision| *revision > before));
    }

    #[test]
    fn dropped_subscriber_is_removed_on_the_next_update() {
        let (service, _sessions, memory, _worker, _clock) = fixture();
        let called = Arc::new(Mutex::new(0));
        let count = Arc::clone(&called);
        service.subscribe(Arc::new(move |_| {
            *count.lock().unwrap() += 1;
            false
        }));
        memory
            .append_trusted(trusted("agent-1", RecordKind::Status, "one", None, 200))
            .unwrap();
        memory
            .append_trusted(trusted("agent-1", RecordKind::Status, "two", None, 201))
            .unwrap();
        assert_eq!(1, *called.lock().unwrap());
    }

    #[test]
    fn explicit_invalidation_covers_orchestration_transitions() {
        let (service, sessions, _memory, _worker, _clock) = fixture();
        let before = service.snapshot().unwrap().revision;
        sessions.touch_output(AgentId(1), 999).unwrap();
        service.invalidate();
        let after = service.snapshot().unwrap();
        assert!(after.revision > before);
        assert_eq!(Some(999), after.agents[0].last_activity_at);
        assert_eq!("running", after.agents[0].state);
    }

    #[test]
    fn sibling_base_mismatch_is_projected_as_drift() {
        let (_service, sessions, _memory, _worker, _clock) = fixture();
        let mut agents = sessions.agents().unwrap();
        agents[1].base_commit = "different-base".to_string();
        let first = project_agent(&agents[0], &agents, 1_000, 30_000, None, 0);
        let second = project_agent(&agents[1], &agents, 1_000, 30_000, None, 0);
        assert!(first.base_drift);
        assert!(second.base_drift);
    }
}
