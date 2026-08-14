//! M0 provenance store: a local SQLite log with a strict trusted/untrusted write boundary.
//!
//! The store deliberately contains execution metadata rather than transcripts or diff bodies.
//! Record ids are the authoritative order; timestamps are display data only.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::{Captures, Regex};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use unicode_normalization::UnicodeNormalization;

use crate::domain::ids::SurfaceId;

const MAX_TEXT_BYTES: usize = 4 * 1024;
const MAX_BRANCH_BYTES: usize = 512;
const MAX_SYMBOL_BYTES: usize = 256;
const MAX_SYMBOLS: usize = 128;
const AGENT_WRITES_PER_SECOND: u32 = 20;
const SIZE_WARNING_BYTES: u64 = 500 * 1024 * 1024;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS record (
    id           INTEGER PRIMARY KEY,
    fact         TEXT NOT NULL,
    subject_key  TEXT NOT NULL,
    why          TEXT,
    agent_id     TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK(kind IN ('decision','change','status','spawn','error')),
    trust        TEXT NOT NULL CHECK(trust IN ('trusted','untrusted')),
    file_key     TEXT,
    branch       TEXT,
    workspace_id INTEGER NOT NULL,
    reverses     INTEGER REFERENCES record(id),
    parent_record INTEGER REFERENCES record(id),
    ts           INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS edge (
    from_id  INTEGER NOT NULL REFERENCES record(id) ON DELETE CASCADE,
    to_id    INTEGER NOT NULL REFERENCES record(id) ON DELETE CASCADE,
    relation TEXT NOT NULL,
    rule     TEXT NOT NULL,
    PRIMARY KEY (from_id, to_id, relation)
);
CREATE TABLE IF NOT EXISTS symbol (
    record_id INTEGER NOT NULL REFERENCES record(id) ON DELETE CASCADE,
    sym       TEXT NOT NULL,
    role      TEXT NOT NULL CHECK(role IN ('defines','references')),
    PRIMARY KEY (record_id, sym, role)
);
CREATE INDEX IF NOT EXISTS record_ts ON record(ts);
CREATE INDEX IF NOT EXISTS record_filekey ON record(file_key) WHERE file_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS record_agent ON record(agent_id);
CREATE INDEX IF NOT EXISTS record_agent_kind_id ON record(agent_id, kind, id DESC);
CREATE INDEX IF NOT EXISTS record_ws ON record(workspace_id);
CREATE INDEX IF NOT EXISTS record_ws_id ON record(workspace_id, id DESC);
CREATE INDEX IF NOT EXISTS record_ws_kind_file ON record(workspace_id, kind, file_key, id);
CREATE INDEX IF NOT EXISTS record_subject ON record(workspace_id, file_key, kind, subject_key, id);
CREATE INDEX IF NOT EXISTS symbol_lookup ON symbol(sym, role, record_id);
CREATE TABLE IF NOT EXISTS record_source (
    source_key TEXT PRIMARY KEY,
    record_id  INTEGER NOT NULL REFERENCES record(id) ON DELETE CASCADE
);
"#;

#[derive(Debug)]
pub enum MemoryError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    UnknownSurface(SurfaceId),
    InvalidReference(i64),
    InvalidSourceKey,
    InvalidInput(String),
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "memory database: {error}"),
            Self::Io(error) => write!(f, "memory path: {error}"),
            Self::UnknownSurface(surface) => write!(f, "no agent identity for {surface}"),
            Self::InvalidReference(id) => {
                write!(
                    f,
                    "memory reference {id} is missing or belongs to another workspace"
                )
            }
            Self::InvalidSourceKey => write!(f, "memory source key is invalid"),
            Self::InvalidInput(message) => write!(f, "memory input: {message}"),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<rusqlite::Error> for MemoryError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

impl From<std::io::Error> for MemoryError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub type Result<T> = std::result::Result<T, MemoryError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    Decision,
    Change,
    Status,
    Spawn,
    Error,
}

impl RecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Change => "change",
            Self::Status => "status",
            Self::Spawn => "spawn",
            Self::Error => "error",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "decision" => Self::Decision,
            "change" => Self::Change,
            "status" => Self::Status,
            "spawn" => Self::Spawn,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

/// Agent self-reports cannot manufacture trusted spawn records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRecordKind {
    Decision,
    Change,
    Status,
    Error,
}

