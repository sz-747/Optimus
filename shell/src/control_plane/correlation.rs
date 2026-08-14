//! M1 deterministic correlation policies. Records commit first; a single FIFO worker derives
//! bounded, explainable edges afterward. Every candidate query is workspace-scoped and indexed.

use std::collections::HashSet;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::control_plane::memory::{
    map_record, MemoryError, MemoryStore, RecordKind, Result, StoredRecord, TrustLevel,
};

const SAME_FILE_LIMIT: usize = 20;
const CONTRADICTION_WINDOW: i64 = 200;
const SYMBOL_MATCH_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeRelation {
    SameFile,
    DependsOn,
    Supersedes,
    Contradicts,
    DecisionReversed,
    Spawned,
}

impl EdgeRelation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SameFile => "same_file",
            Self::DependsOn => "depends_on",
            Self::Supersedes => "supersedes",
            Self::Contradicts => "contradicts",
            Self::DecisionReversed => "decision_reversed",
            Self::Spawned => "spawned",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "same_file" => Self::SameFile,
            "depends_on" => Self::DependsOn,
            "supersedes" => Self::Supersedes,
            "contradicts" => Self::Contradicts,
            "decision_reversed" => Self::DecisionReversed,
            "spawned" => Self::Spawned,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEdge {
    pub from_id: i64,
    pub to_id: i64,
    pub relation: EdgeRelation,
    pub rule: String,
}

