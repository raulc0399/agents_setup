"""Transactional SQLite state and append-only history for one review run."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import sqlite3
import time


SCHEMA_VERSION = 1
BUSY_TIMEOUT_SECONDS = 30

# Schema version 1 has one canonical definition for creation and validation.
SCHEMA_OBJECTS = (
    ('table', 'metadata', 'CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)'),
    ('table', 'records', '''CREATE TABLE records (
        kind TEXT NOT NULL, id TEXT NOT NULL, version INTEGER NOT NULL CHECK(version > 0),
        data TEXT NOT NULL, updated_at REAL NOT NULL, PRIMARY KEY(kind, id))'''),
    ('table', 'events', '''CREATE TABLE events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL,
        record_id TEXT NOT NULL, actor TEXT NOT NULL, payload TEXT NOT NULL, at REAL NOT NULL)'''),
    ('trigger', 'events_no_replace', '''CREATE TRIGGER events_no_replace BEFORE INSERT ON events
        WHEN EXISTS (SELECT 1 FROM events WHERE seq = NEW.seq)
        BEGIN SELECT RAISE(ABORT, 'Events are immutable'); END'''),
    ('trigger', 'events_no_update', '''CREATE TRIGGER events_no_update BEFORE UPDATE ON events
        BEGIN SELECT RAISE(ABORT, 'Events are immutable'); END'''),
    ('trigger', 'events_no_delete', '''CREATE TRIGGER events_no_delete BEFORE DELETE ON events
        BEGIN SELECT RAISE(ABORT, 'Events are immutable'); END'''),
)


class StoreError(RuntimeError):
    """The run database cannot be opened or modified as requested."""


class ConflictError(ValueError):
    """A record no longer has the caller's expected version."""


def _json(value):
    try:
        return json.dumps(value, ensure_ascii=False, allow_nan=False, sort_keys=True)
    except (TypeError, ValueError) as error:
        raise ValueError('Value must be valid JSON') from error


def _name(value, label):
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f'{label} must be a nonempty string')
    return value


