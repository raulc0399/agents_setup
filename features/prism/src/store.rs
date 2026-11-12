use crate::actions::*;
use crate::model::*;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

const SCHEMA_VERSION: i64 = 2;
/// How long a connection waits for another one's write lock on the store.
pub const BUSY_TIMEOUT_SECONDS: u64 = 3;

pub struct Store {
    connection: Connection,
}

#[derive(Serialize, Deserialize)]
struct UndoRecord {
    before: Document,
    after: Document,
    /// Closed state of the document before and after; records written before documents could
    /// close have neither and change nothing here.
    #[serde(default)]
    before_closed: bool,
    #[serde(default)]
    after_closed: bool,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(BUSY_TIMEOUT_SECONDS))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if !(0..=SCHEMA_VERSION).contains(&version) {
            return Err(Error::SchemaVersion(version));
        }
        // `private_views` and `private_ui` held the terminal UI's view state, which now lives
        // in the desktop app. Nothing reads or writes them; they stay so that schema 2 remains
        // the same for an older `prism` binary that opens this project's store.
        if version == 0 {
            transaction.execute_batch(
                "CREATE TABLE workspaces (
                    id TEXT PRIMARY KEY, identity TEXT NOT NULL UNIQUE);
                 CREATE TABLE documents (
                    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                    id TEXT NOT NULL, revision INTEGER NOT NULL, content TEXT NOT NULL,
                    PRIMARY KEY(workspace_id, id));
                 CREATE TABLE publications (
                    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                    request_id TEXT NOT NULL, payload_hash TEXT NOT NULL, result TEXT NOT NULL,
                    PRIMARY KEY(workspace_id, request_id));
                 CREATE TABLE undo_records (
                    workspace_id TEXT NOT NULL, document_id TEXT NOT NULL,
                    id TEXT NOT NULL, content TEXT NOT NULL, consumed INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY(workspace_id, id),
                    FOREIGN KEY(workspace_id, document_id) REFERENCES documents(workspace_id, id));
                 CREATE TABLE private_views (
                    workspace_id TEXT NOT NULL, document_id TEXT NOT NULL,
                    id TEXT NOT NULL, content TEXT NOT NULL,
                    PRIMARY KEY(workspace_id, id),
                    FOREIGN KEY(workspace_id, document_id) REFERENCES documents(workspace_id, id));
                 CREATE TABLE questions (
                    workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                    request_id TEXT NOT NULL, payload_hash TEXT NOT NULL, content TEXT NOT NULL,
                    PRIMARY KEY(workspace_id, request_id));",
            )?;
        }
        if version < 2 {
            transaction.execute_batch(
                "CREATE TABLE private_ui (
                    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id),
                    content TEXT NOT NULL);",
            )?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        // Added within schema 2: an older binary reads and writes `documents` only by explicit
        // columns, so it neither sees nor clears `closed`, and its upsert keeps the value.
        let has_closed: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('documents') WHERE name='closed')",
            [],
            |row| row.get(0),
        )?;
        if !has_closed {
            transaction.execute_batch(
                "ALTER TABLE documents ADD COLUMN closed INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        transaction.commit()?;
        Ok(Self { connection })
    }

    /// The caller supplies a canonical project identity; live tokens never enter this key.
    pub fn workspace(&mut self, key: &WorkspaceKey) -> Result<Workspace> {
        for (field, value) in [
            ("project", &key.project),
            ("conversation", &key.conversation),
        ] {
            bounded(value, MAX_TEXT_BYTES, field)?;
            if value.trim().is_empty() {
                return Err(Error::Invalid(format!("{field} must not be empty")));
            }
        }
        let identity = serialized(key)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate = Id::new_v4();
        transaction.execute(
            "INSERT INTO workspaces(id, identity) VALUES(?1, ?2) ON CONFLICT(identity) DO NOTHING",
            params![candidate.to_string(), identity],
        )?;
        let id: String = transaction.query_row(
            "SELECT id FROM workspaces WHERE identity=?1",
            [&identity],
            |row| row.get(0),
        )?;
        let id = Id::parse_str(&id)
            .map_err(|_| Error::Invalid("stored workspace ID is invalid".into()))?;
        transaction.commit()?;
        Ok(Workspace {
            id,
            key: key.clone(),
        })
    }

    pub fn document(&self, workspace: Id, document: Id) -> Result<Document> {
        read_document(&self.connection, workspace, document)
    }

    /// Agent-safe inspection: excludes local layouts, selections, and drafts.
    pub fn documents(&self, workspace: Id) -> Result<Vec<Document>> {
        require_workspace(&self.connection, workspace)?;
        let mut statement = self
            .connection
            .prepare("SELECT content FROM documents WHERE workspace_id=?1 ORDER BY id")?;
        let rows = statement.query_map([workspace.to_string()], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// IDs of the closed documents, in ID order.
    pub fn closed_documents(&self, workspace: Id) -> Result<Vec<Id>> {
        require_workspace(&self.connection, workspace)?;
        let mut statement = self
            .connection
            .prepare("SELECT id FROM documents WHERE workspace_id=?1 AND closed=1 ORDER BY id")?;
        let rows = statement.query_map([workspace.to_string()], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            Id::parse_str(&row?).map_err(|_| Error::Invalid("stored document ID is invalid".into()))
        })
        .collect()
    }

    pub fn is_closed(&self, workspace: Id, document: Id) -> Result<bool> {
        read_closed(&self.connection, workspace, document)
    }

    pub fn publish(&mut self, workspace: Id, request: &PublishRequest) -> Result<PublishResult> {
        let payload = serialized(request)?;
        let payload_hash = hash(&payload);
        if request.request_id.is_nil() {
            return Err(Error::Invalid("request ID must not be nil".into()));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_workspace(&transaction, workspace)?;
        let replay: Option<(String, String)> = transaction.query_row(
            "SELECT payload_hash, result FROM publications WHERE workspace_id=?1 AND request_id=?2",
            params![workspace.to_string(), request.request_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        if let Some((existing_hash, result)) = replay {
            if existing_hash != payload_hash {
                return Err(Error::RequestConflict);
            }
            return Ok(serde_json::from_str(&result)?);
        }
        let document =
            match (request.document_id, request.expected_revision) {
                (Some(id), Some(expected)) => {
                    let document = read_document(&transaction, workspace, id)?;
                    check_revision(&document, expected)?;
                    document
                }
                (None, None) => Document {
                    id: Id::new_v4(),
                    title: String::new(),
                    revision: 0,
                    elements: BTreeMap::new(),
                    relationships: BTreeMap::new(),
                    groups: BTreeMap::new(),
                    lanes: None,
                },
                _ => return Err(Error::Invalid(
                    "existing document requires expected_revision; new document requires neither"
                        .into(),
                )),
            };
        let reopens = match request.document_id {
            Some(id) => read_closed(&transaction, workspace, id)?,
            None => false,
        };
        let document = publish_into(document, request)?;
        write_document(&transaction, workspace, &document)?;
        // The agent revising a closed document brings it back for everyone.
        if reopens {
            write_closed(&transaction, workspace, document.id, false)?;
            consume_close_records(&transaction, workspace, document.id)?;
        }
        let result = PublishResult {
            document_id: document.id,
            revision: document.revision,
            element_ids: request.elements.iter().map(|v| v.id).collect(),
            relationship_ids: request.relationships.iter().map(|v| v.id).collect(),
            group_ids: request.groups.iter().map(|v| v.id).collect(),
        };
        transaction.execute(
            "INSERT INTO publications(workspace_id, request_id, payload_hash, result) VALUES(?1, ?2, ?3, ?4)",
            params![workspace.to_string(), request.request_id.to_string(), payload_hash, serialized(&result)?],
        )?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn apply(
        &mut self,
        workspace: Id,
        document: Id,
        expected_revision: u64,
        action: &ContentAction,
    ) -> Result<MutationResult> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = read_document(&transaction, workspace, document)?;
        check_revision(&before, expected_revision)?;
        let before_closed = read_closed(&transaction, workspace, document)?;
        let after_closed = closed_by(action).unwrap_or(before_closed);
        if closed_by(action) == Some(before_closed) {
            let state = if before_closed { "closed" } else { "open" };
            return Err(Error::Invalid(format!("document is already {state}")));
        }
        let mut after = before.clone();
        apply_action(&mut after, action)?;
        let undo_id = Id::new_v4();
        write_document(&transaction, workspace, &after)?;
        write_closed(&transaction, workspace, document, after_closed)?;
        let record = serde_json::to_string(&UndoRecord {
            before,
            after: after.clone(),
            before_closed,
            after_closed,
        })?;
        transaction.execute(
            "INSERT INTO undo_records(workspace_id, document_id, id, content) VALUES(?1, ?2, ?3, ?4)",
            params![workspace.to_string(), document.to_string(), undo_id.to_string(), record],
        )?;
        transaction.commit()?;
        Ok(MutationResult {
            revision: after.revision,
            undo_id,
        })
    }

    /// Unconsumed undo records for a document, oldest first.
    pub fn undo_records(&mut self, workspace: Id, document: Id) -> Result<Vec<Id>> {
        let mut statement = self.connection.prepare(
            "SELECT id FROM undo_records WHERE workspace_id=?1 AND document_id=?2 AND consumed=0 ORDER BY rowid",
        )?;
        let rows = statement.query_map(
            params![workspace.to_string(), document.to_string()],
            |row| row.get::<_, String>(0),
        )?;
        rows.map(|row| Id::parse_str(&row?).map_err(|_| Error::Invalid("undo record ID".into())))
            .collect()
    }

    /// Undoes the latest action on the document that can still be undone. The revision is
    /// checked first, so a resent undo that already committed reports a conflict.
    pub fn undo_latest(
        &mut self,
        workspace: Id,
        document: Id,
        expected_revision: u64,
    ) -> Result<Document> {
        check_revision(&self.document(workspace, document)?, expected_revision)?;
        let undo_id = self
            .undo_records(workspace, document)?
            .pop()
            .ok_or_else(|| Error::NotFound("action to undo for this document".into()))?;
        self.undo(workspace, document, expected_revision, undo_id)
    }

    pub fn undo(
        &mut self,
        workspace: Id,
        document: Id,
        expected_revision: u64,
        undo_id: Id,
    ) -> Result<Document> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut current = read_document(&transaction, workspace, document)?;
        check_revision(&current, expected_revision)?;
        let record: Option<String> = transaction.query_row(
            "SELECT content FROM undo_records WHERE workspace_id=?1 AND document_id=?2 AND id=?3 AND consumed=0",
            params![workspace.to_string(), document.to_string(), undo_id.to_string()], |row| row.get(0),
        ).optional()?;
        let record: UndoRecord = serde_json::from_str(
            &record.ok_or_else(|| Error::NotFound("available undo record".into()))?,
        )?;
        let mut closed = read_closed(&transaction, workspace, document)?;
        let restored = restore_changed(
            &mut current.elements,
            &record.before.elements,
            &record.after.elements,
        )
        .and_then(|()| {
            restore_changed(
                &mut current.relationships,
                &record.before.relationships,
                &record.after.relationships,
            )
        })
        .and_then(|()| {
            restore_changed(
                &mut current.groups,
                &record.before.groups,
                &record.after.groups,
            )
        })
        .and_then(|()| restore_closed(&mut closed, &record));
        if let Err(Error::UndoConflict) = restored {
            // Later changes overwrote this record's effect; it can never apply, so discard it
            // instead of letting it block every later undo.
            transaction.execute(
                "UPDATE undo_records SET consumed=1 WHERE workspace_id=?1 AND id=?2",
                params![workspace.to_string(), undo_id.to_string()],
            )?;
            transaction.commit()?;
            return Err(Error::UndoConflict);
        }
        restored?;
        current.revision += 1;
        validate_document(&current)?;
        write_document(&transaction, workspace, &current)?;
        write_closed(&transaction, workspace, document, closed)?;
        transaction.execute(
            "UPDATE undo_records SET consumed=1 WHERE workspace_id=?1 AND id=?2",
            params![workspace.to_string(), undo_id.to_string()],
        )?;
        transaction.commit()?;
        Ok(current)
    }

    pub fn prepare_question(
        &mut self,
        workspace: Id,
        request: &PrepareQuestion,
    ) -> Result<Question> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (question, _) = insert_question(&transaction, workspace, request)?;
        transaction.commit()?;
        Ok(question)
    }

    pub fn question(&self, workspace: Id, request_id: Id) -> Result<Question> {
        read_question(&self.connection, workspace, request_id)
    }

    /// Questions explicitly shared for CLI retrieval; no other question leaves the store.
    pub fn shared_questions(&self, workspace: Id) -> Result<Vec<Question>> {
        Ok(self
            .questions(workspace)?
            .into_iter()
            .filter(|question| question.mode == DeliveryMode::Share)
            .collect())
    }

    pub fn questions(&self, workspace: Id) -> Result<Vec<Question>> {
        require_workspace(&self.connection, workspace)?;
        let mut statement = self
            .connection
            .prepare("SELECT content FROM questions WHERE workspace_id=?1 ORDER BY request_id")?;
        let rows = statement.query_map([workspace.to_string()], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// Compare-and-set prevents two dispatchers claiming Pending. Persist Submitted before I/O.
    /// Unknown never returns to Pending: an explicit retry uses a new request ID.
    pub fn update_delivery(
        &mut self,
        workspace: Id,
        request_id: Id,
        generation: u64,
        expected: &DeliveryOutcome,
        outcome: DeliveryOutcome,
    ) -> Result<Question> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let question = transition(
            &transaction,
            workspace,
            request_id,
            generation,
            expected,
            outcome,
        )?;
        transaction.commit()?;
        Ok(question)
    }

    /// A pending question the mod submitted as a prompt of its own: `submitted`, then
    /// `queued` without a reason, as the prompt may wait behind a turn and its start is not
    /// confirmed. One commit, so no reader sees it `submitted`, which means "in the prompt box".
    pub fn submit_as_prompt(
        &mut self,
        workspace: Id,
        request_id: Id,
        generation: u64,
    ) -> Result<Question> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transition(
            &transaction,
            workspace,
            request_id,
            generation,
            &DeliveryOutcome::Pending,
            DeliveryOutcome::Submitted,
        )?;
        let question = transition(
            &transaction,
            workspace,
            request_id,
            generation,
            &DeliveryOutcome::Submitted,
            DeliveryOutcome::Queued { reason: None },
        )?;
        transaction.commit()?;
        Ok(question)
    }

    /// Records the agent's reply beside the shared question; a later answer replaces it.
    pub fn answer_question(
        &mut self,
        workspace: Id,
        request_id: Id,
        answer: &str,
    ) -> Result<Question> {
        bounded(answer, MAX_TEXT_BYTES, "answer")?;
        if answer.trim().is_empty() {
            return Err(Error::Invalid("answer requires text".into()));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut question = read_question(&transaction, workspace, request_id)?;
        question.answer = Some(answer.to_owned());
        transaction.execute(
            "UPDATE questions SET content=?3 WHERE workspace_id=?1 AND request_id=?2",
            params![
                workspace.to_string(),
                request_id.to_string(),
                serialized(&question)?
            ],
        )?;
        transaction.commit()?;
        Ok(question)
    }

    /// Call when a live binding retires. Persisted in-flight work is never automatically resent.
    pub fn invalidate_generation(
        &mut self,
        workspace: Id,
        generation: u64,
        reason: &str,
    ) -> Result<usize> {
        bounded(reason, MAX_TEXT_BYTES, "delivery reason")?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_workspace(&transaction, workspace)?;
        let questions = {
            let mut statement =
                transaction.prepare("SELECT content FROM questions WHERE workspace_id=?1")?;
            let rows =
                statement.query_map([workspace.to_string()], |row| row.get::<_, String>(0))?;
            rows.map(|row| Ok(serde_json::from_str::<Question>(&row?)?))
                .collect::<Result<Vec<_>>>()?
        };
        let mut count = 0;
        for mut question in questions {
            // Share is already durable and has no native transport left to invalidate.
            if question.generation != generation || question.mode == DeliveryMode::Share {
                continue;
            }
            question.outcome = match question.outcome {
                DeliveryOutcome::Pending => DeliveryOutcome::Failed {
                    reason: reason.into(),
                },
                DeliveryOutcome::Submitted | DeliveryOutcome::Queued { .. } => {
                    DeliveryOutcome::Unknown {
                        reason: reason.into(),
                    }
                }
                _ => continue,
            };
            transaction.execute(
                "UPDATE questions SET content=?3 WHERE workspace_id=?1 AND request_id=?2",
                params![
                    workspace.to_string(),
                    question.request_id.to_string(),
                    serialized(&question)?
                ],
            )?;
            count += 1;
        }
        transaction.commit()?;
        Ok(count)
    }
}

fn hash(payload: &str) -> String {
    format!("{:x}", Sha256::digest(payload.as_bytes()))
}

fn require_workspace(connection: &Connection, workspace: Id) -> Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=?1)",
        [workspace.to_string()],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(Error::NotFound("workspace".into()));
    }
    Ok(())
}