impl StoredEdge {
    fn new(from_id: i64, to_id: i64, relation: EdgeRelation, rule: &str) -> Self {
        Self {
            from_id,
            to_id,
            relation,
            rule: rule.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct CorrelationEngine {
    store: Arc<MemoryStore>,
}

impl CorrelationEngine {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }

    pub fn correlate_record(&self, id: i64) -> Result<Vec<StoredEdge>> {
        let Some(record) = self.store.record(id)? else {
            return Ok(Vec::new());
        };
        let mut edges = Vec::new();
        self.same_file(&record, &mut edges)?;
        self.supersedes_and_reversal(&record, &mut edges)?;
        self.depends_on(&record, &mut edges)?;
        self.contradicts(&record, &mut edges)?;
        self.spawned(&record, &mut edges)?;
        edges.sort_by_key(|edge| (edge.from_id, edge.to_id, edge.relation.as_str()));
        edges.dedup_by(|left, right| {
            left.from_id == right.from_id
                && left.to_id == right.to_id
                && left.relation == right.relation
        });
        if self.insert_edges(&edges)? > 0 {
            self.store.notify_edges(record.id);
        }
        Ok(edges)
    }

    pub fn edges_for_workspace(&self, workspace_id: i64) -> Result<Vec<StoredEdge>> {
        self.store.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT e.from_id, e.to_id, e.relation, e.rule
                 FROM edge e JOIN record r ON r.id = e.from_id
                 WHERE r.workspace_id = ?1 ORDER BY e.from_id, e.to_id, e.relation",
            )?;
            let rows = statement.query_map([workspace_id], map_edge)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(MemoryError::from)
        })
    }

    pub fn edges_for_record(&self, record_id: i64) -> Result<Vec<StoredEdge>> {
        self.store.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT from_id, to_id, relation, rule FROM edge
                 WHERE from_id = ?1 OR to_id = ?1 ORDER BY from_id, to_id, relation",
            )?;
            let rows = statement.query_map([record_id], map_edge)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(MemoryError::from)
        })
    }

    fn same_file(&self, record: &StoredRecord, edges: &mut Vec<StoredEdge>) -> Result<()> {
        let Some(file_key) = record.file_key.as_deref() else {
            return Ok(());
        };
        let prior = self.store.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, fact, subject_key, why, agent_id, kind, trust, file_key, branch,
                        workspace_id, reverses, parent_record, ts
                 FROM record
                 WHERE workspace_id = ?1 AND file_key = ?2 AND id < ?3
                 ORDER BY id DESC LIMIT ?4",
            )?;
            let rows = statement.query_map(
                params![
                    record.workspace_id,
                    file_key,
                    record.id,
                    SAME_FILE_LIMIT as i64
                ],
                map_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(MemoryError::from)
        })?;
        edges.extend(prior.into_iter().map(|candidate| {
            StoredEdge::new(
                record.id,
                candidate.id,
                EdgeRelation::SameFile,
                "same_file:v1:k20",
            )
        }));
        Ok(())
    }

    fn supersedes_and_reversal(
        &self,
        record: &StoredRecord,
        edges: &mut Vec<StoredEdge>,
    ) -> Result<()> {
        let Some(file_key) = record.file_key.as_deref() else {
            return Ok(());
        };
        if !record.subject_key.is_empty() {
            let prior = self.store.with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT id, fact, subject_key, why, agent_id, kind, trust, file_key,
                                branch, workspace_id, reverses, parent_record, ts
                         FROM record
                         WHERE workspace_id = ?1 AND file_key = ?2 AND kind = ?3
                           AND subject_key = ?4 AND id < ?5
                         ORDER BY id DESC LIMIT 1",
                        params![
                            record.workspace_id,
                            file_key,
                            record.kind.as_str(),
                            record.subject_key,
                            record.id
                        ],
                        map_record,
                    )
                    .optional()
                    .map_err(MemoryError::from)
            })?;
            if let Some(prior) = prior {
                edges.push(StoredEdge::new(
                    record.id,
                    prior.id,
                    EdgeRelation::Supersedes,
                    "supersedes:v1:subject-key",
                ));
            }
        }

        if record.kind != RecordKind::Decision {
            return Ok(());
        }

        if let Some(target_id) = record.reverses {
            if let Some(target) = self.store.record(target_id)? {
                if target.id < record.id
                    && target.workspace_id == record.workspace_id
                    && target.kind == RecordKind::Decision
                    && target.file_key.as_deref() == Some(file_key)
                {
                    edges.push(StoredEdge::new(
                        record.id,
                        target.id,
                        EdgeRelation::Supersedes,
                        "supersedes:v1:explicit-reversal",
                    ));
                    edges.push(StoredEdge::new(
                        record.id,
                        target.id,
                        EdgeRelation::DecisionReversed,
                        "decision_reversed:v1:explicit",
                    ));
                }
            }
        }

        let candidates = self.prior_decisions(record.workspace_id, file_key, record.id)?;
        for candidate in candidates {
            if has_closed_antonym_pair(&record.subject_key, &candidate.subject_key) {
                edges.push(StoredEdge::new(
                    record.id,
                    candidate.id,
                    EdgeRelation::Supersedes,
                    "supersedes:v1:closed-antonym",
                ));
                edges.push(StoredEdge::new(
                    record.id,
                    candidate.id,
                    EdgeRelation::DecisionReversed,
                    "decision_reversed:v1:closed-antonym",
                ));
                break;
            }
        }
        Ok(())
    }

    fn prior_decisions(
        &self,
        workspace_id: i64,
        file_key: &str,
        before_id: i64,
    ) -> Result<Vec<StoredRecord>> {
        self.store.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, fact, subject_key, why, agent_id, kind, trust, file_key, branch,
                        workspace_id, reverses, parent_record, ts
                 FROM record WHERE workspace_id = ?1 AND file_key = ?2 AND kind = 'decision'
                   AND id < ?3 ORDER BY id DESC LIMIT ?4",
            )?;
            let rows = statement.query_map(
                params![workspace_id, file_key, before_id, SAME_FILE_LIMIT as i64],
                map_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(MemoryError::from)
        })
    }

    fn depends_on(&self, record: &StoredRecord, edges: &mut Vec<StoredEdge>) -> Result<()> {
        let candidates = self.store.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT DISTINCT r.id, r.fact, r.subject_key, r.why, r.agent_id, r.kind,
                        r.trust, r.file_key, r.branch, r.workspace_id, r.reverses,
                        r.parent_record, r.ts
                 FROM symbol wanted
                 JOIN symbol defined ON defined.sym = wanted.sym AND defined.role = 'defines'
                 JOIN record r ON r.id = defined.record_id
                 WHERE wanted.record_id = ?1 AND wanted.role = 'references'
                   AND r.workspace_id = ?2 AND r.id < ?1
                 ORDER BY r.id DESC LIMIT ?3",
            )?;
            let rows = statement.query_map(
                params![record.id, record.workspace_id, SYMBOL_MATCH_LIMIT as i64],
                map_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(MemoryError::from)
        })?;
        edges.extend(candidates.into_iter().map(|candidate| {
            StoredEdge::new(
                record.id,
                candidate.id,
                EdgeRelation::DependsOn,
                "depends_on:v1:symbol-index",
            )
        }));
        Ok(())
    }

    fn contradicts(&self, record: &StoredRecord, edges: &mut Vec<StoredEdge>) -> Result<()> {
        let Some(file_key) = record.file_key.as_deref() else {
            return Ok(());
        };
        if record.kind != RecordKind::Change || record.trust != TrustLevel::Trusted {
            return Ok(());
        }
        let lower_id = record.id.saturating_sub(CONTRADICTION_WINDOW);
        let candidates = self.store.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, fact, subject_key, why, agent_id, kind, trust, file_key, branch,
                        workspace_id, reverses, parent_record, ts
                 FROM record
                 WHERE workspace_id = ?1 AND file_key = ?2 AND kind = 'change'
                   AND trust = 'trusted' AND agent_id <> ?3 AND id BETWEEN ?4 AND ?5
                 ORDER BY id DESC",
            )?;
            let rows = statement.query_map(
                params![
                    record.workspace_id,
                    file_key,
                    record.agent_id,
                    lower_id,
                    record.id - 1
                ],
                map_record,
            )?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(MemoryError::from)
        })?;
        edges.extend(candidates.into_iter().map(|candidate| {
            StoredEdge::new(
                record.id,
                candidate.id,
                EdgeRelation::Contradicts,
                "contradicts:v1:trusted-window-200",
            )
        }));
        Ok(())
    }

    fn spawned(&self, record: &StoredRecord, edges: &mut Vec<StoredEdge>) -> Result<()> {
        if record.kind != RecordKind::Spawn || record.trust != TrustLevel::Trusted {
            return Ok(());
        }
        let Some(parent_id) = record.parent_record else {
            return Ok(());
        };
        if let Some(parent) = self.store.record(parent_id)? {
            if parent.id < record.id
                && parent.workspace_id == record.workspace_id
                && parent.kind == RecordKind::Spawn
                && parent.trust == TrustLevel::Trusted
            {
                edges.push(StoredEdge::new(
                    parent.id,
                    record.id,
                    EdgeRelation::Spawned,
                    "spawned:v1:trusted-parent",
                ));
            }
        }
        Ok(())
    }

    fn insert_edges(&self, edges: &[StoredEdge]) -> Result<usize> {
        self.store.with_connection_mut(|connection| {
            let transaction = connection.transaction()?;
            let mut inserted = 0;
            for edge in edges {
                inserted += transaction.execute(
                    "INSERT OR IGNORE INTO edge(from_id, to_id, relation, rule)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![edge.from_id, edge.to_id, edge.relation.as_str(), edge.rule],
                )?;
            }
            transaction.commit()?;
            Ok(inserted)
        })
    }
}

