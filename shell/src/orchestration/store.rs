use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::control_plane::memory::{AgentIdentity, AgentIdentityResolver, MemoryError};
use crate::domain::ids::SurfaceId;
use crate::orchestration::git::GitError;
use crate::orchestration::model::{
    Agent, AgentId, AgentKind, AgentState, FileScope, Session, SessionId, SessionState, Workspace,
    WorkspaceKey,
};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS control_workspace (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    repo_root  TEXT NOT NULL UNIQUE,
    created_ts INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS control_session (
    id           INTEGER PRIMARY KEY,
    workspace_id INTEGER NOT NULL REFERENCES control_workspace(id),
    name         TEXT NOT NULL,
    base_commit  TEXT NOT NULL,
    state        TEXT NOT NULL CHECK(state IN ('active','completed','failed')),
    created_ts   INTEGER NOT NULL,
    ended_ts     INTEGER
);
CREATE TABLE IF NOT EXISTS control_agent (
    id              INTEGER PRIMARY KEY,
    session_id      INTEGER NOT NULL REFERENCES control_session(id),
    parent_id       INTEGER REFERENCES control_agent(id),
    surface_id      INTEGER UNIQUE,
    agent_key       TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    task            TEXT NOT NULL,
    command         TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK(kind IN ('writer','recon')),
    state           TEXT NOT NULL CHECK(state IN ('planned','ready','running','stopping','done','failed','interrupted')),
    branch          TEXT NOT NULL,
    base_commit     TEXT,
    worktree_path   TEXT,
    created_ts      INTEGER NOT NULL,
    started_ts      INTEGER,
    ended_ts        INTEGER,
    last_record_ts  INTEGER,
    last_output_ts  INTEGER,
    exit_code       INTEGER,
    last_error      TEXT,
    spawn_record_id INTEGER
);
CREATE TABLE IF NOT EXISTS control_file_scope (
    agent_id INTEGER NOT NULL REFERENCES control_agent(id) ON DELETE CASCADE,
    pattern  TEXT NOT NULL,
    PRIMARY KEY(agent_id, pattern)
);
CREATE TABLE IF NOT EXISTS control_agent_credential (
    agent_id        INTEGER PRIMARY KEY REFERENCES control_agent(id) ON DELETE CASCADE,
    capability_hash BLOB NOT NULL,
    issued_ts       INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS control_git_capture_job (
    agent_id    INTEGER PRIMARY KEY REFERENCES control_agent(id) ON DELETE CASCADE,
    state       TEXT NOT NULL CHECK(state IN ('pending','succeeded','failed')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,
    created_ts  INTEGER NOT NULL,
    updated_ts  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS relay_workspace (
    public_id    TEXT PRIMARY KEY,
    workspace_id INTEGER NOT NULL UNIQUE REFERENCES control_workspace(id),
    display_name TEXT NOT NULL,
    enabled      INTEGER NOT NULL CHECK(enabled IN (0, 1))
);
CREATE TABLE IF NOT EXISTS relay_provider (
    public_id      TEXT PRIMARY KEY,
    display_name   TEXT NOT NULL,
    command_prefix TEXT NOT NULL,
    argument_mode  TEXT NOT NULL CHECK(argument_mode IN ('prompt_arg')),
    auth_mode      TEXT NOT NULL CHECK(auth_mode IN ('subscription','api')),
    enabled        INTEGER NOT NULL CHECK(enabled IN (0, 1))
);
CREATE TABLE IF NOT EXISTS relay_provider_credential (
    public_id      TEXT PRIMARY KEY REFERENCES relay_provider(public_id) ON DELETE CASCADE,
    credential_env TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS control_agent_model_auth (
    agent_id       INTEGER PRIMARY KEY REFERENCES control_agent(id) ON DELETE CASCADE,
    auth_mode      TEXT NOT NULL CHECK(auth_mode IN ('subscription','api')),
    credential_env TEXT
);
CREATE TABLE IF NOT EXISTS relay_session_label (
    session_id   INTEGER PRIMARY KEY REFERENCES control_session(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS relay_agent_label (
    agent_id     INTEGER PRIMARY KEY REFERENCES control_agent(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS relay_command (
    command_id   TEXT PRIMARY KEY,
    payload_hash BLOB NOT NULL,
    kind         TEXT NOT NULL CHECK(kind IN ('start_run','stop_agent')),
    state        TEXT NOT NULL CHECK(state IN ('executing','succeeded','failed','rejected')),
    session_id   INTEGER REFERENCES control_session(id),
    agent_id     INTEGER REFERENCES control_agent(id),
    error_code   TEXT,
    received_ts  INTEGER NOT NULL,
    finished_ts  INTEGER
);
CREATE INDEX IF NOT EXISTS control_agent_session ON control_agent(session_id, id);
CREATE INDEX IF NOT EXISTS control_agent_parent ON control_agent(parent_id);
CREATE INDEX IF NOT EXISTS control_agent_state ON control_agent(state, id);
CREATE INDEX IF NOT EXISTS control_git_capture_pending
    ON control_git_capture_job(state, updated_ts, agent_id);
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitCaptureJob {
    pub agent_id: AgentId,
    pub state: String,
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[derive(Debug)]
pub enum OrchestrationError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Git(GitError),
    Memory(MemoryError),
    Runtime(String),
    Invariant(String),
}

impl fmt::Display for OrchestrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "orchestration database: {error}"),
            Self::Io(error) => write!(f, "orchestration filesystem: {error}"),
            Self::Git(error) => error.fmt(f),
            Self::Memory(error) => error.fmt(f),
            Self::Runtime(error) => write!(f, "agent runtime: {error}"),
            Self::Invariant(error) => write!(f, "orchestration invariant: {error}"),
        }
    }
}

impl std::error::Error for OrchestrationError {}

impl From<rusqlite::Error> for OrchestrationError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

impl From<GitError> for OrchestrationError {
    fn from(value: GitError) -> Self {
        Self::Git(value)
    }
}

impl From<MemoryError> for OrchestrationError {
    fn from(value: MemoryError) -> Self {
        Self::Memory(value)
    }
}

pub struct OrchestrationStore {
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayWorkspace {
    pub public_id: String,
    pub display_name: String,
    pub workspace: Workspace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayProvider {
    pub public_id: String,
    pub display_name: String,
    pub command_prefix: String,
    pub argument_mode: String,
    pub auth_mode: String,
    pub credential_env: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentModelAuth {
    pub auth_mode: String,
    pub credential_env: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayCommandState {
    pub command_id: String,
    pub state: String,
    pub session_id: Option<SessionId>,
    pub agent_id: Option<AgentId>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelayCommandClaim {
    Fresh,
    Existing(RelayCommandState),
}

impl OrchestrationStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OrchestrationError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn in_memory() -> Result<Self, OrchestrationError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, OrchestrationError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;",
        )?;
        connection.execute_batch(SCHEMA)?;
        migrate_orchestration_schema(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn get_or_create_workspace(
        &self,
        name: &str,
        repo_root: &Path,
        ts: i64,
    ) -> Result<Workspace, OrchestrationError> {
        let repo_root = repo_root.to_string_lossy();
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "INSERT OR IGNORE INTO control_workspace(name, repo_root, created_ts) VALUES (?1, ?2, ?3)",
            params![name, repo_root, ts],
        )?;
        connection
            .query_row(
                "SELECT id, name, repo_root, created_ts FROM control_workspace WHERE repo_root = ?1",
                [repo_root.as_ref()],
                map_workspace,
            )
            .map_err(OrchestrationError::from)
    }

    pub fn create_session(
        &self,
        workspace: WorkspaceKey,
        name: &str,
        base_commit: &str,
        ts: i64,
    ) -> Result<Session, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "INSERT INTO control_session(workspace_id, name, base_commit, state, created_ts)
             VALUES (?1, ?2, ?3, 'active', ?4)",
            params![workspace.0, name, base_commit, ts],
        )?;
        let id = SessionId(connection.last_insert_rowid());
        drop(connection);
        self.session(id)?.ok_or_else(|| {
            OrchestrationError::Invariant("inserted session was not readable".to_string())
        })
    }

    pub fn session(&self, id: SessionId) -> Result<Option<Session>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .query_row(
                "SELECT s.id, s.workspace_id, s.name, w.repo_root, s.base_commit, s.state,
                        s.created_ts, s.ended_ts
                 FROM control_session s JOIN control_workspace w ON w.id = s.workspace_id
                 WHERE s.id = ?1",
                [id.0],
                map_session,
            )
            .optional()
            .map_err(OrchestrationError::from)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn plan_agent(
        &self,
        session: SessionId,
        parent: Option<AgentId>,
        name: &str,
        task: &str,
        command: &str,
        branch: &str,
        kind: AgentKind,
        file_scope: &[FileScope],
        ts: i64,
    ) -> Result<Agent, OrchestrationError> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE control_session SET state = 'active', ended_ts = NULL WHERE id = ?1",
            [session.0],
        )?;
        let inserted = transaction.execute(
            "INSERT INTO control_agent(
                session_id, parent_id, agent_key, name, task, command, kind, state,
                branch, base_commit, created_ts
             )
             SELECT ?1, ?2, '', ?3, ?4, ?5, ?6, 'planned', ?7, base_commit, ?8
             FROM control_session WHERE id = ?1",
            params![
                session.0,
                parent.map(|id| id.0),
                name,
                task,
                command,
                kind.as_str(),
                branch,
                ts
            ],
        )?;
        if inserted != 1 {
            return Err(OrchestrationError::Invariant(format!(
                "unknown session {session}"
            )));
        }
        let id = AgentId(transaction.last_insert_rowid());
        let agent_key = id.to_string();
        transaction.execute(
            "UPDATE control_agent SET agent_key = ?1 WHERE id = ?2",
            params![agent_key, id.0],
        )?;
        for scope in file_scope {
            let pattern = normalize_scope(&scope.pattern)?;
            transaction.execute(
                "INSERT OR IGNORE INTO control_file_scope(agent_id, pattern) VALUES (?1, ?2)",
                params![id.0, pattern],
            )?;
        }
        transaction.commit()?;
        drop(connection);
        self.agent(id)?.ok_or_else(|| {
            OrchestrationError::Invariant("inserted agent was not readable".to_string())
        })
    }

    pub fn set_worktree_path(&self, id: AgentId, path: &Path) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "UPDATE control_agent SET worktree_path = ?1 WHERE id = ?2 AND state = 'planned'",
            params![path.to_string_lossy(), id.0],
        )?;
        Ok(())
    }

    pub fn clear_worktree_path(&self, id: AgentId) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "UPDATE control_agent SET worktree_path = NULL WHERE id = ?1",
            [id.0],
        )?;
        Ok(())
    }

    pub fn cancel_git_capture(&self, id: AgentId) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "DELETE FROM control_git_capture_job WHERE agent_id = ?1",
            [id.0],
        )?;
        Ok(())
    }

    pub fn mark_ready(&self, id: AgentId, ts: i64) -> Result<Agent, OrchestrationError> {
        self.transition(id, AgentState::Planned, AgentState::Ready, ts, None, None)
    }

    pub fn mark_running(
        &self,
        id: AgentId,
        surface: SurfaceId,
        ts: i64,
    ) -> Result<Agent, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE control_agent SET state = 'running', surface_id = ?1, started_ts = ?2,
                    last_output_ts = ?2, last_error = NULL
             WHERE id = ?3 AND state = 'ready'",
            params![surface.0, ts, id.0],
        )?;
        drop(connection);
        self.changed_agent(id, changed, "ready -> running")
    }

    pub fn set_agent_capability(
        &self,
        id: AgentId,
        capability: &str,
        ts: i64,
    ) -> Result<(), OrchestrationError> {
        let digest = Sha256::digest(capability.as_bytes());
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "INSERT INTO control_agent_credential(agent_id, capability_hash, issued_ts)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(agent_id) DO UPDATE SET
                capability_hash = excluded.capability_hash,
                issued_ts = excluded.issued_ts",
            params![id.0, digest.as_slice(), ts],
        )?;
        Ok(())
    }

    pub fn verify_agent_capability(
        &self,
        surface: SurfaceId,
        capability: &str,
    ) -> Result<bool, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let expected = connection
            .query_row(
                "SELECT c.capability_hash
                 FROM control_agent_credential c
                 JOIN control_agent a ON a.id = c.agent_id
                 WHERE a.surface_id = ?1 AND a.state IN ('running','stopping')",
                [surface.0],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        let Some(expected) = expected else {
            return Ok(false);
        };
        let actual = Sha256::digest(capability.as_bytes());
        Ok(expected.len() == actual.len()
            && bool::from(expected.as_slice().ct_eq(actual.as_slice())))
    }

    pub fn mark_stopping(&self, id: AgentId, ts: i64) -> Result<Agent, OrchestrationError> {
        self.transition(
            id,
            AgentState::Running,
            AgentState::Stopping,
            ts,
            None,
            None,
        )
    }

    pub fn mark_done(
        &self,
        id: AgentId,
        exit_code: i32,
        ts: i64,
    ) -> Result<Agent, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE control_agent SET state = 'done', ended_ts = ?1, exit_code = ?2
             WHERE id = ?3 AND state IN ('running','stopping')",
            params![ts, exit_code, id.0],
        )?;
        drop(connection);
        let agent = self.changed_agent(id, changed, "running/stopping -> done")?;
        self.refresh_session_state(agent.session_id, ts)?;
        Ok(agent)
    }

    pub fn mark_exited(
        &self,
        id: AgentId,
        exit_code: i32,
        ts: i64,
    ) -> Result<Agent, OrchestrationError> {
        if exit_code == 0 {
            return self.mark_done(id, exit_code, ts);
        }
        let message = format!("Agent process exited with code {exit_code}");
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE control_agent SET state = 'failed', ended_ts = ?1, exit_code = ?2,
                    last_error = ?3
             WHERE id = ?4 AND state IN ('running','stopping')",
            params![ts, exit_code, message, id.0],
        )?;
        drop(connection);
        let agent = self.changed_agent(id, changed, "running/stopping -> failed exit")?;
        self.refresh_session_state(agent.session_id, ts)?;
        Ok(agent)
    }

    pub fn mark_interrupted(
        &self,
        id: AgentId,
        message: &str,
        ts: i64,
    ) -> Result<Agent, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE control_agent SET state = 'interrupted', ended_ts = ?1, surface_id = NULL,
                    last_error = ?2
             WHERE id = ?3 AND state IN ('running','stopping')",
            params![ts, message, id.0],
        )?;
        drop(connection);
        let agent = self.changed_agent(id, changed, "running/stopping -> interrupted")?;
        self.refresh_session_state(agent.session_id, ts)?;
        Ok(agent)
    }

    pub fn mark_failed(
        &self,
        id: AgentId,
        message: &str,
        ts: i64,
    ) -> Result<Agent, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE control_agent SET state = 'failed', ended_ts = ?1, last_error = ?2
             WHERE id = ?3 AND state IN ('planned','ready','running','stopping')",
            params![ts, message, id.0],
        )?;
        drop(connection);
        let agent = self.changed_agent(id, changed, "non-terminal -> failed")?;
        self.refresh_session_state(agent.session_id, ts)?;
        Ok(agent)
    }

    pub fn touch_output(&self, id: AgentId, ts: i64) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "UPDATE control_agent SET last_output_ts = MAX(COALESCE(last_output_ts, 0), ?1)
             WHERE id = ?2 AND state = 'running'",
            params![ts, id.0],
        )?;
        Ok(())
    }

    pub fn touch_record(&self, id: AgentId, ts: i64) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "UPDATE control_agent SET last_record_ts = MAX(COALESCE(last_record_ts, 0), ?1)
             WHERE id = ?2",
            params![ts, id.0],
        )?;
        Ok(())
    }

    pub fn enqueue_git_capture(&self, id: AgentId, ts: i64) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "INSERT OR IGNORE INTO control_git_capture_job(
                agent_id, state, attempts, created_ts, updated_ts
             )
             SELECT id, 'pending', 0, ?1, ?1 FROM control_agent
             WHERE id = ?2 AND worktree_path IS NOT NULL",
            params![ts, id.0],
        )?;
        if changed == 0 {
            let exists = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM control_agent WHERE id = ?1)",
                [id.0],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(OrchestrationError::Invariant(format!(
                    "unknown capture agent {id}"
                )));
            }
        }
        Ok(())
    }

    pub fn enqueue_missing_terminal_git_captures(
        &self,
        ts: i64,
    ) -> Result<usize, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .execute(
                "INSERT OR IGNORE INTO control_git_capture_job(
                    agent_id, state, attempts, created_ts, updated_ts
                 )
                 SELECT id, 'pending', 0, ?1, ?1 FROM control_agent
                 WHERE worktree_path IS NOT NULL
                   AND state IN ('done','failed','interrupted')",
                [ts],
            )
            .map_err(OrchestrationError::from)
    }

    pub fn retry_failed_git_captures(&self, ts: i64) -> Result<usize, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .execute(
                "UPDATE control_git_capture_job
                 SET state = 'pending', attempts = 0, last_error = NULL, updated_ts = ?1
                 WHERE state = 'failed'",
                [ts],
            )
            .map_err(OrchestrationError::from)
    }

    pub fn pending_git_captures(&self, limit: usize) -> Result<Vec<AgentId>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection.prepare(
            "SELECT agent_id FROM control_git_capture_job
             WHERE state = 'pending' AND attempts < 2
             ORDER BY updated_ts, agent_id LIMIT ?1",
        )?;
        let rows = statement.query_map([limit.clamp(1, 500) as i64], |row| {
            row.get::<_, i64>(0).map(AgentId)
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(OrchestrationError::from)
    }

    pub fn mark_git_capture_succeeded(
        &self,
        id: AgentId,
        ts: i64,
    ) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE control_git_capture_job
             SET state = 'succeeded', last_error = NULL, updated_ts = ?1
             WHERE agent_id = ?2",
            params![ts, id.0],
        )?;
        if changed != 1 {
            return Err(OrchestrationError::Invariant(format!(
                "missing capture job for {id}"
            )));
        }
        Ok(())
    }

    pub fn mark_git_capture_failed(
        &self,
        id: AgentId,
        error: &str,
        ts: i64,
    ) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE control_git_capture_job
             SET state = CASE WHEN attempts + 1 >= 2 THEN 'failed' ELSE 'pending' END,
                 attempts = attempts + 1,
                 last_error = ?1, updated_ts = ?2
             WHERE agent_id = ?3",
            params![error, ts, id.0],
        )?;
        if changed != 1 {
            return Err(OrchestrationError::Invariant(format!(
                "missing capture job for {id}"
            )));
        }
        Ok(())
    }

    pub fn git_capture_job(
        &self,
        id: AgentId,
    ) -> Result<Option<GitCaptureJob>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .query_row(
                "SELECT agent_id, state, attempts, last_error
                 FROM control_git_capture_job WHERE agent_id = ?1",
                [id.0],
                |row| {
                    Ok(GitCaptureJob {
                        agent_id: AgentId(row.get(0)?),
                        state: row.get(1)?,
                        attempts: row.get(2)?,
                        last_error: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(OrchestrationError::from)
    }

    pub fn set_spawn_record(&self, id: AgentId, record_id: i64) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE control_agent SET spawn_record_id = ?1
             WHERE id = ?2 AND spawn_record_id IS NULL",
            params![record_id, id.0],
        )?;
        if changed == 0 {
            return Err(OrchestrationError::Invariant(format!(
                "agent {id} already has a spawn record"
            )));
        }
        Ok(())
    }

    pub fn recover_interrupted(&self, ts: i64) -> Result<usize, OrchestrationError> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE control_agent SET state = 'interrupted', ended_ts = ?1, surface_id = NULL,
                        last_error = 'Optimus restarted while the agent was active'
                 WHERE state IN ('running','stopping')",
            [ts],
        )?;
        transaction.execute(
            "UPDATE control_session SET state = 'failed', ended_ts = ?1
             WHERE state = 'active'
               AND EXISTS (SELECT 1 FROM control_agent a WHERE a.session_id = control_session.id)
               AND NOT EXISTS (
                   SELECT 1 FROM control_agent a WHERE a.session_id = control_session.id
                   AND a.state IN ('planned','ready','running','stopping')
               )",
            [ts],
        )?;
        transaction.commit()?;
        Ok(changed)
    }

    pub fn agent(&self, id: AgentId) -> Result<Option<Agent>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        load_agent(&connection, "a.id = ?1", id.0)
    }

    pub fn agent_by_surface(
        &self,
        surface: SurfaceId,
    ) -> Result<Option<Agent>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        load_agent(&connection, "a.surface_id = ?1", i64::from(surface.0))
    }

    pub fn agents_for_session(&self, id: SessionId) -> Result<Vec<Agent>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection.prepare(
            "SELECT a.id, a.agent_key, a.session_id, s.workspace_id, a.parent_id, a.surface_id,
                    a.name, a.task, a.command, a.kind, a.state, a.branch,
                    COALESCE(a.base_commit, s.base_commit), a.worktree_path,
                    a.created_ts, a.started_ts, a.ended_ts, a.last_record_ts, a.last_output_ts,
                    a.exit_code, a.last_error, a.spawn_record_id
             FROM control_agent a JOIN control_session s ON s.id = a.session_id
             WHERE a.session_id = ?1 ORDER BY a.id",
        )?;
        let rows = statement.query_map([id.0], |row| map_agent(row, &connection))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(OrchestrationError::from)
    }

    pub fn workspaces(&self) -> Result<Vec<Workspace>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection
            .prepare("SELECT id, name, repo_root, created_ts FROM control_workspace ORDER BY id")?;
        let rows = statement.query_map([], map_workspace)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(OrchestrationError::from)
    }

    pub fn sessions(&self) -> Result<Vec<Session>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection.prepare(
            "SELECT s.id, s.workspace_id, s.name, w.repo_root, s.base_commit, s.state,
                    s.created_ts, s.ended_ts
             FROM control_session s JOIN control_workspace w ON w.id = s.workspace_id
             ORDER BY s.id",
        )?;
        let rows = statement.query_map([], map_session)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(OrchestrationError::from)
    }

    pub fn agents(&self) -> Result<Vec<Agent>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection.prepare(
            "SELECT a.id, a.agent_key, a.session_id, s.workspace_id, a.parent_id, a.surface_id,
                    a.name, a.task, a.command, a.kind, a.state, a.branch,
                    COALESCE(a.base_commit, s.base_commit),
                    a.worktree_path, a.created_ts, a.started_ts, a.ended_ts, a.last_record_ts,
                    a.last_output_ts, a.exit_code, a.last_error, a.spawn_record_id
             FROM control_agent a JOIN control_session s ON s.id = a.session_id
             ORDER BY a.id",
        )?;
        let rows = statement.query_map([], |row| map_agent(row, &connection))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(OrchestrationError::from)
    }

    pub fn register_relay_workspace(
        &self,
        public_id: &str,
        workspace_id: WorkspaceKey,
        display_name: &str,
    ) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "INSERT INTO relay_workspace(public_id, workspace_id, display_name, enabled)
             VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(public_id) DO UPDATE SET
                workspace_id = excluded.workspace_id,
                display_name = excluded.display_name,
                enabled = 1",
            params![public_id, workspace_id.0, display_name],
        )?;
        Ok(())
    }

    pub fn disable_relay_registry(&self) -> Result<(), OrchestrationError> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let transaction = connection.transaction()?;
        transaction.execute("UPDATE relay_workspace SET enabled = 0", [])?;
        transaction.execute("UPDATE relay_provider SET enabled = 0", [])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn register_relay_provider(
        &self,
        public_id: &str,
        display_name: &str,
        command_prefix: &str,
        argument_mode: &str,
        auth_mode: &str,
        credential_env: Option<&str>,
    ) -> Result<(), OrchestrationError> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO relay_provider(
                public_id, display_name, command_prefix, argument_mode, auth_mode, enabled
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1)
             ON CONFLICT(public_id) DO UPDATE SET
                display_name = excluded.display_name,
                command_prefix = excluded.command_prefix,
                argument_mode = excluded.argument_mode,
                auth_mode = excluded.auth_mode,
                enabled = 1",
            params![
                public_id,
                display_name,
                command_prefix,
                argument_mode,
                auth_mode
            ],
        )?;
        transaction.execute(
            "DELETE FROM relay_provider_credential WHERE public_id = ?1",
            [public_id],
        )?;
        if let Some(credential_env) = credential_env {
            transaction.execute(
                "INSERT INTO relay_provider_credential(public_id, credential_env)
                 VALUES (?1, ?2)",
                params![public_id, credential_env],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn set_agent_model_auth(
        &self,
        id: AgentId,
        auth_mode: &str,
        credential_env: Option<&str>,
    ) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "INSERT INTO control_agent_model_auth(agent_id, auth_mode, credential_env)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(agent_id) DO UPDATE SET
                auth_mode = excluded.auth_mode,
                credential_env = excluded.credential_env",
            params![id.0, auth_mode, credential_env],
        )?;
        Ok(())
    }

    pub fn agent_model_auth(
        &self,
        id: AgentId,
    ) -> Result<Option<AgentModelAuth>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .query_row(
                "SELECT auth_mode, credential_env FROM control_agent_model_auth
                 WHERE agent_id = ?1",
                [id.0],
                |row| {
                    Ok(AgentModelAuth {
                        auth_mode: row.get(0)?,
                        credential_env: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(OrchestrationError::from)
    }

    pub fn set_relay_session_label(
        &self,
        id: SessionId,
        display_name: &str,
    ) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "INSERT INTO relay_session_label(session_id, display_name) VALUES (?1, ?2)
             ON CONFLICT(session_id) DO UPDATE SET display_name = excluded.display_name",
            params![id.0, display_name],
        )?;
        Ok(())
    }

    pub fn set_relay_agent_label(
        &self,
        id: AgentId,
        display_name: &str,
    ) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "INSERT INTO relay_agent_label(agent_id, display_name) VALUES (?1, ?2)
             ON CONFLICT(agent_id) DO UPDATE SET display_name = excluded.display_name",
            params![id.0, display_name],
        )?;
        Ok(())
    }

    pub fn relay_session_labels(&self) -> Result<HashMap<String, String>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection.prepare(
            "SELECT session_id, display_name FROM relay_session_label ORDER BY session_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((format!("session-{}", row.get::<_, i64>(0)?), row.get(1)?))
        })?;
        rows.collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(OrchestrationError::from)
    }

    pub fn relay_agent_labels(&self) -> Result<HashMap<String, String>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection
            .prepare("SELECT agent_id, display_name FROM relay_agent_label ORDER BY agent_id")?;
        let rows = statement.query_map([], |row| {
            Ok((format!("agent-{}", row.get::<_, i64>(0)?), row.get(1)?))
        })?;
        rows.collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(OrchestrationError::from)
    }

    pub fn relay_workspaces(&self) -> Result<Vec<RelayWorkspace>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection.prepare(
            "SELECT r.public_id, r.display_name, w.id, w.name, w.repo_root, w.created_ts
             FROM relay_workspace r JOIN control_workspace w ON w.id = r.workspace_id
             WHERE r.enabled = 1 ORDER BY r.public_id",
        )?;
        let rows = statement.query_map([], map_relay_workspace)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(OrchestrationError::from)
    }

    pub fn relay_workspace(
        &self,
        public_id: &str,
    ) -> Result<Option<RelayWorkspace>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .query_row(
                "SELECT r.public_id, r.display_name, w.id, w.name, w.repo_root, w.created_ts
                 FROM relay_workspace r JOIN control_workspace w ON w.id = r.workspace_id
                 WHERE r.enabled = 1 AND r.public_id = ?1",
                [public_id],
                map_relay_workspace,
            )
            .optional()
            .map_err(OrchestrationError::from)
    }

    pub fn relay_workspace_for_key(
        &self,
        workspace_id: WorkspaceKey,
    ) -> Result<Option<RelayWorkspace>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .query_row(
                "SELECT r.public_id, r.display_name, w.id, w.name, w.repo_root, w.created_ts
                 FROM relay_workspace r JOIN control_workspace w ON w.id = r.workspace_id
                 WHERE r.enabled = 1 AND r.workspace_id = ?1",
                [workspace_id.0],
                map_relay_workspace,
            )
            .optional()
            .map_err(OrchestrationError::from)
    }

    pub fn relay_providers(&self) -> Result<Vec<RelayProvider>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection.prepare(
            "SELECT p.public_id, p.display_name, p.command_prefix, p.argument_mode, p.auth_mode,
                    c.credential_env
             FROM relay_provider p LEFT JOIN relay_provider_credential c ON c.public_id = p.public_id
             WHERE p.enabled = 1 ORDER BY p.public_id",
        )?;
        let rows = statement.query_map([], map_relay_provider)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(OrchestrationError::from)
    }

    pub fn relay_provider(
        &self,
        public_id: &str,
    ) -> Result<Option<RelayProvider>, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .query_row(
                "SELECT p.public_id, p.display_name, p.command_prefix, p.argument_mode, p.auth_mode,
                        c.credential_env
                 FROM relay_provider p LEFT JOIN relay_provider_credential c ON c.public_id = p.public_id
                 WHERE p.enabled = 1 AND p.public_id = ?1",
                [public_id],
                map_relay_provider,
            )
            .optional()
            .map_err(OrchestrationError::from)
    }

    pub fn claim_relay_command(
        &self,
        command_id: &str,
        payload_hash: &[u8],
        kind: &str,
        ts: i64,
    ) -> Result<RelayCommandClaim, OrchestrationError> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let transaction = connection.transaction()?;
        let existing = transaction
            .query_row(
                "SELECT payload_hash, state, session_id, agent_id, error_code
                 FROM relay_command WHERE command_id = ?1",
                [command_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        RelayCommandState {
                            command_id: command_id.to_string(),
                            state: row.get(1)?,
                            session_id: row.get::<_, Option<i64>>(2)?.map(SessionId),
                            agent_id: row.get::<_, Option<i64>>(3)?.map(AgentId),
                            error_code: row.get(4)?,
                        },
                    ))
                },
            )
            .optional()?;
        if let Some((stored_hash, state)) = existing {
            if stored_hash != payload_hash {
                return Err(OrchestrationError::Invariant(
                    "relay command id was reused with different content".to_string(),
                ));
            }
            transaction.commit()?;
            return Ok(RelayCommandClaim::Existing(state));
        }
        transaction.execute(
            "INSERT INTO relay_command(command_id, payload_hash, kind, state, received_ts)
             VALUES (?1, ?2, ?3, 'executing', ?4)",
            params![command_id, payload_hash, kind, ts],
        )?;
        transaction.commit()?;
        Ok(RelayCommandClaim::Fresh)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_relay_command(
        &self,
        command_id: &str,
        state: &str,
        session_id: Option<SessionId>,
        agent_id: Option<AgentId>,
        error_code: Option<&str>,
        ts: i64,
    ) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE relay_command SET state = ?1,
                    session_id = COALESCE(?2, session_id),
                    agent_id = COALESCE(?3, agent_id),
                    error_code = ?4, finished_ts = ?5
             WHERE command_id = ?6 AND state = 'executing'",
            params![
                state,
                session_id.map(|value| value.0),
                agent_id.map(|value| value.0),
                error_code,
                ts,
                command_id
            ],
        )?;
        if changed == 0 {
            return Err(OrchestrationError::Invariant(
                "relay command was not executing".to_string(),
            ));
        }
        Ok(())
    }

    pub fn link_relay_command(
        &self,
        command_id: &str,
        session_id: Option<SessionId>,
        agent_id: Option<AgentId>,
    ) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let changed = connection.execute(
            "UPDATE relay_command SET
                session_id = COALESCE(?1, session_id),
                agent_id = COALESCE(?2, agent_id)
             WHERE command_id = ?3 AND state = 'executing'",
            params![
                session_id.map(|value| value.0),
                agent_id.map(|value| value.0),
                command_id
            ],
        )?;
        if changed == 0 {
            return Err(OrchestrationError::Invariant(
                "relay command was not executing".to_string(),
            ));
        }
        Ok(())
    }

    pub fn recover_relay_commands(&self, ts: i64) -> Result<usize, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .execute(
                "UPDATE relay_command SET state = 'failed', error_code = 'desktop_restarted',
                        finished_ts = ?1 WHERE state = 'executing'",
                [ts],
            )
            .map_err(OrchestrationError::from)
    }

    fn transition(
        &self,
        id: AgentId,
        from: AgentState,
        to: AgentState,
        ts: i64,
        exit_code: Option<i32>,
        error: Option<&str>,
    ) -> Result<Agent, OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let ended = to.is_terminal().then_some(ts);
        let changed = connection.execute(
            "UPDATE control_agent SET state = ?1, ended_ts = COALESCE(?2, ended_ts),
                    exit_code = COALESCE(?3, exit_code), last_error = COALESCE(?4, last_error)
             WHERE id = ?5 AND state = ?6",
            params![to.as_str(), ended, exit_code, error, id.0, from.as_str()],
        )?;
        drop(connection);
        self.changed_agent(
            id,
            changed,
            &format!("{} -> {}", from.as_str(), to.as_str()),
        )
    }

    fn changed_agent(
        &self,
        id: AgentId,
        changed: usize,
        transition: &str,
    ) -> Result<Agent, OrchestrationError> {
        if changed == 0 {
            return Err(OrchestrationError::Invariant(format!(
                "agent {id} rejected transition {transition}"
            )));
        }
        self.agent(id)?.ok_or_else(|| {
            OrchestrationError::Invariant("updated agent was not readable".to_string())
        })
    }

    fn refresh_session_state(
        &self,
        session_id: SessionId,
        ts: i64,
    ) -> Result<(), OrchestrationError> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection.execute(
            "UPDATE control_session SET
                state = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM control_agent a WHERE a.session_id = ?1
                        AND a.state IN ('planned','ready','running','stopping')
                    ) THEN 'active'
                    WHEN EXISTS (
                        SELECT 1 FROM control_agent a WHERE a.session_id = ?1
                        AND a.state IN ('failed','interrupted')
                    ) THEN 'failed'
                    ELSE 'completed'
                END,
                ended_ts = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM control_agent a WHERE a.session_id = ?1
                        AND a.state IN ('planned','ready','running','stopping')
                    ) THEN NULL ELSE ?2 END
             WHERE id = ?1",
            params![session_id.0, ts],
        )?;
        Ok(())
    }
}