impl From<AgentRecordKind> for RecordKind {
    fn from(value: AgentRecordKind) -> Self {
        match value {
            AgentRecordKind::Decision => Self::Decision,
            AgentRecordKind::Change => Self::Change,
            AgentRecordKind::Status => Self::Status,
            AgentRecordKind::Error => Self::Error,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustLevel {
    Trusted,
    Untrusted,
}

impl TrustLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Untrusted => "untrusted",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "trusted" => Self::Trusted,
            "untrusted" => Self::Untrusted,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolRole {
    Defines,
    References,
}

impl SymbolRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Defines => "defines",
            Self::References => "references",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordSymbol {
    pub value: String,
    pub role: SymbolRole,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentIdentity {
    pub agent_id: String,
    pub workspace_id: i64,
    pub branch: Option<String>,
}

/// Resolves caller identity from the pipe-authenticated surface. The `memory.record` payload never
/// carries `agent_id`, workspace, branch, or trust.
pub trait AgentIdentityResolver: Send + Sync {
    fn resolve(&self, surface: SurfaceId) -> Option<AgentIdentity>;
}

#[derive(Clone, Debug)]
pub struct TrustedRecord {
    pub fact: String,
    pub why: Option<String>,
    pub agent_id: String,
    pub kind: RecordKind,
    pub file_key: Option<String>,
    pub branch: Option<String>,
    pub workspace_id: i64,
    pub reverses: Option<i64>,
    pub parent_record: Option<i64>,
    pub symbols: Vec<RecordSymbol>,
    pub ts: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct AgentRecord {
    pub fact: String,
    pub why: Option<String>,
    pub kind: AgentRecordKind,
    pub file_key: Option<String>,
    pub reverses: Option<i64>,
    pub symbols: Vec<RecordSymbol>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRecord {
    pub id: i64,
    pub fact: String,
    pub subject_key: String,
    pub why: Option<String>,
    pub agent_id: String,
    pub kind: RecordKind,
    pub trust: TrustLevel,
    pub file_key: Option<String>,
    pub branch: Option<String>,
    pub workspace_id: i64,
    pub reverses: Option<i64>,
    pub parent_record: Option<i64>,
    pub ts: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    Stored { record_id: i64 },
    Coalesced { record_id: i64, dropped: u32 },
}

#[derive(Default)]
struct RateWindow {
    started_ms: i64,
    accepted: u32,
    dropped: u32,
    coalesced_record_id: Option<i64>,
}

struct StoreState {
    connection: Connection,
    rate_windows: HashMap<SurfaceId, RateWindow>,
}

pub struct MemoryStore {
    state: Mutex<StoreState>,
    record_observers: Mutex<Vec<RecordObserver>>,
    edge_observers: Mutex<Vec<RecordObserver>>,
    resolver: Arc<dyn AgentIdentityResolver>,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    path: Option<PathBuf>,
}

pub type RecordObserver = Arc<dyn Fn(i64) + Send + Sync>;

impl MemoryStore {
    pub fn open_default(resolver: Arc<dyn AgentIdentityResolver>) -> Result<Self> {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("optimus");
        std::fs::create_dir_all(&root)?;
        Self::open(root.join("memory.db"), resolver)
    }

    pub fn open(path: impl AsRef<Path>, resolver: Arc<dyn AgentIdentityResolver>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path.as_ref())?;
        Self::from_connection(
            connection,
            resolver,
            Some(path.as_ref().to_path_buf()),
            Arc::new(now_ms),
        )
    }

    pub fn in_memory(resolver: Arc<dyn AgentIdentityResolver>) -> Result<Self> {
        Self::from_connection(
            Connection::open_in_memory()?,
            resolver,
            None,
            Arc::new(now_ms),
        )
    }

    fn from_connection(
        connection: Connection,
        resolver: Arc<dyn AgentIdentityResolver>,
        path: Option<PathBuf>,
        now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;",
        )?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self {
            state: Mutex::new(StoreState {
                connection,
                rate_windows: HashMap::new(),
            }),
            record_observers: Mutex::new(Vec::new()),
            edge_observers: Mutex::new(Vec::new()),
            resolver,
            now_ms,
            path,
        })
    }

    #[cfg(test)]
    fn in_memory_with_clock(
        resolver: Arc<dyn AgentIdentityResolver>,
        now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?, resolver, None, now_ms)
    }

    pub fn append_trusted(&self, record: TrustedRecord) -> Result<StoredRecord> {
        validate_symbol_count(&record.symbols)?;
        let cleaned = clean_trusted_record(record, (self.now_ms)());
        let stored = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            insert_record(&mut state.connection, cleaned)?
        };
        self.notify_record(stored.id);
        Ok(stored)
    }

    pub fn append_trusted_once(
        &self,
        source_key: &str,
        record: TrustedRecord,
    ) -> Result<StoredRecord> {
        validate_symbol_count(&record.symbols)?;
        if source_key.is_empty()
            || source_key.len() > 512
            || !source_key.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, ':' | '.' | '_' | '-')
            })
        {
            return Err(MemoryError::InvalidSourceKey);
        }
        let cleaned = clean_trusted_record(record, (self.now_ms)());
        let (stored, inserted) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let transaction = state.connection.transaction()?;
            let existing = transaction
                .query_row(
                    "SELECT r.id, r.fact, r.subject_key, r.why, r.agent_id, r.kind, r.trust,
                            r.file_key, r.branch, r.workspace_id, r.reverses, r.parent_record, r.ts
                     FROM record_source s JOIN record r ON r.id = s.record_id
                     WHERE s.source_key = ?1",
                    [source_key],
                    map_record,
                )
                .optional()?;
            let result = if let Some(existing) = existing {
                (existing, false)
            } else {
                let stored = insert_record_in_transaction(&transaction, cleaned)?;
                transaction.execute(
                    "INSERT INTO record_source(source_key, record_id) VALUES (?1, ?2)",
                    params![source_key, stored.id],
                )?;
                (stored, true)
            };
            transaction.commit()?;
            result
        };
        if inserted {
            self.notify_record(stored.id);
        }
        Ok(stored)
    }

    pub fn append_agent(&self, surface: SurfaceId, record: AgentRecord) -> Result<AppendOutcome> {
        validate_symbol_count(&record.symbols)?;
        let identity = self
            .resolver
            .resolve(surface)
            .ok_or(MemoryError::UnknownSurface(surface))?;
        let timestamp = (self.now_ms)();
        let cleaned = NewRecord {
            fact: scrub_text(&record.fact, MAX_TEXT_BYTES),
            why: record
                .why
                .as_deref()
                .map(|value| scrub_text(value, MAX_TEXT_BYTES)),
            agent_id: scrub_text(&identity.agent_id, MAX_BRANCH_BYTES),
            kind: record.kind.into(),
            trust: TrustLevel::Untrusted,
            file_key: scrub_file_key(record.file_key.as_deref()),
            branch: identity
                .branch
                .as_deref()
                .map(|value| scrub_text(value, MAX_BRANCH_BYTES)),
            workspace_id: identity.workspace_id,
            reverses: record.reverses,
            parent_record: None,
            symbols: scrub_symbols(record.symbols),
            ts: timestamp,
        };

        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let decision = {
            let window = state.rate_windows.entry(surface).or_default();
            if timestamp.saturating_sub(window.started_ms) >= 1_000 || timestamp < window.started_ms
            {
                *window = RateWindow {
                    started_ms: timestamp,
                    ..RateWindow::default()
                };
            }
            if window.accepted < AGENT_WRITES_PER_SECOND {
                window.accepted += 1;
                RateDecision::Store
            } else {
                window.dropped += 1;
                RateDecision::Coalesce {
                    existing_id: window.coalesced_record_id,
                    dropped: window.dropped,
                }
            }
        };

        let (outcome, notify_id) = match decision {
            RateDecision::Store => {
                let stored = insert_record(&mut state.connection, cleaned)?;
                (
                    AppendOutcome::Stored {
                        record_id: stored.id,
                    },
                    stored.id,
                )
            }
            RateDecision::Coalesce {
                existing_id,
                dropped,
            } => {
                let fact = format!("Rate limit exceeded; dropped {dropped} agent records");
                let record_id = if let Some(id) = existing_id {
                    state.connection.execute(
                        "UPDATE record SET fact = ?1, ts = ?2 WHERE id = ?3",
                        params![fact, timestamp, id],
                    )?;
                    id
                } else {
                    let summary = NewRecord {
                        fact,
                        why: None,
                        agent_id: cleaned.agent_id,
                        kind: RecordKind::Status,
                        trust: TrustLevel::Untrusted,
                        file_key: None,
                        branch: cleaned.branch,
                        workspace_id: cleaned.workspace_id,
                        reverses: None,
                        parent_record: None,
                        symbols: Vec::new(),
                        ts: timestamp,
                    };
                    insert_record(&mut state.connection, summary)?.id
                };
                state
                    .rate_windows
                    .get_mut(&surface)
                    .expect("rate window was created above")
                    .coalesced_record_id = Some(record_id);
                (AppendOutcome::Coalesced { record_id, dropped }, record_id)
            }
        };
        drop(state);
        self.notify_record(notify_id);
        Ok(outcome)
    }

    /// Subscribe to committed record inserts/updates. Observers run after the SQLite lock is
    /// released, so correlation or UI work can never hold up the write transaction.
    pub fn subscribe_records(&self, observer: RecordObserver) {
        self.record_observers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(observer);
    }

    pub fn subscribe_edges(&self, observer: RecordObserver) {
        self.edge_observers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(observer);
    }

    fn notify_record(&self, id: i64) {
        let observers = self
            .record_observers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for observer in observers {
            observer(id);
        }
    }

    pub(crate) fn notify_edges(&self, id: i64) {
        let observers = self
            .edge_observers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for observer in observers {
            observer(id);
        }
    }

    pub(crate) fn with_connection<T>(
        &self,
        action: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        action(&state.connection)
    }

    pub(crate) fn with_connection_mut<T>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        action(&mut state.connection)
    }

    pub fn records(&self, workspace_id: i64, limit: usize) -> Result<Vec<StoredRecord>> {
        self.records_before(workspace_id, limit, None)
    }

    pub fn records_before(
        &self,
        workspace_id: i64,
        limit: usize,
        before_id: Option<i64>,
    ) -> Result<Vec<StoredRecord>> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut statement = state.connection.prepare(
            "SELECT id, fact, subject_key, why, agent_id, kind, trust, file_key, branch, workspace_id, reverses, parent_record, ts
             FROM record WHERE workspace_id = ?1 AND (?2 IS NULL OR id < ?2)
             ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![workspace_id, before_id, limit.min(500) as i64],
            map_record,
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(MemoryError::from)
    }

    pub fn recent_records(&self, limit: usize) -> Result<Vec<StoredRecord>> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut statement = state.connection.prepare(
            "SELECT id, fact, subject_key, why, agent_id, kind, trust, file_key, branch,
                    workspace_id, reverses, parent_record, ts
             FROM record ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], map_record)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(MemoryError::from)
    }

    pub fn record(&self, id: i64) -> Result<Option<StoredRecord>> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .connection
            .query_row(
                "SELECT id, fact, subject_key, why, agent_id, kind, trust, file_key, branch, workspace_id, reverses, parent_record, ts
                 FROM record WHERE id = ?1",
                [id],
                map_record,
            )
            .optional()
            .map_err(MemoryError::from)
    }

    pub fn symbols(&self, record_id: i64) -> Result<Vec<RecordSymbol>> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut statement = state
            .connection
            .prepare("SELECT sym, role FROM symbol WHERE record_id = ?1 ORDER BY sym, role")?;
        let rows = statement.query_map([record_id], |row| {
            let role: String = row.get(1)?;
            Ok(RecordSymbol {
                value: row.get(0)?,
                role: if role == "defines" {
                    SymbolRole::Defines
                } else {
                    SymbolRole::References
                },
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(MemoryError::from)
    }

    pub fn clear_workspace(&self, workspace_id: i64) -> Result<usize> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let transaction = state.connection.transaction()?;
        transaction.execute(
            "UPDATE record SET reverses = NULL
             WHERE reverses IN (SELECT id FROM record WHERE workspace_id = ?1)",
            [workspace_id],
        )?;
        transaction.execute(
            "UPDATE record SET parent_record = NULL
             WHERE parent_record IN (SELECT id FROM record WHERE workspace_id = ?1)",
            [workspace_id],
        )?;
        let has_control_agents = transaction
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'control_agent'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if has_control_agents {
            transaction.execute(
                "UPDATE control_agent SET spawn_record_id = NULL
                 WHERE spawn_record_id IN (SELECT id FROM record WHERE workspace_id = ?1)",
                [workspace_id],
            )?;
        }
        let deleted =
            transaction.execute("DELETE FROM record WHERE workspace_id = ?1", [workspace_id])?;
        transaction.commit()?;
        state.rate_windows.retain(|surface, _| {
            self.resolver
                .resolve(*surface)
                .is_none_or(|identity| identity.workspace_id != workspace_id)
        });
        drop(state);
        self.notify_edges(0);
        Ok(deleted)
    }

    pub fn database_size_bytes(&self) -> Result<u64> {
        match &self.path {
            Some(path) => {
                let mut total = file_size_or_zero(path)?;
                for suffix in ["-wal", "-shm"] {
                    let mut sidecar = path.as_os_str().to_os_string();
                    sidecar.push(suffix);
                    total = total.saturating_add(file_size_or_zero(Path::new(&sidecar))?);
                }
                Ok(total)
            }
            None => Ok(0),
        }
    }

    pub fn size_warning(&self) -> Result<bool> {
        Ok(self.database_size_bytes()? >= SIZE_WARNING_BYTES)
    }
}

fn file_size_or_zero(path: &Path) -> Result<u64> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

enum RateDecision {
    Store,
    Coalesce {
        existing_id: Option<i64>,
        dropped: u32,
    },
}

struct NewRecord {
    fact: String,
    why: Option<String>,
    agent_id: String,
    kind: RecordKind,
    trust: TrustLevel,
    file_key: Option<String>,
    branch: Option<String>,
    workspace_id: i64,
    reverses: Option<i64>,
    parent_record: Option<i64>,
    symbols: Vec<RecordSymbol>,
    ts: i64,
}

fn clean_trusted_record(record: TrustedRecord, now: i64) -> NewRecord {
    NewRecord {
        fact: scrub_text(&record.fact, MAX_TEXT_BYTES),
        why: record
            .why
            .as_deref()
            .map(|value| scrub_text(value, MAX_TEXT_BYTES)),
        agent_id: scrub_text(&record.agent_id, MAX_BRANCH_BYTES),
        kind: record.kind,
        trust: TrustLevel::Trusted,
        file_key: scrub_file_key(record.file_key.as_deref()),
        branch: record
            .branch
            .as_deref()
            .map(|value| scrub_text(value, MAX_BRANCH_BYTES)),
        workspace_id: record.workspace_id,
        reverses: record.reverses,
        parent_record: record.parent_record,
        symbols: scrub_symbols(record.symbols),
        ts: record.ts.unwrap_or(now),
    }
}

fn insert_record(connection: &mut Connection, record: NewRecord) -> Result<StoredRecord> {
    let transaction = connection.transaction()?;
    let stored = insert_record_in_transaction(&transaction, record)?;
    transaction.commit()?;
    Ok(stored)
}

fn insert_record_in_transaction(
    transaction: &Transaction<'_>,
    record: NewRecord,
) -> Result<StoredRecord> {
    validate_reversal(
        transaction,
        record.reverses,
        record.workspace_id,
        record.kind,
    )?;
    validate_parent(
        transaction,
        record.parent_record,
        record.workspace_id,
        record.kind,
    )?;
    let subject_key = normalize_subject(&record.fact);
    transaction.execute(
        "INSERT INTO record(fact, subject_key, why, agent_id, kind, trust, file_key, branch, workspace_id, reverses, parent_record, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            record.fact,
            subject_key,
            record.why,
            record.agent_id,
            record.kind.as_str(),
            record.trust.as_str(),
            record.file_key,
            record.branch,
            record.workspace_id,
            record.reverses,
            record.parent_record,
            record.ts,
        ],
    )?;
    let id = transaction.last_insert_rowid();
    insert_symbols(transaction, id, &record.symbols)?;
    Ok(StoredRecord {
        id,
        fact: record.fact,
        subject_key,
        why: record.why,
        agent_id: record.agent_id,
        kind: record.kind,
        trust: record.trust,
        file_key: record.file_key,
        branch: record.branch,
        workspace_id: record.workspace_id,
        reverses: record.reverses,
        parent_record: record.parent_record,
        ts: record.ts,
    })
}