enum WorkerMessage {
    Record(i64),
    Barrier(Sender<()>),
    Stop,
}

/// Process-lifetime FIFO correlation worker. `flush` is the deterministic test/read-after-write
/// barrier; ordinary UI reads are eventually consistent and receive an edge update shortly after
/// the record event.
pub struct CorrelationWorker {
    tx: Sender<WorkerMessage>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl CorrelationWorker {
    pub fn attach(store: Arc<MemoryStore>) -> Self {
        let (tx, rx) = mpsc::channel();
        let observer_tx = tx.clone();
        store.subscribe_records(Arc::new(move |id| {
            let _ = observer_tx.send(WorkerMessage::Record(id));
        }));
        let thread = thread::Builder::new()
            .name("optimus-correlation".to_string())
            .spawn(move || {
                let engine = CorrelationEngine::new(store);
                while let Ok(message) = rx.recv() {
                    match message {
                        WorkerMessage::Record(id) => {
                            let _ = engine.correlate_record(id);
                        }
                        WorkerMessage::Barrier(done) => {
                            let _ = done.send(());
                        }
                        WorkerMessage::Stop => break,
                    }
                }
            })
            .expect("spawn correlation worker");
        Self {
            tx,
            thread: Mutex::new(Some(thread)),
        }
    }

    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(WorkerMessage::Barrier(tx)).is_ok() {
            let _ = rx.recv();
        }
    }
}

impl Drop for CorrelationWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(WorkerMessage::Stop);
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
    }
}

