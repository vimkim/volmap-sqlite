//! Optional, descriptive SQLite metadata. The wire format contains only hashes and numbers.
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::inspection::{CellIdentity, SchemaEvidence, SchemaObjectType};

const REQUEST: &[u8] = b"VOLMAP-SEMANTIC-1\n";
const MAX_COPY: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT: u64 = 256 * 1024;
const MAX_TABLES: usize = 1024;
const TIMEOUT: Duration = Duration::from_secs(2);
const SCHEMA_QUERY: &str = "SELECT type, sql FROM main.sqlite_schema";
const TABLE_QUERY: &str = "PRAGMA main.table_list";
const SETUP_QUERIES: [&str; 3] = [
    "PRAGMA hard_heap_limit=16777216",
    "PRAGMA query_only=ON",
    "PRAGMA temp_store=MEMORY",
];
const SCHEMA_QUERY_BIT: u8 = 1 << 3;
const TABLE_QUERY_BIT: u8 = 1 << 4;
const UNEXPECTED_QUERY_BIT: u8 = 1 << 7;
const EXPECTED_QUERY_MASK: u8 =
    ((1 << SETUP_QUERIES.len()) - 1) | SCHEMA_QUERY_BIT | TABLE_QUERY_BIT;
static QUERY_MASK: AtomicU8 = AtomicU8::new(0);

/// Configurable operational budgets; larger values are clamped to security ceilings.
#[derive(Clone, Copy, Debug)]
pub struct SemanticBudget {
    pub max_copy_bytes: u64,
    pub max_schema_records: usize,
    pub timeout_ms: u64,
    pub max_output_bytes: u64,
}

impl Default for SemanticBudget {
    fn default() -> Self {
        Self {
            max_copy_bytes: MAX_COPY,
            max_schema_records: MAX_TABLES,
            timeout_ms: 2000,
            max_output_bytes: MAX_OUTPUT,
        }
    }
}

impl SemanticBudget {
    fn bounded(self) -> Self {
        let defaults = Self::default();
        Self {
            max_copy_bytes: self.max_copy_bytes.min(defaults.max_copy_bytes),
            max_schema_records: self.max_schema_records.min(defaults.max_schema_records),
            timeout_ms: self.timeout_ms.min(defaults.timeout_ms),
            max_output_bytes: self.max_output_bytes.min(defaults.max_output_bytes),
        }
    }
}