fn validate_reversal(
    transaction: &Transaction<'_>,
    reference: Option<i64>,
    workspace_id: i64,
    source_kind: RecordKind,
) -> Result<()> {
    let Some(reference) = reference else {
        return Ok(());
    };
    let target = transaction
        .query_row(
            "SELECT workspace_id, kind FROM record WHERE id = ?1",
            [reference],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if source_kind != RecordKind::Decision
        || !matches!(target, Some((workspace, ref kind)) if workspace == workspace_id && kind == "decision")
    {
        return Err(MemoryError::InvalidReference(reference));
    }
    Ok(())
}

fn validate_parent(
    transaction: &Transaction<'_>,
    reference: Option<i64>,
    workspace_id: i64,
    source_kind: RecordKind,
) -> Result<()> {
    let Some(reference) = reference else {
        return Ok(());
    };
    let target = transaction
        .query_row(
            "SELECT workspace_id, kind, trust FROM record WHERE id = ?1",
            [reference],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    if source_kind != RecordKind::Spawn
        || !matches!(target, Some((workspace, ref kind, ref trust)) if workspace == workspace_id && kind == "spawn" && trust == "trusted")
    {
        return Err(MemoryError::InvalidReference(reference));
    }
    Ok(())
}

fn insert_symbols(
    transaction: &Transaction<'_>,
    record_id: i64,
    symbols: &[RecordSymbol],
) -> Result<()> {
    for symbol in symbols {
        transaction.execute(
            "INSERT OR IGNORE INTO symbol(record_id, sym, role) VALUES (?1, ?2, ?3)",
            params![record_id, symbol.value, symbol.role.as_str()],
        )?;
    }
    Ok(())
}

pub(crate) fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRecord> {
    let kind: String = row.get(5)?;
    let trust: String = row.get(6)?;
    Ok(StoredRecord {
        id: row.get(0)?,
        fact: row.get(1)?,
        subject_key: row.get(2)?,
        why: row.get(3)?,
        agent_id: row.get(4)?,
        kind: RecordKind::parse(&kind).ok_or(rusqlite::Error::InvalidQuery)?,
        trust: TrustLevel::parse(&trust).ok_or(rusqlite::Error::InvalidQuery)?,
        file_key: row.get(7)?,
        branch: row.get(8)?,
        workspace_id: row.get(9)?,
        reverses: row.get(10)?,
        parent_record: row.get(11)?,
        ts: row.get(12)?,
    })
}

/// M1 subject identity: first line, NFC, Unicode lowercase, collapsed whitespace, 200 chars.
/// Record id remains the direction/tiebreak authority; this key only selects candidates.
pub(crate) fn normalize_subject(value: &str) -> String {
    value
        .lines()
        .next()
        .unwrap_or_default()
        .nfc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

fn scrub_symbols(symbols: Vec<RecordSymbol>) -> Vec<RecordSymbol> {
    let mut cleaned = symbols
        .into_iter()
        .filter_map(|symbol| {
            let value = scrub_text(symbol.value.trim(), MAX_SYMBOL_BYTES);
            (!value.is_empty() && !value.contains(char::is_whitespace)).then_some(RecordSymbol {
                value,
                role: symbol.role,
            })
        })
        .collect::<Vec<_>>();
    cleaned.sort_by(|a, b| {
        a.value
            .cmp(&b.value)
            .then(a.role.as_str().cmp(b.role.as_str()))
    });
    cleaned.dedup();
    cleaned
}

fn validate_symbol_count(symbols: &[RecordSymbol]) -> Result<()> {
    if symbols.len() > MAX_SYMBOLS {
        return Err(MemoryError::InvalidInput(format!(
            "at most {MAX_SYMBOLS} symbols are allowed"
        )));
    }
    Ok(())
}

fn scrub_file_key(value: Option<&str>) -> Option<String> {
    let normalized = value?.trim().replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(&lower);
    let secret_name = file_name == ".env"
        || file_name.starts_with(".env.")
        || file_name.ends_with(".pem")
        || file_name == "id_rsa"
        || file_name == "id_ed25519"
        || file_name.starts_with("id_");
    if normalized.is_empty()
        || Path::new(&normalized).is_absolute()
        || normalized.split('/').any(|part| part == "..")
        || secret_name
    {
        return None;
    }
    Some(scrub_text(&normalized, MAX_BRANCH_BYTES))
}

fn scrub_text(value: &str, max_bytes: usize) -> String {
    static PEM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]+-----.*?(?:-----END [A-Z0-9 ]+-----|$)")
            .expect("valid PEM regex")
    });
    static ENV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^[A-Z][A-Z0-9_]{2,}\s*=\s*[^\r\n]+$").expect("valid env regex")
    });
    static PREFIXED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9_-]{16,}|xox[a-z]-[A-Za-z0-9-]{16,})")
            .expect("valid prefixed-secret regex")
    });
    static JWT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}")
            .expect("valid JWT regex")
    });
    static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[A-Za-z0-9_+/=-]{32,}").expect("valid high-entropy token regex")
    });

    let value = PEM.replace_all(value, "[REDACTED:pem]");
    let value = ENV.replace_all(&value, "[REDACTED:env]");
    let value = PREFIXED.replace_all(&value, "[REDACTED:token]");
    let value = JWT.replace_all(&value, "[REDACTED:jwt]");
    let value = TOKEN.replace_all(&value, |captures: &Captures<'_>| {
        let candidate = captures.get(0).map_or("", |value| value.as_str());
        if shannon_entropy(candidate) >= 4.0 {
            "[REDACTED:entropy]".to_string()
        } else {
            candidate.to_string()
        }
    });
    truncate_utf8(&value, max_bytes)
}

