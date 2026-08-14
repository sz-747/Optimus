use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

id_type!(WorkspaceKey, "ws-");
id_type!(SessionId, "session-");
id_type!(AgentId, "agent-");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Active,
    Completed,
    Failed,
}

impl SessionState {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "active" => Self::Active,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Writer,
    Recon,
}

impl AgentKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Writer => "writer",
            Self::Recon => "recon",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "writer" => Self::Writer,
            "recon" => Self::Recon,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Planned,
    Ready,
    Running,
    Stopping,
    Done,
    Failed,
    Interrupted,
}

impl AgentState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "planned" => Self::Planned,
            "ready" => Self::Ready,
            "running" => Self::Running,
            "stopping" => Self::Stopping,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Interrupted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileScope {
    pub pattern: String,
}

impl FileScope {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceKey,
    pub name: String,
    pub repo_root: PathBuf,
    pub created_ts: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub workspace_id: WorkspaceKey,
    pub name: String,
    pub repo_root: PathBuf,
    pub base_commit: String,
    pub state: SessionState,
    pub created_ts: i64,
    pub ended_ts: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub agent_key: String,
    pub session_id: SessionId,
    pub workspace_id: WorkspaceKey,
    pub parent_id: Option<AgentId>,
    pub surface_id: Option<i32>,
    pub name: String,
    pub task: String,
    pub command: String,
    pub kind: AgentKind,
    pub state: AgentState,
    pub branch: String,
    pub base_commit: String,
    pub worktree_path: Option<PathBuf>,
    pub file_scope: Vec<FileScope>,
    pub created_ts: i64,
    pub started_ts: Option<i64>,
    pub ended_ts: Option<i64>,
    pub last_record_ts: Option<i64>,
    pub last_output_ts: Option<i64>,
    pub exit_code: Option<i32>,
    pub last_error: Option<String>,
    pub spawn_record_id: Option<i64>,
}