fn trace_query(sql: &str) {
    let sql = sql.trim().trim_end_matches(';');
    let bit = match sql {
        SCHEMA_QUERY => SCHEMA_QUERY_BIT,
        TABLE_QUERY => TABLE_QUERY_BIT,
        _ => SETUP_QUERIES
            .iter()
            .position(|query| *query == sql)
            .map_or(UNEXPECTED_QUERY_BIT, |index| 1 << index),
    };
    QUERY_MASK.fetch_or(bit, Ordering::Relaxed);
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticMetadata {
    pub state: &'static str,
    pub tables: Vec<TableDescription>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableDescription {
    pub identity: CellIdentity,
    pub column_count: u16,
    pub strict: bool,
    pub without_rowid: bool,
}

impl SemanticMetadata {
    pub(crate) fn unavailable() -> Self {
        Self {
            state: "unavailable",
            tables: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u8,
    #[serde(rename = "queryMask")]
    query_mask: u8,
    tables: Vec<WireTable>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTable {
    name_hash: [u8; 32],
    column_count: u16,
    strict: bool,
    without_rowid: bool,
}

pub(crate) fn enrich(
    file: &File,
    schema: &SchemaEvidence,
    budget: SemanticBudget,
    cancelled: impl Fn() -> bool,
) -> SemanticMetadata {
    run(file, schema, budget.bounded(), cancelled).unwrap_or_else(SemanticMetadata::unavailable)
}

fn run(
    file: &File,
    schema: &SchemaEvidence,
    budget: SemanticBudget,
    cancelled: impl Fn() -> bool,
) -> Option<SemanticMetadata> {
    let start = Instant::now();
    let timeout = Duration::from_millis(budget.timeout_ms);
    let length = file.metadata().ok()?.len();
    if length > budget.max_copy_bytes
        || schema.state != crate::inspection::SchemaState::Complete
        || budget.max_schema_records == 0
        || schema.objects.len() > budget.max_schema_records
        || budget.timeout_ms == 0
        || budget.max_output_bytes == 0
        || cancelled()
    {
        return None;
    }
    let directory = tempfile::tempdir().ok()?;
    let mut copy = File::create(directory.path().join("snapshot.sqlite")).ok()?;
    let mut offset = 0;
    let mut buffer = [0; 16384];
    while offset < length {
        if cancelled() || start.elapsed() >= timeout {
            return None;
        }
        let count = usize::try_from((length - offset).min(buffer.len() as u64)).ok()?;
        file.read_exact_at(&mut buffer[..count], offset).ok()?;
        copy.write_all(&buffer[..count]).ok()?;
        offset += count as u64;
    }
    drop(copy);
    let mut child = Command::new(std::env::current_exe().ok()?)
        .arg("--private-semantic-helper")
        .current_dir(directory.path())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Always reap the child, including write, timeout and protocol failures.
    let input_ok = child
        .stdin
        .take()
        .is_some_and(|mut pipe| pipe.write_all(REQUEST).is_ok());
    let stdout = child.stdout.take()?;
    let reader = std::thread::Builder::new().spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(budget.max_output_bytes + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(bytes)
    });
    let Ok(reader) = reader else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let success = loop {
        if !input_ok || cancelled() || start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let bytes = reader.join().ok()??;
    if !success || bytes.len() as u64 > budget.max_output_bytes {
        return None;
    }
    if cancelled() || start.elapsed() >= timeout {
        return None;
    }
    validate_reply(schema, &bytes)
}

fn validate_reply(schema: &SchemaEvidence, bytes: &[u8]) -> Option<SemanticMetadata> {
    let reply: Reply = serde_json::from_slice(bytes).ok()?;
    if reply.query_mask != EXPECTED_QUERY_MASK
        || reply.version != 1
        || reply.tables.len() > MAX_TABLES
    {
        return None;
    }
    let mut known = std::collections::BTreeMap::new();
    for object in &schema.objects {
        if object.object_type == Some(SchemaObjectType::Table) {
            let hash: [u8; 32] = Sha256::digest(object.name.as_ref()?.as_bytes()).into();
            if known.insert(hash, object).is_some() {
                return None;
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut tables = Vec::new();
    for table in reply.tables {
        if table.column_count == 0 || table.column_count > 256 || !seen.insert(table.name_hash) {
            return None;
        }
        let object = known.get(&table.name_hash)?;
        object.root.as_ref()?;
        tables.push(TableDescription {
            identity: object.identity.clone(),
            column_count: table.column_count,
            strict: table.strict,
            without_rowid: table.without_rowid,
        });
    }
    if tables.len() != known.len() {
        return None;
    }
    Some(SemanticMetadata {
        state: "available",
        tables,
    })
}

/// Private executable entry point; accepts a fixed version token, never SQL or paths.
/// Returns only a process exit code. Errors and SQLite messages never cross the protocol.
#[doc(hidden)]
#[must_use]
pub fn run_private_helper() -> i32 {
    match helper() {
        Some(()) => 0,
        None => 1,
    }
}

fn helper() -> Option<()> {
    constrain_process()?;
    if std::env::args_os().len() != 2 {
        return None;
    }
    let mut request = Vec::new();
    std::io::stdin()
        .take(REQUEST.len() as u64 + 1)
        .read_to_end(&mut request)
        .ok()?;
    if request != REQUEST {
        return None;
    }
    // The parent creates a fresh private directory. Refuse any unexpected entries.
    let entries: Vec<_> = std::fs::read_dir(".")
        .ok()?
        .collect::<Result<_, _>>()
        .ok()?;
    if entries.len() != 1
        || entries[0].file_name() != "snapshot.sqlite"
        || !entries[0].file_type().ok()?.is_file()
    {
        return None;
    }
    let db = Connection::open_with_flags(
        "file:snapshot.sqlite?immutable=1&mode=ro",
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    QUERY_MASK.store(0, Ordering::Relaxed);
    db.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(|event| {
            if let rusqlite::trace::TraceEvent::Stmt(_, sql) = event {
                trace_query(sql);
            }
        }),
    );
    constrain(&db).ok()?;
    // Schema reads do not connect virtual tables. Refuse before table_list, which can do so.
    let mut statement = db.prepare(SCHEMA_QUERY).ok()?;
    if !statement.readonly() {
        return None;
    }
    let mut rows = statement.query([]).ok()?;
    let mut count = 0;
    while let Some(row) = rows.next().ok()? {
        count += 1;
        if count > MAX_TABLES {
            return None;
        }
        let kind: String = row.get(0).ok()?;
        let declaration: Option<String> = row.get(1).ok()?;
        if kind == "view"
            || declaration.is_some_and(|sql| sql.to_ascii_uppercase().contains("VIRTUAL"))
        {
            return None;
        }
    }
    drop(rows);
    drop(statement);
    let mut statement = db.prepare(TABLE_QUERY).ok()?;
    if !statement.readonly() {
        return None;
    }
    let mut rows = statement.query([]).ok()?;
    let mut tables = Vec::new();
    while let Some(row) = rows.next().ok()? {
        let name: String = row.get(1).ok()?;
        if name == "sqlite_schema" {
            continue;
        }
        if row.get::<_, String>(2).ok()? != "table" || tables.len() >= MAX_TABLES {
            return None;
        }
        tables.push(WireTable {
            name_hash: Sha256::digest(name.as_bytes()).into(),
            column_count: row.get(3).ok()?,
            without_rowid: row.get(4).ok()?,
            strict: row.get(5).ok()?,
        });
    }
    let query_mask = QUERY_MASK.load(Ordering::Relaxed);
    if query_mask != EXPECTED_QUERY_MASK {
        return None;
    }
    let bytes = serde_json::to_vec(&Reply {
        version: 1,
        query_mask,
        tables,
    })
    .ok()?;
    if bytes.len() as u64 > MAX_OUTPUT {
        return None;
    }
    std::io::stdout().write_all(&bytes).ok()?;
    Some(())
}

fn constrain(db: &Connection) -> rusqlite::Result<()> {
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    db.busy_timeout(Duration::ZERO)?;
    for (limit, value) in [
        (Limit::SQLITE_LIMIT_LENGTH, 65536),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, 65536),
        (Limit::SQLITE_LIMIT_COLUMN, 256),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, 32),
        (Limit::SQLITE_LIMIT_COMPOUND_SELECT, 8),
        (Limit::SQLITE_LIMIT_VDBE_OP, 10000),
        (Limit::SQLITE_LIMIT_FUNCTION_ARG, 16),
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 8),
        (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 0),
        (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
    ] {
        db.set_limit(limit, value)?;
    }
    // This fixed setup pragma runs before any schema read, in the isolated process only.
    for query in SETUP_QUERIES {
        db.execute_batch(query)?;
    }
    let start = Instant::now();
    let mut ticks = 0;
    db.progress_handler(
        100,
        Some(move || {
            ticks += 1;
            ticks > 1000 || start.elapsed() >= TIMEOUT
        }),
    );
    db.authorizer(Some(|context: AuthContext<'_>| {
        let allowed = context.accessor.is_none()
            && match context.action {
                AuthAction::Select => true,
                AuthAction::Read {
                    table_name: "sqlite_master" | "sqlite_schema",
                    column_name: "type" | "sql",
                }
                | AuthAction::Pragma {
                    pragma_name: "table_list",
                    pragma_value: None,
                } => context.database_name == Some("main"),
                _ => false,
            };
        if allowed {
            Authorization::Allow
        } else {
            Authorization::Deny
        }
    }));
    Ok(())
}

fn constrain_process() -> Option<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    for (resource, ceiling) in [
        (Resource::As, 256 * 1024 * 1024),
        (Resource::Cpu, 2),
        (Resource::Core, 0),
        (Resource::Fsize, 0),
    ] {
        let existing = getrlimit(resource);
        let ceiling = existing.maximum.map_or(ceiling, |value| value.min(ceiling));
        setrlimit(
            resource,
            Rlimit {
                current: Some(ceiling),
                maximum: Some(ceiling),
            },
        )
        .ok()?;
    }
    Some(())
}