fn shannon_entropy(value: &str) -> f64 {
    let mut counts = [0_u32; 256];
    for byte in value.bytes() {
        counts[byte as usize] += 1;
    }
    let length = value.len() as f64;
    if length == 0.0 {
        return 0.0;
    }
    counts
        .into_iter()
        .filter(|count| *count > 0)
        .map(|count| {
            let probability = f64::from(count) / length;
            -probability * probability.log2()
        })
        .sum()
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

#[cfg(test)]
pub(crate) fn test_timestamp() -> i64 {
    now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct Resolver {
        identities: HashMap<SurfaceId, AgentIdentity>,
    }

    impl AgentIdentityResolver for Resolver {
        fn resolve(&self, surface: SurfaceId) -> Option<AgentIdentity> {
            self.identities.get(&surface).cloned()
        }
    }

    fn resolver() -> Arc<dyn AgentIdentityResolver> {
        Arc::new(Resolver {
            identities: HashMap::from([(
                SurfaceId(7),
                AgentIdentity {
                    agent_id: "agent-7".to_string(),
                    workspace_id: 42,
                    branch: Some("feat/agent-seven".to_string()),
                },
            )]),
        })
    }

    fn trusted(fact: &str, workspace_id: i64) -> TrustedRecord {
        TrustedRecord {
            fact: fact.to_string(),
            why: Some("because".to_string()),
            agent_id: "orchestrator".to_string(),
            kind: RecordKind::Spawn,
            file_key: None,
            branch: Some("main".to_string()),
            workspace_id,
            reverses: None,
            parent_record: None,
            symbols: Vec::new(),
            ts: Some(100),
        }
    }

    fn agent(fact: &str) -> AgentRecord {
        AgentRecord {
            fact: fact.to_string(),
            why: None,
            kind: AgentRecordKind::Change,
            file_key: Some("shell/src/lib.rs".to_string()),
            reverses: None,
            symbols: Vec::new(),
        }
    }

    #[test]
    fn trusted_and_agent_records_share_one_monotonic_log() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let first = store.append_trusted(trusted("spawned", 42)).unwrap();
        let second = store
            .append_agent(SurfaceId(7), agent("changed host"))
            .unwrap();

        assert_eq!(1, first.id);
        assert_eq!(AppendOutcome::Stored { record_id: 2 }, second);
        let records = store.records(42, 10).unwrap();
        assert_eq!(
            vec![2, 1],
            records.iter().map(|record| record.id).collect::<Vec<_>>()
        );
        assert_eq!("agent-7", records[0].agent_id);
        assert_eq!(Some("feat/agent-seven"), records[0].branch.as_deref());
        assert_eq!(TrustLevel::Untrusted, records[0].trust);
        assert_eq!(TrustLevel::Trusted, records[1].trust);
    }

    #[test]
    fn agent_identity_is_required_and_never_comes_from_the_payload() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let error = store.append_agent(SurfaceId(999), agent("forged"));
        assert!(matches!(
            error,
            Err(MemoryError::UnknownSurface(SurfaceId(999)))
        ));
        assert!(store.records(42, 10).unwrap().is_empty());
    }

    #[test]
    fn write_boundary_scrubs_secrets_and_caps_text() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let long = "x".repeat(MAX_TEXT_BYTES + 50);
        let mut record = trusted(
            "TOKEN=secret-value\nghp_abcdefghijklmnopqrstuvwxyz0123456789 eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.VeryLongSignatureValue1234567890",
            42,
        );
        record.why = Some(long);
        let stored = store.append_trusted(record).unwrap();

        assert!(stored.fact.contains("[REDACTED:env]"), "{}", stored.fact);
        assert!(stored.fact.contains("[REDACTED:token]"), "{}", stored.fact);
        assert!(stored.fact.contains("[REDACTED:jwt]"), "{}", stored.fact);
        assert!(stored.why.unwrap().len() <= MAX_TEXT_BYTES);
    }

    #[test]
    fn secret_and_unsafe_file_names_never_land_as_correlation_keys() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        for path in [
            ".env.local",
            "keys/deploy.pem",
            "../outside.rs",
            "C:\\secret.rs",
        ] {
            let mut record = agent("changed");
            record.file_key = Some(path.to_string());
            store.append_agent(SurfaceId(7), record).unwrap();
        }
        assert!(store
            .records(42, 10)
            .unwrap()
            .iter()
            .all(|record| record.file_key.is_none()));
    }

    #[test]
    fn symbol_rows_are_typed_deduplicated_and_indexable() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let mut record = trusted("symbols", 42);
        record.symbols = vec![
            RecordSymbol {
                value: "MemoryStore".into(),
                role: SymbolRole::Defines,
            },
            RecordSymbol {
                value: "MemoryStore".into(),
                role: SymbolRole::Defines,
            },
            RecordSymbol {
                value: "Connection".into(),
                role: SymbolRole::References,
            },
        ];
        let stored = store.append_trusted(record).unwrap();
        let symbols = store.symbols(stored.id).unwrap();
        assert_eq!(2, symbols.len());
        assert!(symbols.contains(&RecordSymbol {
            value: "MemoryStore".into(),
            role: SymbolRole::Defines
        }));
        assert!(symbols.contains(&RecordSymbol {
            value: "Connection".into(),
            role: SymbolRole::References
        }));
    }

    #[test]
    fn oversized_symbol_arrays_are_rejected_atomically() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let mut record = trusted("many symbols", 42);
        record.symbols = (0..=MAX_SYMBOLS)
            .map(|index| RecordSymbol {
                value: format!("Symbol{index}"),
                role: SymbolRole::References,
            })
            .collect();
        assert!(matches!(
            store.append_trusted(record),
            Err(MemoryError::InvalidInput(_))
        ));
        assert!(store.records(42, 10).unwrap().is_empty());
    }

    #[test]
    fn agent_timestamp_comes_from_the_local_clock() {
        let clock = Arc::new(AtomicI64::new(7_777));
        let reader = Arc::clone(&clock);
        let store = MemoryStore::in_memory_with_clock(
            resolver(),
            Arc::new(move || reader.load(Ordering::Relaxed)),
        )
        .unwrap();
        let outcome = store
            .append_agent(SurfaceId(7), agent("local time"))
            .unwrap();
        let id = match outcome {
            AppendOutcome::Stored { record_id } => record_id,
            other => panic!("unexpected outcome: {other:?}"),
        };
        assert_eq!(7_777, store.record(id).unwrap().unwrap().ts);
    }

    #[test]
    fn cross_workspace_references_are_rejected() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let first = store.append_trusted(trusted("first", 1)).unwrap();
        let mut second = trusted("second", 2);
        second.reverses = Some(first.id);
        assert!(matches!(
            store.append_trusted(second),
            Err(MemoryError::InvalidReference(id)) if id == first.id
        ));
        assert_eq!(1, store.clear_workspace(1).unwrap());
    }

    #[test]
    fn decision_reversal_requires_a_same_workspace_decision() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let mut original = trusted("choose sqlite", 42);
        original.kind = RecordKind::Decision;
        let original = store.append_trusted(original).unwrap();
        let mut reversal = trusted("choose postgres", 42);
        reversal.kind = RecordKind::Decision;
        reversal.reverses = Some(original.id);
        assert_eq!(
            Some(original.id),
            store.append_trusted(reversal).unwrap().reverses
        );

        let spawn = store.append_trusted(trusted("spawn", 42)).unwrap();
        let mut wrong_kind = trusted("not a decision", 42);
        wrong_kind.kind = RecordKind::Decision;
        wrong_kind.reverses = Some(spawn.id);
        assert!(matches!(
            store.append_trusted(wrong_kind),
            Err(MemoryError::InvalidReference(id)) if id == spawn.id
        ));
    }

    #[test]
    fn trusted_source_keys_make_lifecycle_capture_idempotent() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let first = store
            .append_trusted_once("git-stop:agent-7:abc123", trusted("first", 42))
            .unwrap();
        let second = store
            .append_trusted_once("git-stop:agent-7:abc123", trusted("different", 42))
            .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!("first", second.fact);
        assert_eq!(1, store.records(42, 10).unwrap().len());
    }

    #[test]
    fn agent_rate_limit_coalesces_excess_without_growing_the_log() {
        let clock = Arc::new(AtomicI64::new(1_000));
        let clock_reader = Arc::clone(&clock);
        let store = MemoryStore::in_memory_with_clock(
            resolver(),
            Arc::new(move || clock_reader.load(Ordering::Relaxed)),
        )
        .unwrap();

        for index in 0..AGENT_WRITES_PER_SECOND {
            let outcome = store
                .append_agent(SurfaceId(7), agent(&format!("record-{index}")))
                .unwrap();
            assert!(matches!(outcome, AppendOutcome::Stored { .. }));
        }
        let first_drop = store.append_agent(SurfaceId(7), agent("drop-a")).unwrap();
        let second_drop = store.append_agent(SurfaceId(7), agent("drop-b")).unwrap();
        assert_eq!(
            AppendOutcome::Coalesced {
                record_id: 21,
                dropped: 1
            },
            first_drop
        );
        assert_eq!(
            AppendOutcome::Coalesced {
                record_id: 21,
                dropped: 2
            },
            second_drop
        );
        let records = store.records(42, 100).unwrap();
        assert_eq!(21, records.len());
        assert_eq!(
            "Rate limit exceeded; dropped 2 agent records",
            records[0].fact
        );

        clock.store(2_001, Ordering::Relaxed);
        assert!(matches!(
            store
                .append_agent(SurfaceId(7), agent("new-window"))
                .unwrap(),
            AppendOutcome::Stored { record_id: 22 }
        ));
    }

    #[test]
    fn clear_is_scoped_to_one_workspace_and_cascades_metadata() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let first = store.append_trusted(trusted("one", 1)).unwrap();
        store.append_trusted(trusted("two", 2)).unwrap();
        assert_eq!(1, store.clear_workspace(1).unwrap());
        assert!(store.record(first.id).unwrap().is_none());
        assert_eq!(1, store.records(2, 10).unwrap().len());
    }

    #[test]
    fn clear_detaches_orchestration_spawn_record_pointers() {
        let store = MemoryStore::in_memory(resolver()).unwrap();
        let record = store.append_trusted(trusted("spawn", 1)).unwrap();
        store
            .with_connection_mut(|connection| {
                connection.execute_batch(
                    "CREATE TABLE control_agent(id INTEGER PRIMARY KEY, spawn_record_id INTEGER);
                     INSERT INTO control_agent(id, spawn_record_id) VALUES (1, 1);",
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(1, store.clear_workspace(1).unwrap());
        let pointer = store
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT spawn_record_id FROM control_agent WHERE id = 1",
                        [],
                        |row| row.get::<_, Option<i64>>(0),
                    )
                    .map_err(MemoryError::from)
            })
            .unwrap();
        assert_eq!(None, pointer);
        assert!(store.record(record.id).unwrap().is_none());
    }
}
