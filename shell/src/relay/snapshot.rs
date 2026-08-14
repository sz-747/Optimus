use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::model::{
    RemoteActivity, RemoteAgent, RemoteCapacity, RemoteCatalog, RemoteCatalogEntry, RemoteSession,
    RemoteSnapshot, RemoteTotals, RemoteWorkspace, SnapshotEnvelope,
};
use super::RelayError;
use crate::control_plane::read_model::{ControlPlaneService, ControlPlaneSnapshot};
use crate::orchestration::store::OrchestrationStore;

pub trait SnapshotSource: Send + Sync {
    fn snapshot(&self) -> Result<ControlPlaneSnapshot, RelayError>;
}

impl SnapshotSource for ControlPlaneService {
    fn snapshot(&self) -> Result<ControlPlaneSnapshot, RelayError> {
        ControlPlaneService::snapshot(self).map_err(|error| RelayError::Contract(error.to_string()))
    }
}

pub fn remote_snapshot(
    local: &ControlPlaneSnapshot,
    store: &Arc<OrchestrationStore>,
    instance_id: &str,
    captured_at: i64,
) -> Result<SnapshotEnvelope, RelayError> {
    let registered = store.relay_workspaces()?;
    let providers = store.relay_providers()?;
    let session_labels = store.relay_session_labels()?;
    let agent_labels = store.relay_agent_labels()?;
    let workspace_ids = registered
        .iter()
        .map(|entry| (entry.workspace.id.to_string(), entry.public_id.clone()))
        .collect::<HashMap<_, _>>();
    let public_workspace = |local_id: &str| workspace_ids.get(local_id).cloned();
    let session_ids = local
        .sessions
        .iter()
        .filter(|entry| public_workspace(&entry.workspace_id).is_some())
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    let agent_ids = local
        .agents
        .iter()
        .filter(|entry| {
            public_workspace(&entry.workspace_id).is_some()
                && session_ids.contains(&entry.session_id)
        })
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    let agents = local
        .agents
        .iter()
        .filter(|entry| agent_ids.contains(&entry.id))
        .filter_map(|entry| {
            public_workspace(&entry.workspace_id).map(|workspace_id| RemoteAgent {
                id: entry.id.clone(),
                label: agent_labels.get(&entry.id).cloned(),
                session_id: entry.session_id.clone(),
                workspace_id,
                parent_id: entry
                    .parent_id
                    .as_ref()
                    .filter(|id| agent_ids.contains(*id))
                    .cloned(),
                kind: entry.kind.to_string(),
                state: entry.state.to_string(),
                started_at: entry.started_at,
                elapsed_ms: entry.elapsed_ms,
                last_activity_at: entry.last_activity_at,
                contradiction_count: entry.contradiction_count,
            })
        })
        .collect::<Vec<_>>();
    let totals = remote_totals(&agents);

    Ok(SnapshotEnvelope {
        schema_version: 1,
        instance_id: instance_id.to_string(),
        source_revision: local.revision,
        captured_at,
        snapshot: RemoteSnapshot {
            generated_at: local.generated_at,
            workspaces: local
                .workspaces
                .iter()
                .filter_map(|entry| {
                    public_workspace(&entry.id).map(|id| RemoteWorkspace {
                        id,
                        session_count: entry.session_count,
                        active_agents: entry.active_agents,
                    })
                })
                .collect(),
            sessions: local
                .sessions
                .iter()
                .filter(|entry| session_ids.contains(&entry.id))
                .filter_map(|entry| {
                    public_workspace(&entry.workspace_id).map(|workspace_id| RemoteSession {
                        id: entry.id.clone(),
                        label: session_labels.get(&entry.id).cloned(),
                        workspace_id,
                        state: entry.state.to_string(),
                        created_at: entry.created_at,
                    })
                })
                .collect(),
            agents,
            activity: local
                .activity
                .iter()
                .filter(|entry| agent_ids.contains(&entry.agent_id))
                .filter_map(|entry| {
                    public_workspace(&entry.workspace_id).map(|workspace_id| RemoteActivity {
                        id: entry.id,
                        workspace_id,
                        agent_id: entry.agent_id.clone(),
                        kind: entry.kind.to_string(),
                        timestamp: entry.timestamp,
                    })
                })
                .take(250)
                .collect(),
            capacity: RemoteCapacity {
                used: local.capacity.used,
                reserved: local.capacity.reserved,
                max: local.capacity.max,
                level: local.capacity.level.to_string(),
                fraction: local.capacity.fraction,
                at_cap: local.capacity.at_cap,
            },
            totals,
            catalog: RemoteCatalog {
                workspaces: registered
                    .into_iter()
                    .map(|entry| RemoteCatalogEntry {
                        id: entry.public_id,
                        label: entry.display_name,
                    })
                    .collect(),
                profiles: providers
                    .into_iter()
                    .map(|entry| RemoteCatalogEntry {
                        id: entry.public_id,
                        label: entry.display_name,
                    })
                    .collect(),
            },
        },
    })
}