fn read_document(connection: &Connection, workspace: Id, document: Id) -> Result<Document> {
    let content: Option<String> = connection
        .query_row(
            "SELECT content FROM documents WHERE workspace_id=?1 AND id=?2",
            params![workspace.to_string(), document.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(serde_json::from_str(
        &content.ok_or_else(|| Error::NotFound("document".into()))?,
    )?)
}

fn write_document(connection: &Connection, workspace: Id, document: &Document) -> Result<()> {
    let revision =
        i64::try_from(document.revision).map_err(|_| Error::Invalid("revision overflow".into()))?;
    connection.execute(
        "INSERT INTO documents(workspace_id, id, revision, content) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(workspace_id, id) DO UPDATE SET revision=excluded.revision, content=excluded.content",
        params![workspace.to_string(), document.id.to_string(), revision, serialized(document)?],
    )?;
    Ok(())
}

fn read_closed(connection: &Connection, workspace: Id, document: Id) -> Result<bool> {
    let closed: Option<bool> = connection
        .query_row(
            "SELECT closed FROM documents WHERE workspace_id=?1 AND id=?2",
            params![workspace.to_string(), document.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    closed.ok_or_else(|| Error::NotFound("document".into()))
}

fn write_closed(connection: &Connection, workspace: Id, document: Id, closed: bool) -> Result<()> {
    connection.execute(
        "UPDATE documents SET closed=?3 WHERE workspace_id=?1 AND id=?2",
        params![workspace.to_string(), document.to_string(), closed],
    )?;
    Ok(())
}

fn read_question(connection: &Connection, workspace: Id, request: Id) -> Result<Question> {
    let content: Option<String> = connection
        .query_row(
            "SELECT content FROM questions WHERE workspace_id=?1 AND request_id=?2",
            params![workspace.to_string(), request.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(serde_json::from_str(
        &content.ok_or_else(|| Error::NotFound("question".into()))?,
    )?)
}

/// Publishing reopened the document, so no close can be undone any more: its records are
/// consumed, and the next undo reaches the action before them.
fn consume_close_records(connection: &Connection, workspace: Id, document: Id) -> Result<()> {
    let records = {
        let mut statement = connection.prepare(
            "SELECT id, content FROM undo_records WHERE workspace_id=?1 AND document_id=?2 AND consumed=0",
        )?;
        let rows = statement.query_map(
            params![workspace.to_string(), document.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, content) in records {
        let record: UndoRecord = serde_json::from_str(&content)?;
        if record.before_closed || !record.after_closed {
            continue;
        }
        connection.execute(
            "UPDATE undo_records SET consumed=1 WHERE workspace_id=?1 AND id=?2",
            params![workspace.to_string(), id],
        )?;
    }
    Ok(())
}

/// Undoes a close or reopen, unless something since changed the closed state again.
fn restore_closed(closed: &mut bool, record: &UndoRecord) -> Result<()> {
    if record.before_closed == record.after_closed {
        return Ok(());
    }
    if *closed != record.after_closed {
        return Err(Error::UndoConflict);
    }
    *closed = record.before_closed;
    Ok(())
}

fn restore_changed<T: Clone + PartialEq>(
    current: &mut BTreeMap<Id, T>,
    before: &BTreeMap<Id, T>,
    after: &BTreeMap<Id, T>,
) -> Result<()> {
    for (id, previous) in before {
        if after.get(id) == Some(previous) {
            continue;
        }
        if current.get(id) != after.get(id) {
            return Err(Error::UndoConflict);
        }
        current.insert(*id, previous.clone());
    }
    Ok(())
}

/// Moves a question of `generation` from `expected` to `outcome` inside `connection`'s
/// transaction; refused when either changed meanwhile or the transition is not allowed.
fn transition(
    connection: &Connection,
    workspace: Id,
    request_id: Id,
    generation: u64,
    expected: &DeliveryOutcome,
    outcome: DeliveryOutcome,
) -> Result<Question> {
    let mut question = read_question(connection, workspace, request_id)?;
    if question.generation != generation || question.outcome != *expected {
        return Err(Error::Invalid(
            "delivery generation or outcome changed".into(),
        ));
    }
    validate_transition(expected, &outcome)?;
    question.outcome = outcome;
    connection.execute(
        "UPDATE questions SET content=?3 WHERE workspace_id=?1 AND request_id=?2",
        params![
            workspace.to_string(),
            request_id.to_string(),
            serialized(&question)?
        ],
    )?;
    Ok(question)
}

fn validate_transition(previous: &DeliveryOutcome, next: &DeliveryOutcome) -> Result<()> {
    use DeliveryOutcome::*;
    let allowed = match previous {
        Pending => matches!(
            next,
            Submitted | Failed { .. } | Refused { .. } | Dropped { .. }
        ),
        Submitted => matches!(
            next,
            Queued { .. }
                | Started { .. }
                | Failed { .. }
                | Refused { .. }
                | Dropped { .. }
                | Unknown { .. }
        ),
        Queued { .. } | Unknown { .. } => matches!(
            next,
            Started { .. } | Failed { .. } | Refused { .. } | Dropped { .. } | Unknown { .. }
        ),
        Started { .. } | Failed { .. } | Refused { .. } | Dropped { .. } => false,
    };
    if !allowed {
        return Err(Error::Invalid("invalid delivery transition".into()));
    }
    match next {
        Started { turn_id } => {
            bounded(turn_id, MAX_TITLE_BYTES, "turn ID")?;
            if turn_id.is_empty() {
                return Err(Error::Invalid(
                    "started requires authoritative turn ID".into(),
                ));
            }
        }
        Failed { reason } | Refused { reason } | Dropped { reason } | Unknown { reason } => {
            bounded(reason, MAX_TEXT_BYTES, "delivery reason")?
        }
        Queued {
            reason: Some(reason),
        } => bounded(reason, MAX_TEXT_BYTES, "delivery reason")?,
        _ => {}
    }
    Ok(())
}

fn insert_question(
    connection: &Connection,
    workspace: Id,
    request: &PrepareQuestion,
) -> Result<(Question, bool)> {
    bounded(&request.text, MAX_TEXT_BYTES, "question")?;
    if request.text.trim().is_empty() || request.request_id.is_nil() || request.generation == 0 {
        return Err(Error::Invalid(
            "question requires text, request ID, and nonzero generation".into(),
        ));
    }
    let payload_hash = hash(&serialized(request)?);
    let existing: Option<(String, String)> = connection
        .query_row(
            "SELECT payload_hash, content FROM questions WHERE workspace_id=?1 AND request_id=?2",
            params![workspace.to_string(), request.request_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((previous_hash, content)) = existing {
        if previous_hash != payload_hash {
            return Err(Error::RequestConflict);
        }
        return Ok((serde_json::from_str(&content)?, false));
    }
    let document = read_document(connection, workspace, request.selection.document_id)?;
    let question = Question {
        request_id: request.request_id,
        workspace_id: workspace,
        generation: request.generation,
        snapshot: snapshot(&document, &request.selection)?,
        text: request.text.clone(),
        mode: request.mode.clone(),
        outcome: DeliveryOutcome::Pending,
        answer: None,
        intent: request.intent,
        quoted: request.quoted,
    };
    connection.execute(
            "INSERT INTO questions(workspace_id, request_id, payload_hash, content) VALUES(?1, ?2, ?3, ?4)",
            params![workspace.to_string(), request.request_id.to_string(), payload_hash, serialized(&question)?],
        )?;
    Ok((question, true))
}