impl AgentIdentityResolver for OrchestrationStore {
    fn resolve(&self, surface: SurfaceId) -> Option<AgentIdentity> {
        let connection = self.connection.lock().ok()?;
        connection
            .query_row(
                "SELECT a.agent_key, s.workspace_id, a.branch
                 FROM control_agent a JOIN control_session s ON s.id = a.session_id
                 WHERE a.surface_id = ?1 AND a.state IN ('running','stopping')",
                [surface.0],
                |row| {
                    Ok(AgentIdentity {
                        agent_id: row.get(0)?,
                        workspace_id: row.get(1)?,
                        branch: row.get(2)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }
}

fn load_agent(
    connection: &Connection,
    predicate: &str,
    value: i64,
) -> Result<Option<Agent>, OrchestrationError> {
    let sql = format!(
        "SELECT a.id, a.agent_key, a.session_id, s.workspace_id, a.parent_id, a.surface_id,
                a.name, a.task, a.command, a.kind, a.state, a.branch,
                COALESCE(a.base_commit, s.base_commit), a.worktree_path,
                a.created_ts, a.started_ts, a.ended_ts, a.last_record_ts, a.last_output_ts,
                a.exit_code, a.last_error, a.spawn_record_id
         FROM control_agent a JOIN control_session s ON s.id = a.session_id WHERE {predicate}"
    );
    connection
        .query_row(&sql, [value], |row| map_agent(row, connection))
        .optional()
        .map_err(OrchestrationError::from)
}

fn map_relay_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<RelayWorkspace> {
    Ok(RelayWorkspace {
        public_id: row.get(0)?,
        display_name: row.get(1)?,
        workspace: Workspace {
            id: WorkspaceKey(row.get(2)?),
            name: row.get(3)?,
            repo_root: PathBuf::from(row.get::<_, String>(4)?),
            created_ts: row.get(5)?,
        },
    })
}

fn map_relay_provider(row: &rusqlite::Row<'_>) -> rusqlite::Result<RelayProvider> {
    Ok(RelayProvider {
        public_id: row.get(0)?,
        display_name: row.get(1)?,
        command_prefix: row.get(2)?,
        argument_mode: row.get(3)?,
        auth_mode: row.get(4)?,
        credential_env: row.get(5)?,
    })
}

fn map_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: WorkspaceKey(row.get(0)?),
        name: row.get(1)?,
        repo_root: PathBuf::from(row.get::<_, String>(2)?),
        created_ts: row.get(3)?,
    })
}

fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    let state: String = row.get(5)?;
    Ok(Session {
        id: SessionId(row.get(0)?),
        workspace_id: WorkspaceKey(row.get(1)?),
        name: row.get(2)?,
        repo_root: PathBuf::from(row.get::<_, String>(3)?),
        base_commit: row.get(4)?,
        state: SessionState::parse(&state).ok_or(rusqlite::Error::InvalidQuery)?,
        created_ts: row.get(6)?,
        ended_ts: row.get(7)?,
    })
}

