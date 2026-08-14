use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayWorkspaceConfig {
    pub id: String,
    pub label: String,
    pub repo_root: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayProviderConfig {
    pub id: String,
    pub label: String,
    pub command: String,
    pub auth_mode: String,
    pub credential_env: Option<String>,
}

#[derive(Clone)]
pub struct RelayConfig {
    pub base_url: String,
    pub desktop_token: String,
    pub device_id: String,
    pub instance_id: String,
    pub consumer_id: String,
    pub workspaces: Vec<RelayWorkspaceConfig>,
    pub providers: Vec<RelayProviderConfig>,
}

impl std::fmt::Debug for RelayConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayConfig")
            .field("base_url", &self.base_url)
            .field("desktop_token", &"[redacted]")
            .field("device_id", &self.device_id)
            .field("instance_id", &self.instance_id)
            .field("consumer_id", &self.consumer_id)
            .field("workspaces", &self.workspaces)
            .field("providers", &self.providers)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayCommandKind {
    StartRun,
    StopAgent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRunPayload {
    pub workspace_id: String,
    pub profile_id: String,
    pub session_name: String,
    pub agents: Vec<StartAgentPayload>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartAgentPayload {
    pub agent_name: String,
    pub task: String,
    pub kind: String,
    pub file_scope: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StopAgentPayload {
    pub agent_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RelayCommandPayload {
    Start(StartRunPayload),
    Stop(StopAgentPayload),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayCommand {
    pub command_id: String,
    pub device_id: String,
    #[serde(rename = "type")]
    pub kind: RelayCommandKind,
    pub payload: serde_json::Value,
    pub status: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub lease_id: String,
    pub lease_owner: String,
    pub lease_until: i64,
    pub error_code: Option<String>,
    pub updated_at: i64,
}

impl RelayCommand {
    pub fn start_payload(&self) -> Result<StartRunPayload, serde_json::Error> {
        serde_json::from_value(self.payload.clone())
    }

    pub fn stop_payload(&self) -> Result<StopAgentPayload, serde_json::Error> {
        serde_json::from_value(self.payload.clone())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayAck<'a> {
    pub command_id: &'a str,
    pub lease_id: &'a str,
    pub status: &'a str,
    pub error_code: Option<&'a str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEnvelope {
    pub schema_version: u8,
    pub instance_id: String,
    pub source_revision: u64,
    pub captured_at: i64,
    pub snapshot: RemoteSnapshot,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSnapshot {
    pub generated_at: i64,
    pub workspaces: Vec<RemoteWorkspace>,
    pub sessions: Vec<RemoteSession>,
    pub agents: Vec<RemoteAgent>,
    pub activity: Vec<RemoteActivity>,
    pub capacity: RemoteCapacity,
    pub totals: RemoteTotals,
    pub catalog: RemoteCatalog,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkspace {
    pub id: String,
    pub session_count: usize,
    pub active_agents: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSession {
    pub id: String,
    pub label: Option<String>,
    pub workspace_id: String,
    pub state: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAgent {
    pub id: String,
    pub label: Option<String>,
    pub session_id: String,
    pub workspace_id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub state: String,
    pub started_at: Option<i64>,
    pub elapsed_ms: i64,
    pub last_activity_at: Option<i64>,
    pub contradiction_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteActivity {
    pub id: i64,
    pub workspace_id: String,
    pub agent_id: String,
    pub kind: String,
    pub timestamp: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCapacity {
    pub used: i32,
    pub reserved: i32,
    pub max: i32,
    pub level: String,
    pub fraction: f64,
    pub at_cap: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemoteTotals {
    pub running: usize,
    pub stalled: usize,
    pub waiting: usize,
    pub done: usize,
    pub failed: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemoteCatalog {
    pub workspaces: Vec<RemoteCatalogEntry>,
    pub profiles: Vec<RemoteCatalogEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RemoteCatalogEntry {
    pub id: String,
    pub label: String,
}