fn remote_totals(agents: &[RemoteAgent]) -> RemoteTotals {
    RemoteTotals {
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
            .filter(|agent| matches!(agent.state.as_str(), "failed" | "interrupted"))
            .count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::read_model::{
        ActivityItem, AgentNode, CapacityNode, SessionNode, TotalsNode, WorkspaceNode,
    };
    use crate::orchestration::store::OrchestrationStore;
    use std::path::Path;

    #[test]
    fn projection_drops_private_paths_tasks_status_facts_and_git_metadata() {
        let sentinel = "SENTINEL-PRIVATE-CONTENT";
        let store = Arc::new(OrchestrationStore::in_memory().unwrap());
        let workspace = store
            .get_or_create_workspace("private", Path::new("C:/private"), 1)
            .unwrap();
        let hidden_workspace = store
            .get_or_create_workspace("hidden", Path::new("C:/hidden"), 1)
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
        let local = ControlPlaneSnapshot {
            revision: 7,
            generated_at: 10,
            workspaces: vec![
                WorkspaceNode {
                    id: workspace.id.to_string(),
                    name: sentinel.to_string(),
                    repo_root: format!("C:/{sentinel}"),
                    session_count: 1,
                    active_agents: 1,
                },
                WorkspaceNode {
                    id: hidden_workspace.id.to_string(),
                    name: sentinel.to_string(),
                    repo_root: format!("C:/hidden/{sentinel}"),
                    session_count: 1,
                    active_agents: 0,
                },
            ],
            sessions: vec![
                SessionNode {
                    id: "session-1".to_string(),
                    workspace_id: workspace.id.to_string(),
                    name: sentinel.to_string(),
                    base_commit: sentinel.to_string(),
                    state: "active",
                    created_at: 1,
                },
                SessionNode {
                    id: "session-hidden".to_string(),
                    workspace_id: hidden_workspace.id.to_string(),
                    name: sentinel.to_string(),
                    base_commit: sentinel.to_string(),
                    state: "failed",
                    created_at: 1,
                },
            ],
            agents: vec![
                AgentNode {
                    id: "agent-1".to_string(),
                    session_id: "session-1".to_string(),
                    workspace_id: workspace.id.to_string(),
                    parent_id: None,
                    name: sentinel.to_string(),
                    task: sentinel.to_string(),
                    kind: "writer",
                    state: "running",
                    status: sentinel.to_string(),
                    branch: sentinel.to_string(),
                    worktree: Some(sentinel.to_string()),
                    base_commit: sentinel.to_string(),
                    base_drift: false,
                    started_at: Some(1),
                    elapsed_ms: 2,
                    last_activity_at: Some(3),
                    contradiction_count: 0,
                },
                AgentNode {
                    id: "agent-hidden".to_string(),
                    session_id: "session-hidden".to_string(),
                    workspace_id: hidden_workspace.id.to_string(),
                    parent_id: None,
                    name: sentinel.to_string(),
                    task: sentinel.to_string(),
                    kind: "writer",
                    state: "failed",
                    status: sentinel.to_string(),
                    branch: sentinel.to_string(),
                    worktree: Some(sentinel.to_string()),
                    base_commit: sentinel.to_string(),
                    base_drift: false,
                    started_at: Some(1),
                    elapsed_ms: 2,
                    last_activity_at: Some(3),
                    contradiction_count: 0,
                },
            ],
            activity: vec![ActivityItem {
                id: 1,
                workspace_id: workspace.id.to_string(),
                agent_id: "agent-1".to_string(),
                kind: "status",
                fact: sentinel.to_string(),
                file_key: Some(sentinel.to_string()),
                branch: Some(sentinel.to_string()),
                trust: "trusted",
                timestamp: 3,
                has_provenance: true,
            }],
            contradictions: Vec::new(),
            capacity: CapacityNode {
                used: 1,
                reserved: 0,
                max: 8,
                level: "calm",
                fraction: 0.125,
                at_cap: false,
            },
            totals: TotalsNode {
                running: 1,
                stalled: 0,
                waiting: 0,
                done: 0,
                failed: 1,
            },
        };
        let remote = remote_snapshot(&local, &store, "instance", 11).unwrap();
        assert_eq!(1, remote.snapshot.agents.len());
        assert_eq!(1, remote.snapshot.totals.running);
        assert_eq!(0, remote.snapshot.totals.failed);
        let json = serde_json::to_string(&remote).unwrap();
        assert!(!json.contains(sentinel), "private sentinel escaped: {json}");
        assert!(!json.contains("repoRoot"));
        assert!(!json.contains("baseCommit"));
    }
}