fn map_agent(row: &rusqlite::Row<'_>, connection: &Connection) -> rusqlite::Result<Agent> {
    let kind: String = row.get(9)?;
    let state: String = row.get(10)?;
    let id = AgentId(row.get(0)?);
    let mut scope_statement = connection
        .prepare("SELECT pattern FROM control_file_scope WHERE agent_id = ?1 ORDER BY pattern")?;
    let file_scope = scope_statement
        .query_map([id.0], |scope| {
            Ok(FileScope::new(scope.get::<_, String>(0)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Agent {
        id,
        agent_key: row.get(1)?,
        session_id: SessionId(row.get(2)?),
        workspace_id: WorkspaceKey(row.get(3)?),
        parent_id: row.get::<_, Option<i64>>(4)?.map(AgentId),
        surface_id: row.get(5)?,
        name: row.get(6)?,
        task: row.get(7)?,
        command: row.get(8)?,
        kind: AgentKind::parse(&kind).ok_or(rusqlite::Error::InvalidQuery)?,
        state: AgentState::parse(&state).ok_or(rusqlite::Error::InvalidQuery)?,
        branch: row.get(11)?,
        base_commit: row.get(12)?,
        worktree_path: row.get::<_, Option<String>>(13)?.map(PathBuf::from),
        file_scope,
        created_ts: row.get(14)?,
        started_ts: row.get(15)?,
        ended_ts: row.get(16)?,
        last_record_ts: row.get(17)?,
        last_output_ts: row.get(18)?,
        exit_code: row.get(19)?,
        last_error: row.get(20)?,
        spawn_record_id: row.get(21)?,
    })
}

fn migrate_orchestration_schema(connection: &Connection) -> Result<(), OrchestrationError> {
    let has_agent_base = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('control_agent') WHERE name = 'base_commit'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !has_agent_base {
        connection.execute_batch("ALTER TABLE control_agent ADD COLUMN base_commit TEXT;")?;
    }
    connection.execute(
        "UPDATE control_agent
         SET base_commit = (
             SELECT base_commit FROM control_session WHERE id = control_agent.session_id
         )
         WHERE base_commit IS NULL",
        [],
    )?;

    let capture_schema = connection.query_row(
        "SELECT sql FROM sqlite_master
         WHERE type = 'table' AND name = 'control_git_capture_job'",
        [],
        |row| row.get::<_, String>(0),
    )?;
    if !capture_schema.contains("'failed'") {
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE control_git_capture_job RENAME TO control_git_capture_job_legacy;
             CREATE TABLE control_git_capture_job (
                 agent_id    INTEGER PRIMARY KEY REFERENCES control_agent(id) ON DELETE CASCADE,
                 state       TEXT NOT NULL CHECK(state IN ('pending','succeeded','failed')),
                 attempts    INTEGER NOT NULL DEFAULT 0,
                 last_error  TEXT,
                 created_ts  INTEGER NOT NULL,
                 updated_ts  INTEGER NOT NULL
             );
             INSERT INTO control_git_capture_job(
                 agent_id, state, attempts, last_error, created_ts, updated_ts
             )
             SELECT agent_id,
                    CASE WHEN state = 'pending' AND attempts >= 2 THEN 'failed' ELSE state END,
                    attempts, last_error, created_ts, updated_ts
             FROM control_git_capture_job_legacy;
             DROP TABLE control_git_capture_job_legacy;
             CREATE INDEX control_git_capture_pending
                 ON control_git_capture_job(state, updated_ts, agent_id);
             COMMIT;",
        )?;
    }
    Ok(())
}

fn normalize_scope(value: &str) -> Result<String, OrchestrationError> {
    super::canonical_file_scope(value)
        .ok_or_else(|| OrchestrationError::Invariant(format!("unsafe file scope: {value}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(store: &OrchestrationStore) -> (Session, Agent) {
        let workspace = store
            .get_or_create_workspace("optimus", Path::new("C:/dev/Optimus"), 1)
            .unwrap();
        let session = store
            .create_session(workspace.id, "session", "abc123", 2)
            .unwrap();
        let agent = store
            .plan_agent(
                session.id,
                None,
                "writer",
                "task",
                "codex",
                "feat/writer",
                AgentKind::Writer,
                &[FileScope::new("shell/src")],
                3,
            )
            .unwrap();
        (session, agent)
    }

    #[test]
    fn lifecycle_transitions_are_checked_and_stop_is_idempotent_at_the_caller_boundary() {
        let store = OrchestrationStore::in_memory().unwrap();
        let (session, agent) = seed(&store);
        store.mark_ready(agent.id, 4).unwrap();
        store.mark_running(agent.id, SurfaceId(9), 5).unwrap();
        store
            .set_agent_capability(agent.id, "capability", 5)
            .unwrap();
        assert!(store
            .verify_agent_capability(SurfaceId(9), "capability")
            .unwrap());
        store.mark_stopping(agent.id, 6).unwrap();
        let done = store.mark_done(agent.id, 0, 7).unwrap();
        assert_eq!(AgentState::Done, done.state);
        assert_eq!(Some(0), done.exit_code);
        assert!(store.mark_done(agent.id, 0, 8).is_err());
        assert_eq!(
            Some(AgentState::Done),
            store.agent(agent.id).unwrap().map(|a| a.state)
        );
        assert!(!store
            .verify_agent_capability(SurfaceId(9), "capability")
            .unwrap());
        let completed = store.session(session.id).unwrap().unwrap();
        assert_eq!(SessionState::Completed, completed.state);
        assert_eq!(Some(7), completed.ended_ts);
    }

    #[test]
    fn active_agents_become_interrupted_on_restart_but_worktree_data_survives() {
        let store = OrchestrationStore::in_memory().unwrap();
        let (_session, agent) = seed(&store);
        store
            .set_worktree_path(agent.id, Path::new("C:/worktrees/a"))
            .unwrap();
        store.mark_ready(agent.id, 4).unwrap();
        store.mark_running(agent.id, SurfaceId(9), 5).unwrap();
        assert_eq!(1, store.recover_interrupted(10).unwrap());
        let recovered = store.agent(agent.id).unwrap().unwrap();
        assert_eq!(AgentState::Interrupted, recovered.state);
        assert_eq!(
            Some(Path::new("C:/worktrees/a")),
            recovered.worktree_path.as_deref()
        );
        assert_eq!(None, recovered.surface_id);
    }

    #[test]
    fn running_surface_resolves_to_durable_agent_identity() {
        let store = OrchestrationStore::in_memory().unwrap();
        let (session, agent) = seed(&store);
        store.mark_ready(agent.id, 4).unwrap();
        store.mark_running(agent.id, SurfaceId(9), 5).unwrap();
        let identity = store.resolve(SurfaceId(9)).unwrap();
        assert_eq!(agent.agent_key, identity.agent_id);
        assert_eq!(session.workspace_id.0, identity.workspace_id);
        assert_eq!(Some("feat/writer"), identity.branch.as_deref());
    }

    #[test]
    fn file_scopes_reject_parent_escape() {
        let store = OrchestrationStore::in_memory().unwrap();
        let workspace = store
            .get_or_create_workspace("optimus", Path::new("C:/dev/Optimus"), 1)
            .unwrap();
        let session = store
            .create_session(workspace.id, "session", "abc123", 2)
            .unwrap();
        let result = store.plan_agent(
            session.id,
            None,
            "writer",
            "task",
            "codex",
            "feat/writer",
            AgentKind::Writer,
            &[FileScope::new("../outside")],
            3,
        );
        assert!(result.is_err());
        assert!(store.agents_for_session(session.id).unwrap().is_empty());
    }

    #[test]
    fn planning_after_completion_reactivates_the_session() {
        let store = OrchestrationStore::in_memory().unwrap();
        let (session, first) = seed(&store);
        store.mark_ready(first.id, 4).unwrap();
        store.mark_running(first.id, SurfaceId(9), 5).unwrap();
        store.mark_done(first.id, 0, 6).unwrap();
        assert_eq!(
            SessionState::Completed,
            store.session(session.id).unwrap().unwrap().state
        );

        store
            .plan_agent(
                session.id,
                None,
                "later",
                "task",
                "codex",
                "feat/later",
                AgentKind::Writer,
                &[FileScope::new("ui/./src")],
                7,
            )
            .unwrap();
        let reopened = store.session(session.id).unwrap().unwrap();
        assert_eq!(SessionState::Active, reopened.state);
        assert_eq!(None, reopened.ended_ts);
        assert_eq!(
            "ui/src",
            store.agents_for_session(session.id).unwrap()[1].file_scope[0].pattern
        );
    }

    #[test]
    fn per_agent_pinned_base_is_durable_and_observable() {
        let store = OrchestrationStore::in_memory().unwrap();
        let (session, first) = seed(&store);
        let second = store
            .plan_agent(
                session.id,
                None,
                "second",
                "task",
                "codex",
                "feat/second",
                AgentKind::Writer,
                &[FileScope::new("ui")],
                4,
            )
            .unwrap();
        {
            let connection = store.connection.lock().unwrap();
            connection
                .execute(
                    "UPDATE control_agent SET base_commit = 'unexpected' WHERE id = ?1",
                    [second.id.0],
                )
                .unwrap();
        }
        let agents = store.agents_for_session(session.id).unwrap();
        assert_eq!(
            "abc123",
            agents
                .iter()
                .find(|agent| agent.id == first.id)
                .unwrap()
                .base_commit
        );
        assert_eq!(
            "unexpected",
            agents
                .iter()
                .find(|agent| agent.id == second.id)
                .unwrap()
                .base_commit
        );
    }

    #[test]
    fn legacy_database_migrates_capture_failures_and_agent_bases() {
        let connection = Connection::open_in_memory().unwrap();
        let legacy_schema = SCHEMA.replace("    base_commit     TEXT,\n", "").replace(
            "CHECK(state IN ('pending','succeeded','failed'))",
            "CHECK(state IN ('pending','succeeded'))",
        );
        connection.execute_batch(&legacy_schema).unwrap();
        connection
            .execute(
                "INSERT INTO control_workspace(id, name, repo_root, created_ts)
                 VALUES (1, 'legacy', 'C:/legacy', 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO control_session(id, workspace_id, name, base_commit, state, created_ts)
                 VALUES (1, 1, 'legacy', 'legacy-base', 'completed', 2)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO control_agent(
                    id, session_id, agent_key, name, task, command, kind, state, branch, created_ts
                 ) VALUES (1, 1, '1', 'legacy', 'task', 'codex', 'writer', 'done', 'feat/legacy', 3)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO control_git_capture_job(
                    agent_id, state, attempts, last_error, created_ts, updated_ts
                 ) VALUES (1, 'pending', 2, 'failed twice', 4, 5)",
                [],
            )
            .unwrap();

        let store = OrchestrationStore::from_connection(connection).unwrap();
        assert_eq!(
            "legacy-base",
            store.agent(AgentId(1)).unwrap().unwrap().base_commit
        );
        let job = store.git_capture_job(AgentId(1)).unwrap().unwrap();
        assert_eq!("failed", job.state);
        assert_eq!(2, job.attempts);
    }

    #[test]
    fn disabling_the_relay_registry_revokes_omitted_entries() {
        let store = OrchestrationStore::in_memory().unwrap();
        let workspace = store
            .get_or_create_workspace("optimus", Path::new("C:/dev/Optimus"), 1)
            .unwrap();
        store
            .register_relay_workspace("workspace-primary", workspace.id, "Primary")
            .unwrap();
        store
            .register_relay_provider(
                "codex-subscription",
                "Codex",
                "codex",
                "prompt_arg",
                "subscription",
                None,
            )
            .unwrap();
        assert!(store
            .relay_workspace("workspace-primary")
            .unwrap()
            .is_some());
        assert!(store
            .relay_provider("codex-subscription")
            .unwrap()
            .is_some());
        store.disable_relay_registry().unwrap();
        assert!(store
            .relay_workspace("workspace-primary")
            .unwrap()
            .is_none());
        assert!(store
            .relay_provider("codex-subscription")
            .unwrap()
            .is_none());
    }
}
