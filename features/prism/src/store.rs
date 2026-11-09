use crate::actions::*;
use crate::model::*;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

const SCHEMA_VERSION: i64 = 2;
const BUSY_TIMEOUT_SECONDS: u64 = 3;

pub struct Store {
    connection: Connection,
}

#[derive(Serialize, Deserialize)]
struct UndoRecord {
    before: Document,
    after: Document,
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
                },
                _ => return Err(Error::Invalid(
                    "existing document requires expected_revision; new document requires neither"
                        .into(),
                )),
            };
        let document = publish_into(document, request)?;
        write_document(&transaction, workspace, &document)?;
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
        let mut after = before.clone();
        apply_action(&mut after, action)?;
        let undo_id = Id::new_v4();
        write_document(&transaction, workspace, &after)?;
        let record = serde_json::to_string(&UndoRecord {
            before,
            after: after.clone(),
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
        });
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
        transaction.execute(
            "UPDATE undo_records SET consumed=1 WHERE workspace_id=?1 AND id=?2",
            params![workspace.to_string(), undo_id.to_string()],
        )?;
        transaction.commit()?;
        Ok(current)
    }

    pub fn save_view(&mut self, workspace: Id, view: &ViewState) -> Result<()> {
        if view.id.is_nil() {
            return Err(Error::Invalid("view ID must not be nil".into()));
        }
        bounded(&view.draft, MAX_TEXT_BYTES, "draft")?;
        bounded(&view.filter, MAX_TITLE_BYTES, "filter")?;
        let content = serialized(view)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let document = read_document(&transaction, workspace, view.document_id)?;
        if view.positions.len() > MAX_SELECTION || view.hidden.len() > MAX_SELECTION {
            return Err(Error::Invalid("view record limit exceeded".into()));
        }
        if let Some(selection) = &view.selection
            && (selection.document_id != document.id
                || selection.revision > document.revision
                || selection.ids.len() > MAX_SELECTION)
        {
            return Err(Error::Invalid("invalid saved selection".into()));
        }
        let existing: Option<String> = transaction
            .query_row(
                "SELECT document_id FROM private_views WHERE workspace_id=?1 AND id=?2",
                params![workspace.to_string(), view.id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some_and(|id| id != view.document_id.to_string()) {
            return Err(Error::Invalid(
                "view cannot change its document identity".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO private_views(workspace_id, document_id, id, content) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(workspace_id, id) DO UPDATE SET content=excluded.content",
            params![workspace.to_string(), view.document_id.to_string(), view.id.to_string(), content],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn views(&self, workspace: Id) -> Result<Vec<ViewState>> {
        require_workspace(&self.connection, workspace)?;
        let mut statement = self
            .connection
            .prepare("SELECT content FROM private_views WHERE workspace_id=?1 ORDER BY id")?;
        let rows = statement.query_map([workspace.to_string()], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    /// UI arrangement is private and is never returned by agent-facing inspection.
    pub fn save_ui_state(&mut self, workspace: Id, state: &serde_json::Value) -> Result<()> {
        let content = serialized(state)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_workspace(&transaction, workspace)?;
        transaction.execute(
            "INSERT INTO private_ui(workspace_id, content) VALUES(?1, ?2)
             ON CONFLICT(workspace_id) DO UPDATE SET content=excluded.content",
            params![workspace.to_string(), content],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn ui_state(&self, workspace: Id) -> Result<Option<serde_json::Value>> {
        require_workspace(&self.connection, workspace)?;
        let content: Option<String> = self
            .connection
            .query_row(
                "SELECT content FROM private_ui WHERE workspace_id=?1",
                [workspace.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        content
            .map(|value| Ok(serde_json::from_str(&value)?))
            .transpose()
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

    /// A send and its private draft clear succeed together; retries cannot clear newer input.
    pub fn prepare_question_and_clear_draft(
        &mut self,
        workspace: Id,
        request: &PrepareQuestion,
        view_id: Id,
    ) -> Result<Question> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (question, created) = insert_question(&transaction, workspace, request)?;
        if created {
            let content: Option<String> = transaction
                .query_row(
                    "SELECT content FROM private_views WHERE workspace_id=?1 AND id=?2",
                    params![workspace.to_string(), view_id.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            let mut view: ViewState = serde_json::from_str(
                &content.ok_or_else(|| Error::NotFound("private view".into()))?,
            )?;
            if view.document_id != request.selection.document_id || view.draft != request.text {
                return Err(Error::Invalid(
                    "private draft changed before send; refresh it".into(),
                ));
            }
            view.draft.clear();
            transaction.execute(
                "UPDATE private_views SET content=?3 WHERE workspace_id=?1 AND id=?2",
                params![
                    workspace.to_string(),
                    view_id.to_string(),
                    serialized(&view)?
                ],
            )?;
        }
        transaction.commit()?;
        Ok(question)
    }

    pub fn question(&self, workspace: Id, request_id: Id) -> Result<Question> {
        read_question(&self.connection, workspace, request_id)
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
        let mut question = read_question(&transaction, workspace, request_id)?;
        if question.generation != generation || question.outcome != *expected {
            return Err(Error::Invalid(
                "delivery generation or outcome changed".into(),
            ));
        }
        validate_transition(expected, &outcome)?;
        question.outcome = outcome;
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
    };
    connection.execute(
            "INSERT INTO questions(workspace_id, request_id, payload_hash, content) VALUES(?1, ?2, ?3, ?4)",
            params![workspace.to_string(), request.request_id.to_string(), payload_hash, serialized(&question)?],
        )?;
    Ok((question, true))
}