fn map_edge(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEdge> {
    let relation: String = row.get(2)?;
    Ok(StoredEdge {
        from_id: row.get(0)?,
        to_id: row.get(1)?,
        relation: EdgeRelation::parse(&relation).ok_or(rusqlite::Error::InvalidQuery)?,
        rule: row.get(3)?,
    })
}

fn has_closed_antonym_pair(left: &str, right: &str) -> bool {
    const PAIRS: [(&str, &str); 4] = [
        ("adopt", "drop"),
        ("use", "remove"),
        ("enable", "disable"),
        ("add", "delete"),
    ];
    let left = tokens(left);
    let right = tokens(right);
    PAIRS.iter().any(|(a, b)| {
        (left.contains(*a) && right.contains(*b)) || (left.contains(*b) && right.contains(*a))
    })
}

fn tokens(value: &str) -> HashSet<&str> {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::memory::{
        AgentIdentity, AgentIdentityResolver, RecordSymbol, SymbolRole, TrustedRecord,
    };
    use crate::domain::ids::SurfaceId;
    use std::collections::HashMap;
    use std::time::Instant;

    struct NoAgents;

    impl AgentIdentityResolver for NoAgents {
        fn resolve(&self, _surface: SurfaceId) -> Option<AgentIdentity> {
            None
        }
    }

    struct Agents(HashMap<SurfaceId, AgentIdentity>);

    impl AgentIdentityResolver for Agents {
        fn resolve(&self, surface: SurfaceId) -> Option<AgentIdentity> {
            self.0.get(&surface).cloned()
        }
    }

    fn store() -> Arc<MemoryStore> {
        Arc::new(MemoryStore::in_memory(Arc::new(NoAgents)).unwrap())
    }

    fn record(
        fact: &str,
        agent: &str,
        kind: RecordKind,
        trust_file: Option<&str>,
        workspace: i64,
    ) -> TrustedRecord {
        TrustedRecord {
            fact: fact.to_string(),
            why: None,
            agent_id: agent.to_string(),
            kind,
            file_key: trust_file.map(str::to_string),
            branch: Some(format!("feat/{agent}")),
            workspace_id: workspace,
            reverses: None,
            parent_record: None,
            symbols: Vec::new(),
            ts: Some(100),
        }
    }

    fn relations(edges: &[StoredEdge]) -> HashSet<EdgeRelation> {
        edges.iter().map(|edge| edge.relation).collect()
    }

    #[test]
    fn worker_derives_same_file_and_supersedes_after_commit() {
        let store = store();
        let worker = CorrelationWorker::attach(Arc::clone(&store));
        store
            .append_trusted(record(
                "Use SQLite for provenance",
                "a",
                RecordKind::Decision,
                Some("shell/store.rs"),
                1,
            ))
            .unwrap();
        let second = store
            .append_trusted(record(
                "  use   sqlite for provenance  \nignored second line",
                "b",
                RecordKind::Decision,
                Some("shell/store.rs"),
                1,
            ))
            .unwrap();
        worker.flush();

        let edges = CorrelationEngine::new(store)
            .edges_for_record(second.id)
            .unwrap();
        let found = relations(&edges);
        assert!(found.contains(&EdgeRelation::SameFile));
        assert!(found.contains(&EdgeRelation::Supersedes));
    }

    #[test]
    fn monotonic_id_not_timestamp_decides_edge_direction() {
        let store = store();
        let engine = CorrelationEngine::new(Arc::clone(&store));
        let mut older = record("Use SQLite", "a", RecordKind::Decision, Some("store.rs"), 1);
        older.ts = Some(9_000);
        let older = store.append_trusted(older).unwrap();
        let mut newer = record("Use SQLite", "b", RecordKind::Decision, Some("store.rs"), 1);
        newer.ts = Some(100);
        let newer = store.append_trusted(newer).unwrap();

        let edges = engine.correlate_record(newer.id).unwrap();
        assert!(edges.iter().any(|edge| {
            edge.relation == EdgeRelation::Supersedes
                && edge.from_id == newer.id
                && edge.to_id == older.id
        }));
    }

    #[test]
    fn explicit_and_closed_antonym_reversals_are_precise() {
        let store = store();
        let engine = CorrelationEngine::new(Arc::clone(&store));
        let prior = store
            .append_trusted(record(
                "Enable automatic rebases",
                "a",
                RecordKind::Decision,
                Some("orchestration.rs"),
                1,
            ))
            .unwrap();
        let mut explicit = record(
            "Use pinned bases instead",
            "b",
            RecordKind::Decision,
            Some("orchestration.rs"),
            1,
        );
        explicit.reverses = Some(prior.id);
        let explicit = store.append_trusted(explicit).unwrap();
        let edges = engine.correlate_record(explicit.id).unwrap();
        assert!(relations(&edges).contains(&EdgeRelation::DecisionReversed));

        let antonym = store
            .append_trusted(record(
                "Disable automatic rebases",
                "c",
                RecordKind::Decision,
                Some("orchestration.rs"),
                1,
            ))
            .unwrap();
        let edges = engine.correlate_record(antonym.id).unwrap();
        assert!(relations(&edges).contains(&EdgeRelation::DecisionReversed));

        let negation_only = store
            .append_trusted(record(
                "Do not reconsider the retry policy",
                "d",
                RecordKind::Decision,
                Some("orchestration.rs"),
                1,
            ))
            .unwrap();
        let edges = engine.correlate_record(negation_only.id).unwrap();
        assert!(!relations(&edges).contains(&EdgeRelation::DecisionReversed));
    }

    #[test]
    fn symbol_join_is_indexed_and_workspace_scoped() {
        let store = store();
        let engine = CorrelationEngine::new(Arc::clone(&store));
        let mut definition = record(
            "Define MemoryStore",
            "a",
            RecordKind::Change,
            Some("memory.rs"),
            1,
        );
        definition.symbols.push(RecordSymbol {
            value: "MemoryStore".to_string(),
            role: SymbolRole::Defines,
        });
        let definition = store.append_trusted(definition).unwrap();
        let mut reference = record(
            "Call MemoryStore",
            "b",
            RecordKind::Change,
            Some("main.rs"),
            1,
        );
        reference.symbols.push(RecordSymbol {
            value: "MemoryStore".to_string(),
            role: SymbolRole::References,
        });
        let reference = store.append_trusted(reference).unwrap();
        let edges = engine.correlate_record(reference.id).unwrap();
        assert!(edges.iter().any(|edge| {
            edge.relation == EdgeRelation::DependsOn && edge.to_id == definition.id
        }));

        let mut other_workspace = record(
            "Call MemoryStore elsewhere",
            "c",
            RecordKind::Change,
            Some("main.rs"),
            2,
        );
        other_workspace.symbols.push(RecordSymbol {
            value: "MemoryStore".to_string(),
            role: SymbolRole::References,
        });
        let other = store.append_trusted(other_workspace).unwrap();
        assert!(!engine
            .correlate_record(other.id)
            .unwrap()
            .iter()
            .any(|edge| edge.relation == EdgeRelation::DependsOn));
    }

    #[test]
    fn contradictions_require_two_trusted_agents_on_the_same_file() {
        let store = store();
        let engine = CorrelationEngine::new(Arc::clone(&store));
        let first = store
            .append_trusted(record(
                "Change capacity",
                "agent-a",
                RecordKind::Change,
                Some("capacity.rs"),
                1,
            ))
            .unwrap();
        let second = store
            .append_trusted(record(
                "Change capacity differently",
                "agent-b",
                RecordKind::Change,
                Some("capacity.rs"),
                1,
            ))
            .unwrap();
        let edges = engine.correlate_record(second.id).unwrap();
        assert!(edges
            .iter()
            .any(|edge| { edge.relation == EdgeRelation::Contradicts && edge.to_id == first.id }));

        let same_agent = store
            .append_trusted(record(
                "Change capacity again",
                "agent-b",
                RecordKind::Change,
                Some("capacity.rs"),
                1,
            ))
            .unwrap();
        let edges = engine.correlate_record(same_agent.id).unwrap();
        assert!(!edges
            .iter()
            .any(|edge| { edge.relation == EdgeRelation::Contradicts && edge.to_id == second.id }));
    }

    #[test]
    fn untrusted_agent_change_cannot_raise_a_contradiction_badge() {
        let surface = SurfaceId(7);
        let resolver: Arc<dyn AgentIdentityResolver> = Arc::new(Agents(HashMap::from([(
            surface,
            AgentIdentity {
                agent_id: "untrusted-agent".to_string(),
                workspace_id: 1,
                branch: Some("feat/untrusted".to_string()),
            },
        )])));
        let store = Arc::new(MemoryStore::in_memory(resolver).unwrap());
        let engine = CorrelationEngine::new(Arc::clone(&store));
        store
            .append_trusted(record(
                "Trusted change",
                "trusted-agent",
                RecordKind::Change,
                Some("capacity.rs"),
                1,
            ))
            .unwrap();
        let untrusted = store
            .append_agent(
                surface,
                crate::control_plane::memory::AgentRecord {
                    fact: "Try to forge collision".to_string(),
                    why: None,
                    kind: crate::control_plane::memory::AgentRecordKind::Change,
                    file_key: Some("capacity.rs".to_string()),
                    reverses: None,
                    symbols: Vec::new(),
                },
            )
            .unwrap();
        let id = match untrusted {
            crate::control_plane::memory::AppendOutcome::Stored { record_id } => record_id,
            other => panic!("unexpected rate outcome: {other:?}"),
        };
        assert!(!engine
            .correlate_record(id)
            .unwrap()
            .iter()
            .any(|edge| edge.relation == EdgeRelation::Contradicts));
    }

    #[test]
    fn spawned_edge_can_only_follow_a_trusted_spawn_parent() {
        let store = store();
        let engine = CorrelationEngine::new(Arc::clone(&store));
        let parent = store
            .append_trusted(record("Spawn root", "root", RecordKind::Spawn, None, 1))
            .unwrap();
        let mut child = record("Spawn child", "child", RecordKind::Spawn, None, 1);
        child.parent_record = Some(parent.id);
        let child = store.append_trusted(child).unwrap();
        let edges = engine.correlate_record(child.id).unwrap();
        assert!(edges.iter().any(|edge| {
            edge.relation == EdgeRelation::Spawned
                && edge.from_id == parent.id
                && edge.to_id == child.id
        }));
    }

    #[test]
    fn null_file_flood_and_hot_file_have_bounded_edges() {
        let store = store();
        let engine = CorrelationEngine::new(Arc::clone(&store));
        for index in 0..500 {
            store
                .append_trusted(record(
                    &format!("status {index}"),
                    "agent",
                    RecordKind::Status,
                    None,
                    1,
                ))
                .unwrap();
        }
        let null_tail = store
            .append_trusted(record("tail", "agent", RecordKind::Status, None, 1))
            .unwrap();
        assert!(engine.correlate_record(null_tail.id).unwrap().is_empty());

        let mut last = None;
        for index in 0..10_000 {
            last = Some(
                store
                    .append_trusted(record(
                        &format!("change {index}"),
                        "agent",
                        RecordKind::Change,
                        Some("hot.rs"),
                        1,
                    ))
                    .unwrap(),
            );
        }
        let edges = engine.correlate_record(last.unwrap().id).unwrap();
        assert_eq!(SAME_FILE_LIMIT, edges.len());
    }

    #[test]
    fn correlation_retries_are_idempotent() {
        let store = store();
        let engine = CorrelationEngine::new(Arc::clone(&store));
        store
            .append_trusted(record("first", "a", RecordKind::Change, Some("same.rs"), 1))
            .unwrap();
        let second = store
            .append_trusted(record(
                "second",
                "b",
                RecordKind::Change,
                Some("same.rs"),
                1,
            ))
            .unwrap();
        engine.correlate_record(second.id).unwrap();
        let once = engine.edges_for_workspace(1).unwrap();
        engine.correlate_record(second.id).unwrap();
        assert_eq!(once, engine.edges_for_workspace(1).unwrap());
    }

    #[test]
    fn candidate_queries_use_declared_indexes() {
        let store = store();
        let plans = store
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "EXPLAIN QUERY PLAN SELECT id FROM record
                     WHERE workspace_id = 1 AND file_key = 'hot.rs' AND id < 100
                     ORDER BY id DESC LIMIT 20",
                )?;
                let rows = statement.query_map([], |row| row.get::<_, String>(3))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(MemoryError::from)
            })
            .unwrap();
        let plan = plans.join(" ").to_ascii_lowercase();
        assert!(plan.contains("index"), "query plan was not indexed: {plan}");
        assert!(!plan.contains("scan record"), "full table scan: {plan}");
    }

    #[test]
    #[ignore = "release-mode M1 certification gate"]
    fn release_insert_lock_budget_at_100k_records() {
        let store = store();
        let started = Instant::now();
        for index in 0..100_000 {
            store
                .append_trusted(record(
                    &format!("record {index}"),
                    "load",
                    RecordKind::Status,
                    None,
                    1,
                ))
                .unwrap();
        }
        let average = started.elapsed().as_nanos() / 100_000;
        assert!(
            average < 1_000_000,
            "average insert lock time was {average}ns (budget <1ms)"
        );
    }
}