class Store:
    def __init__(self, run_dir: Path, create=False, metadata=None, readonly=False):
        self.run_dir = Path(run_dir).resolve()
        self.readonly = readonly
        self._depth = 0
        self._savepoint = 0
        self._closed = False
        if create and readonly:
            raise StoreError('Cannot create a read-only store')
        if metadata is not None and (not create or not isinstance(metadata, dict)):
            raise ValueError('metadata must be an object supplied only when creating a run')
        if metadata is not None:
            _json(metadata)
            for key in metadata:
                _name(key, 'Metadata key')

        database = self.run_dir / 'run.db'
        if create:
            self.run_dir.mkdir(parents=True, exist_ok=True)
            if any(self.run_dir.iterdir()):
                raise StoreError(f'Run directory is not empty: {self.run_dir}')
            try:
                descriptor = os.open(database, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
                os.close(descriptor)
            except OSError as error:
                raise StoreError(f'Cannot create run database: {database}') from error
        elif not database.is_file():
            raise StoreError(f'Run database does not exist: {database}')

        try:
            mode = 'ro' if readonly else 'rw'
            self.connection = sqlite3.connect(
                database.as_uri() + f'?mode={mode}', uri=True,
                isolation_level=None, timeout=BUSY_TIMEOUT_SECONDS)
            self.connection.row_factory = sqlite3.Row
            self.connection.execute('PRAGMA foreign_keys = ON')
            if readonly:
                self.connection.execute('PRAGMA query_only = ON')
            if create:
                self.connection.execute('PRAGMA journal_mode = WAL')
                self._initialize(metadata or {})
            self._validate_schema()
        except (sqlite3.Error, StoreError) as error:
            if hasattr(self, 'connection'):
                self.connection.close()
            raise StoreError(f'Cannot open run database: {database}: {error}') from error

    def _initialize(self, metadata):
        with self.transaction():
            for _, _, sql in SCHEMA_OBJECTS:
                self.connection.execute(sql)
            self.connection.execute(f'PRAGMA user_version = {SCHEMA_VERSION}')
            for key, value in metadata.items():
                self.set_meta(key, value)

    def _validate_schema(self):
        if self.connection.execute('PRAGMA user_version').fetchone()[0] != SCHEMA_VERSION:
            raise StoreError('Unsupported run database schema')
        for object_type, name, expected_sql in SCHEMA_OBJECTS:
            row = self.connection.execute(
                'SELECT sql FROM sqlite_schema WHERE type = ? AND name = ?',
                (object_type, name)).fetchone()
            # Match the pinned definition, including constraints and trigger bodies.
            # Version 1 databases created by this Store retain this exact SQL.
            if row is None or ' '.join(row['sql'].split()) != ' '.join(expected_sql.split()):
                raise StoreError(f'Invalid run database schema: {name}')

    @contextmanager
    def transaction(self):
        """Serialize writers; a failed nested block rolls back only that block."""
        if self._closed:
            raise StoreError('Store is closed')
        if self.readonly:
            raise StoreError('Store is read-only')
        nested = self._depth > 0
        self._savepoint += 1
        savepoint = f'store_{self._savepoint}'
        self.connection.execute(f'SAVEPOINT {savepoint}' if nested else 'BEGIN IMMEDIATE')
        self._depth += 1
        try:
            yield self
            self.connection.execute(f'RELEASE SAVEPOINT {savepoint}' if nested else 'COMMIT')
        except BaseException:
            if nested:
                self.connection.execute(f'ROLLBACK TO SAVEPOINT {savepoint}')
                self.connection.execute(f'RELEASE SAVEPOINT {savepoint}')
            elif self.connection.in_transaction:
                self.connection.execute('ROLLBACK')
            raise
        finally:
            self._depth -= 1

    @contextmanager
    def read_transaction(self):
        """Read one consistent snapshot; callers must not write in this context."""
        if self._closed:
            raise StoreError('Store is closed')
        if self.connection.in_transaction:
            yield self
            return

        self.connection.execute('BEGIN')
        self._depth += 1
        try:
            yield self
            self.connection.execute('COMMIT')
        except BaseException:
            if self.connection.in_transaction:
                self.connection.execute('ROLLBACK')
            raise
        finally:
            self._depth -= 1

    def get_meta(self, key, default=None):
        _name(key, 'Metadata key')
        row = self.connection.execute('SELECT value FROM metadata WHERE key = ?', (key,)).fetchone()
        return default if row is None else json.loads(row['value'])

    def set_meta(self, key, value):
        _name(key, 'Metadata key')
        encoded = _json(value)
        with self.transaction():
            if key in ('policies', 'protocol'):
                row = self.connection.execute('SELECT value FROM metadata WHERE key = ?', (key,)).fetchone()
                if row is not None and row['value'] != encoded:
                    raise StoreError(f'Run {key} is immutable; create a new run')
            self.connection.execute('INSERT INTO metadata(key, value) VALUES (?, ?) '
                                    'ON CONFLICT(key) DO UPDATE SET value = excluded.value', (key, encoded))

    @staticmethod
    def _record(row):
        if row is None:
            return None
        result = dict(row)
        result['data'] = json.loads(result['data'])
        return result

    def get_record(self, kind, id):
        _name(kind, 'Record kind')
        _name(id, 'Record id')
        row = self.connection.execute('SELECT * FROM records WHERE kind = ? AND id = ?', (kind, id)).fetchone()
        return self._record(row)

    def list_records(self, kind):
        _name(kind, 'Record kind')
        rows = self.connection.execute('SELECT * FROM records WHERE kind = ? ORDER BY id', (kind,)).fetchall()
        return [self._record(row) for row in rows]

    def put_record(self, kind, id, data, actor='system', event='record.updated', expected_version=None):
        _name(kind, 'Record kind')
        _name(id, 'Record id')
        _name(actor, 'Actor')
        _name(event, 'Event kind')
        if not isinstance(data, dict):
            raise ValueError('Record data must be a JSON object')
        if expected_version is not None and (type(expected_version) is not int or expected_version < 0):
            raise ValueError('Expected version must be a nonnegative integer')
        encoded = _json(data)
        with self.transaction():
            old = self.get_record(kind, id)
            version = old['version'] if old else 0
            if expected_version is not None and expected_version != version:
                raise ConflictError(f'{kind}/{id}: expected version {expected_version}, found {version}')
            updated_at = time.time()
            self.connection.execute('INSERT INTO records(kind, id, version, data, updated_at) VALUES (?, ?, ?, ?, ?) '
                                    'ON CONFLICT(kind, id) DO UPDATE SET version = excluded.version, '
                                    'data = excluded.data, updated_at = excluded.updated_at',
                                    (kind, id, version + 1, encoded, updated_at))
            self.append_event(event, id, {'kind': kind, 'version': version + 1, 'data': json.loads(encoded)}, actor)
            return self.get_record(kind, id)

    def append_event(self, kind, record_id, payload, actor='system'):
        _name(kind, 'Event kind')
        _name(record_id, 'Record id')
        _name(actor, 'Actor')
        encoded = _json(payload)
        with self.transaction():
            at = time.time()
            cursor = self.connection.execute('INSERT INTO events(kind, record_id, actor, payload, at) '
                                             'VALUES (?, ?, ?, ?, ?)', (kind, record_id, actor, encoded, at))
            return {'seq': cursor.lastrowid, 'kind': kind, 'record_id': record_id,
                    'actor': actor, 'payload': json.loads(encoded), 'at': at}

    def events(self, after=0):
        if type(after) is not int or after < 0:
            raise ValueError('Event cursor must be a nonnegative integer')
        rows = self.connection.execute('SELECT * FROM events WHERE seq > ? ORDER BY seq', (after,)).fetchall()
        return [{**dict(row), 'payload': json.loads(row['payload'])} for row in rows]

    def close(self):
        if not self._closed:
            self.connection.close()
            self._closed = True

    def __enter__(self):
        if self._closed:
            raise StoreError('Store is closed')
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        self.close()
