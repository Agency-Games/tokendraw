use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::thread;

use rusqlite::params;
use serde_json::json;
use tempfile::TempDir;

use super::ingest::snapshot;
use super::*;
use crate::adapters::Definition;
use crate::adapters::claude_code::ClaudeCode;
use crate::adapters::codex::Codex;
use crate::adapters::pi::Pi;
use crate::adapters::user::declared::Declared;
use crate::adapters::user::external;
use crate::adapters::user::{self, UserAdapter};
use crate::quantity::Tokens;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Fixture {
    dir: TempDir,
    adapter: ClaudeCode,
}

impl Fixture {
    fn new() -> io::Result<Self> {
        let dir = tempfile::tempdir()?;
        let adapter = ClaudeCode::new(dir.path().join("projects"));
        fs::create_dir_all(dir.path().join("projects/-work"))?;
        Ok(Self { dir, adapter })
    }

    fn log(&self, name: &str) -> PathBuf {
        self.dir.path().join("projects/-work").join(name)
    }

    fn ledger(&self) -> Result<Ledger, Error> {
        Ledger::open(&self.dir.path().join("state/ledger.sqlite"))
    }

    fn ingest(&self) -> Result<Ledger, Error> {
        let mut ledger = self.ledger()?;
        ledger.ingest(&self.adapter)?;
        Ok(ledger)
    }
}

fn call(id: &str, timestamp: &str, session: &str, output: u64) -> String {
    let line = json!({
        "type": "assistant",
        "sessionId": session,
        "timestamp": timestamp,
        "cwd": "/work",
        "entrypoint": "cli",
        "message": {
            "id": id,
            "model": "claude-opus-5-5",
            "usage": {
                "input_tokens": 1,
                "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 100,
                "output_tokens": output,
            }
        }
    });
    format!("{line}\n")
}

fn append(path: &Path, text: &str) -> io::Result<()> {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())
}

fn totals(ledger: &Ledger) -> Result<(u64, u64, u64), Error> {
    let calls = ledger.calls(None, None, None)?;
    Ok(calls.first().map_or((0, 0, 0), |entry| {
        (
            entry.calls,
            entry.tokens.output.0,
            entry.tokens.cache_read.0,
        )
    }))
}

#[test]
fn appended_lines_are_read_once_across_ingestions() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-09-01T10:00:00Z", "s", 5))?;
    fixture.ingest()?;
    append(&log, &call("m2", "2026-09-01T10:01:00Z", "s", 7))?;
    fixture.ingest()?;
    let ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (2, 12, 200));
    let sources = ledger.sources()?;
    assert_eq!(sources.first().map(|entry| entry.files), Some(1));
    Ok(())
}

#[test]
fn usage_that_grows_between_ingestions_updates_the_call() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-09-01T10:00:00Z", "s", 5))?;
    fixture.ingest()?;
    append(&log, &call("m1", "2026-09-01T10:00:02Z", "s", 40))?;
    let ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (1, 40, 100));
    Ok(())
}

#[test]
fn the_same_message_in_two_files_is_one_call_from_the_earliest_copy() -> TestResult {
    let fixture = Fixture::new()?;
    append(
        &fixture.log("resumed.jsonl"),
        &call("m1", "2026-09-02T09:00:00Z", "later", 5),
    )?;
    append(
        &fixture.log("original.jsonl"),
        &call("m1", "2026-09-01T10:00:00Z", "original", 5),
    )?;
    let ledger = fixture.ingest()?;
    let calls = ledger.calls(None, None, None)?;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].calls, 1);
    assert_eq!(calls[0].earliest, "2026-09-01T10:00:00Z".parse()?);
    let session: String = ledger
        .conn
        .query_row("SELECT session_id FROM calls", [], |row| row.get(0))?;
    assert_eq!(session, "original");
    Ok(())
}

#[test]
fn a_deleted_source_file_keeps_its_calls() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-09-01T10:00:00Z", "s", 5))?;
    fixture.ingest()?;
    fs::remove_file(&log)?;
    let ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (1, 5, 100));
    Ok(())
}

#[test]
fn a_truncated_file_is_read_again_from_the_start() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-09-01T10:00:00Z", "s", 5))?;
    append(&log, &call("m2", "2026-09-01T10:01:00Z", "s", 5))?;
    fixture.ingest()?;
    let file = fs::OpenOptions::new().write(true).open(&log)?;
    file.set_len(0)?;
    drop(file);
    append(&log, &call("m3", "2026-09-01T10:02:00Z", "s", 5))?;
    let ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (3, 15, 300));
    let lines: u64 = ledger
        .conn
        .query_row("SELECT lines FROM files", [], |row| row.get(0))?;
    assert_eq!(lines, 1);
    Ok(())
}

#[test]
fn a_replaced_file_is_read_again_from_the_start() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-09-01T10:00:00Z", "s", 5))?;
    fixture.ingest()?;
    // The replacement is longer than the original, so continuing at the old
    // offset would skip its first call.
    let replacement = fixture.dir.path().join("replacement");
    append(&replacement, &call("m2", "2026-09-01T10:01:00Z", "s", 7))?;
    append(&replacement, &call("m3", "2026-09-01T10:02:00Z", "s", 9))?;
    fs::rename(&replacement, &log)?;
    let ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (3, 21, 300));
    Ok(())
}

#[test]
fn a_last_line_without_its_newline_waits_for_the_next_ingestion() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    let line = call("m1", "2026-09-01T10:00:00Z", "s", 5);
    let (start, end) = line.split_at(line.len() - 1);
    append(&log, start)?;
    let ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (0, 0, 0));
    assert_eq!(ledger.sources()?[0].unreadable_lines, 0);
    append(&log, end)?;
    let ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (1, 5, 100));
    Ok(())
}

struct FailingSource {
    inner: Cursor<Vec<u8>>,
    fail_at: u64,
}

impl Read for FailingSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.inner.position() >= self.fail_at {
            return Err(io::Error::other("simulated read failure"));
        }
        let room = usize::try_from(self.fail_at - self.inner.position()).unwrap_or(usize::MAX);
        let len = buf.len().min(room);
        self.inner.read(&mut buf[..len])
    }
}

impl Seek for FailingSource {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

#[test]
fn an_interruption_inside_the_transaction_leaves_neither_calls_nor_checkpoint() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    let first = call("m1", "2026-09-01T10:00:00Z", "s", 5);
    append(&log, &first)?;
    append(&log, &call("m2", "2026-09-01T10:01:00Z", "s", 7))?;
    let mut ledger = fixture.ledger()?;
    let source = FailingSource {
        inner: Cursor::new(fs::read(&log)?),
        fail_at: first.len() as u64 + 3,
    };
    let metadata = fs::metadata(&log)?;
    let result = ledger.ingest_source(&fixture.adapter, &log, snapshot(&metadata), source);
    assert!(matches!(result, Err(Error::Read { .. })), "{result:?}");
    assert_eq!(totals(&ledger)?, (0, 0, 0));
    assert_eq!(ledger.sources()?, []);
    ledger.ingest(&fixture.adapter)?;
    assert_eq!(totals(&ledger)?, (2, 12, 200));
    Ok(())
}

#[test]
fn concurrent_ingestions_count_every_call_once() -> TestResult {
    let fixture = Fixture::new()?;
    for file in 0..20 {
        for line in 0..50 {
            append(
                &fixture.log(&format!("{file}.jsonl")),
                &call(
                    &format!("m{file}-{line}"),
                    "2026-09-01T10:00:00Z",
                    &format!("s{file}"),
                    1,
                ),
            )?;
        }
    }
    let results: Vec<Result<(), String>> = thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| fixture.ingest().map(drop).map_err(|e| e.to_string())))
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap_or_else(|_| Err("panicked".to_owned())))
            .collect()
    });
    for result in results {
        result?;
    }
    let ledger = fixture.ledger()?;
    assert_eq!(totals(&ledger)?, (1000, 1000, 100_000));
    let lines: u64 = ledger
        .conn
        .query_row("SELECT sum(lines) FROM files", [], |row| row.get(0))?;
    assert_eq!(lines, 1000);
    Ok(())
}

#[test]
fn calls_are_counted_in_a_half_open_time_range() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-08-31T23:59:59.999Z", "s", 1))?;
    append(&log, &call("m2", "2026-09-01T00:00:00Z", "s", 2))?;
    append(&log, &call("m3", "2026-09-30T23:59:59.999Z", "s", 4))?;
    append(&log, &call("m4", "2026-10-01T00:00:00Z", "s", 8))?;
    let ledger = fixture.ingest()?;
    let september = ledger.calls(
        Some("2026-09-01T00:00:00Z".parse()?),
        Some("2026-10-01T00:00:00Z".parse()?),
        None,
    )?;
    assert_eq!(september[0].calls, 2);
    assert_eq!(september[0].tokens.output, Tokens(6));
    Ok(())
}

#[test]
fn bounds_finer_than_a_millisecond_keep_the_range_half_open() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-09-01T00:00:00.001Z", "s", 1))?;
    let ledger = fixture.ingest()?;
    let count = |since: Option<&str>, until: Option<&str>| -> TestResult<u64> {
        let since = since.map(str::parse).transpose()?;
        let until = until.map(str::parse).transpose()?;
        Ok(ledger
            .calls(since, until, None)?
            .first()
            .map_or(0, |entry| entry.calls))
    };
    assert_eq!(count(None, Some("2026-09-01T00:00:00.0015Z"))?, 1);
    assert_eq!(count(None, Some("2026-09-01T00:00:00.0005Z"))?, 0);
    assert_eq!(count(None, Some("2026-09-01T00:00:00.001Z"))?, 0);
    assert_eq!(count(Some("2026-09-01T00:00:00.0015Z"), None)?, 0);
    assert_eq!(count(Some("2026-09-01T00:00:00.0005Z"), None)?, 1);
    assert_eq!(count(Some("2026-09-01T00:00:00.001Z"), None)?, 1);
    Ok(())
}

#[test]
fn a_ledger_from_a_newer_version_is_refused() -> TestResult {
    let fixture = Fixture::new()?;
    let ledger = fixture.ledger()?;
    ledger
        .conn
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)?;
    drop(ledger);
    assert!(matches!(fixture.ledger(), Err(Error::Newer { .. })));
    Ok(())
}

/// One Codex rollout line, without records, whose cumulative totals have
/// reached `output` output tokens, with its newline.
fn codex_total(output: u64) -> String {
    let line = json!({
        "timestamp": "2026-09-01T10:00:00Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {"total_token_usage": {
                "input_tokens": 100 * output,
                "cached_input_tokens": 0,
                "output_tokens": output,
                "total_tokens": 101 * output,
            }},
        },
    });
    format!("{line}\n")
}

fn codex_rollout(dir: &Path) -> io::Result<(Codex, PathBuf)> {
    let sessions = dir.join("sessions");
    fs::create_dir_all(&sessions)?;
    let log = sessions.join("rollout-2026-09-01T10-00-00-s.jsonl");
    let header = json!({
        "timestamp": "2026-09-01T10:00:00Z",
        "type": "session_meta",
        "payload": {"id": "s", "timestamp": "2026-09-01T10:00:00Z", "model_provider": "openai", "source": "exec", "cli_version": "0.200.0"},
    });
    let turn = json!({
        "timestamp": "2026-09-01T10:00:00Z",
        "type": "turn_context",
        "payload": {"model": "gpt-6-astra", "cwd": "/work"},
    });
    append(&log, &format!("{header}\n{turn}\n"))?;
    Ok((Codex::new(sessions), log))
}

#[test]
fn a_rollout_growing_between_ingestions_counts_each_increase_once() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (codex, log) = codex_rollout(dir.path())?;
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    append(&log, &codex_total(5))?;
    ledger.ingest(&codex)?;
    append(&log, &codex_total(12))?;
    ledger.ingest(&codex)?;
    assert_eq!(totals(&ledger)?, (2, 12, 0));
    Ok(())
}

#[test]
fn saved_state_that_cannot_be_read_makes_the_file_be_read_again() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (codex, log) = codex_rollout(dir.path())?;
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    append(&log, &codex_total(5))?;
    ledger.ingest(&codex)?;
    ledger
        .conn
        .execute("UPDATE files SET state = 'not a state'", [])?;
    append(&log, &codex_total(12))?;
    ledger.ingest(&codex)?;
    assert_eq!(totals(&ledger)?, (2, 12, 0));
    let (lines, usage_lines): (u64, u64) =
        ledger
            .conn
            .query_row("SELECT lines, usage_lines FROM files", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
    assert_eq!((lines, usage_lines), (4, 2));
    Ok(())
}

const SCHEMA_6_CALLS: &str = "DROP VIEW estimated; ALTER TABLE calls DROP COLUMN client_version;";

const SCHEMA_5_CALLS: &str = "ALTER TABLE calls DROP COLUMN compaction;
    ALTER TABLE calls DROP COLUMN context_input;";

const SCHEMA_4_CALLS: &str = "ALTER TABLE calls DROP COLUMN final_usage;
    ALTER TABLE calls DROP COLUMN stop_reason; ALTER TABLE calls DROP COLUMN first_output;";

#[test]
fn a_schema_1_ledger_is_migrated_and_keeps_its_calls() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-09-01T10:00:00Z", "s", 5))?;
    let ledger = fixture.ingest()?;
    ledger.conn.execute_batch(&format!(
        "{SCHEMA_6_CALLS} {SCHEMA_5_CALLS} {SCHEMA_4_CALLS} ALTER TABLE files DROP COLUMN state; DROP TABLE definitions;
             DROP TABLE directories; DROP TABLE labels; PRAGMA user_version = 1;"
    ))?;
    drop(ledger);
    append(&log, &call("m2", "2026-09-01T10:01:00Z", "s", 7))?;
    let ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (2, 12, 200));
    let version: i32 = ledger
        .conn
        .pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, SCHEMA_VERSION);
    Ok(())
}

#[test]
fn a_schema_2_ledger_is_migrated_and_keeps_its_calls() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-09-01T10:00:00Z", "s", 5))?;
    let ledger = fixture.ingest()?;
    ledger.conn.execute_batch(&format!(
        "{SCHEMA_6_CALLS} {SCHEMA_5_CALLS} {SCHEMA_4_CALLS} DROP TABLE definitions; DROP TABLE directories; DROP TABLE labels;
             PRAGMA user_version = 2;"
    ))?;
    drop(ledger);
    let ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (1, 5, 100));
    let version: i32 = ledger
        .conn
        .pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, SCHEMA_VERSION);
    let definitions: u64 =
        ledger
            .conn
            .query_row("SELECT count(*) FROM definitions", [], |row| row.get(0))?;
    assert_eq!(definitions, 0);
    Ok(())
}

#[test]
fn a_schema_3_ledger_is_migrated_and_keeps_its_calls() -> TestResult {
    let fixture = Fixture::new()?;
    let log = fixture.log("a.jsonl");
    append(&log, &call("m1", "2026-09-01T10:00:00Z", "s", 5))?;
    let ledger = fixture.ingest()?;
    ledger.conn.execute_batch(&format!(
        "{SCHEMA_6_CALLS} {SCHEMA_5_CALLS} {SCHEMA_4_CALLS} DROP TABLE directories; DROP TABLE labels; PRAGMA user_version = 3;"
    ))?;
    drop(ledger);
    let mut ledger = fixture.ingest()?;
    assert_eq!(totals(&ledger)?, (1, 5, 100));
    let version: i32 = ledger
        .conn
        .pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, SCHEMA_VERSION);
    assert_eq!(ledger.labels()?, []);
    assert_eq!(ledger.recorded_checkouts()?, Vec::<(String, String)>::new());
    ledger.record_checkouts(
        &[("/w/topic".to_owned(), "/w/repo".to_owned())],
        "2026-09-02T00:00:00Z".parse()?,
    )?;
    assert_eq!(
        ledger.recorded_checkouts()?,
        [("/w/topic".to_owned(), "/w/repo".to_owned())]
    );
    Ok(())
}

/// A user adapter for logs under `dir/logs` whose output tokens are at
/// `output`, so two pointers make two definitions.
fn user_adapter(dir: &Path, output: &str) -> TestResult<UserAdapter> {
    let file = dir.join("adapters/example.toml");
    fs::create_dir_all(dir.join("adapters"))?;
    fs::create_dir_all(dir.join("logs"))?;
    let root = toml_string(&dir.join("logs"));
    fs::write(
        &file,
        format!(
            r#"client = "example"

[files]
root = {root}

[fields]
id = "/id"
timestamp = "/time"
model = "/model"

[values]
provider = "example-cloud"

[tokens]
input = "/in"
output = "{output}"
"#
        ),
    )?;
    Ok(user::load(&file, &[])?)
}

/// A TOML literal string, so Windows paths need no escapes.
fn toml_string(path: &Path) -> String {
    format!("'{}'", path.display())
}

fn user_call(id: &str, output: u64) -> String {
    let line = json!({"id": id, "time": "2026-09-01T10:00:00Z", "model": "m", "in": 1, "out": output, "other": output * 2});
    format!("{line}\n")
}

fn declared(adapter: &UserAdapter) -> &Declared {
    match adapter {
        UserAdapter::Declared(declared) => declared,
        UserAdapter::External(_) => panic!("expected a declared adapter"),
    }
}

fn rows(ledger: &Ledger) -> TestResult<Vec<String>> {
    let mut rows = Vec::new();
    for query in [
        "SELECT client, call_id, output FROM calls ORDER BY client, call_id",
        "SELECT client, path, offset, lines FROM files ORDER BY client, path",
    ] {
        let mut statement = ledger.conn.prepare(query)?;
        let mut found = statement.query([])?;
        while let Some(row) = found.next()? {
            let columns = row.as_ref().column_count();
            let values: Vec<String> = (0..columns)
                .map(|index| {
                    row.get_ref(index)
                        .map(|value| format!("{value:?}"))
                        .unwrap_or_default()
                })
                .collect();
            rows.push(values.join(" "));
        }
    }
    Ok(rows)
}

#[test]
fn a_changed_adapter_definition_is_refused_whether_or_not_its_logs_changed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let original = user_adapter(dir.path(), "/out")?;
    let log = dir.path().join("logs/a.jsonl");
    append(&log, &user_call("c1", 5))?;
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    ledger.ingest(declared(&original))?;
    let before = rows(&ledger)?;
    let changed = user_adapter(dir.path(), "/other")?;
    let unchanged_logs = ledger.ingest(declared(&changed));
    assert!(
        matches!(unchanged_logs, Err(Error::Changed { .. })),
        "{unchanged_logs:?}"
    );
    append(&log, &user_call("c2", 7))?;
    let appended_logs = ledger.ingest(declared(&changed));
    assert!(
        matches!(appended_logs, Err(Error::Changed { .. })),
        "{appended_logs:?}"
    );
    assert_eq!(rows(&ledger)?, before);
    Ok(())
}

#[test]
fn forgetting_a_client_lets_its_logs_be_read_again_under_a_new_definition() -> TestResult {
    let dir = tempfile::tempdir()?;
    let original = user_adapter(dir.path(), "/out")?;
    append(&dir.path().join("logs/a.jsonl"), &user_call("c1", 5))?;
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    ledger.ingest(declared(&original))?;
    assert_eq!(ledger.forget("example")?, Forgotten { calls: 1, files: 1 });
    let changed = user_adapter(dir.path(), "/other")?;
    ledger.ingest(declared(&changed))?;
    assert_eq!(totals(&ledger)?, (1, 10, 0));
    Ok(())
}

#[test]
fn a_writer_holding_an_old_definition_cannot_write_after_a_reset() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ledger.sqlite");
    let log = dir.path().join("logs/a.jsonl");
    let old = user_adapter(dir.path(), "/out")?;
    append(&log, &user_call("c1", 5))?;
    let mut first = Ledger::open(&path)?;
    first.ingest(declared(&old))?;
    let new = user_adapter(dir.path(), "/other")?;
    let mut second = Ledger::open(&path)?;
    second.forget("example")?;
    second.ingest(declared(&new))?;
    let after_new = rows(&second)?;
    append(&log, &user_call("c2", 7))?;
    let stale = first.ingest(declared(&old));
    assert!(matches!(stale, Err(Error::Changed { .. })), "{stale:?}");
    // The file's own transaction checks too, without the up-front claim.
    let stale_file = first.ingest_source(
        declared(&old),
        &log,
        snapshot(&fs::metadata(&log)?),
        File::open(&log)?,
    );
    assert!(
        matches!(stale_file, Err(Error::Changed { .. })),
        "{stale_file:?}"
    );
    assert_eq!(rows(&second)?, after_new);
    Ok(())
}

fn record(id: &str, output: u64) -> String {
    json!({
        "id": id, "timestamp": "2026-09-01T10:00:00Z", "provider": "p", "model": "m",
        "session_id": "s", "tokens": {"uncached_input": 1, "output": output},
    })
    .to_string()
}

fn definition(dir: &Path, text: &str) -> Definition {
    Definition {
        file: dir.join("command.toml"),
        text: text.to_owned(),
    }
}

#[test]
fn command_output_replaces_what_its_source_read_before() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    let defined = definition(dir.path(), "a");
    let first = format!("{}\nnot json\n{}", record("r1", 5), record("r2", 7));
    ledger.ingest_output(
        "tool",
        &defined,
        &defined.file,
        first.into_bytes(),
        external::reader(),
    )?;
    assert_eq!(totals(&ledger)?, (2, 12, 0));
    let second = format!(
        "{}\n{}\n{}\n",
        record("r1", 5),
        record("r2", 7),
        record("r3", 1)
    );
    ledger.ingest_output(
        "tool",
        &defined,
        &defined.file,
        second.into_bytes(),
        external::reader(),
    )?;
    assert_eq!(totals(&ledger)?, (3, 13, 0));
    assert_eq!(
        ledger.sources()?,
        [Sources {
            client: "tool".to_owned(),
            files: 1,
            unreadable_lines: 0,
            files_without_usage: 0,
        }]
    );
    Ok(())
}

#[test]
fn command_output_under_an_old_definition_is_refused_after_a_reset() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ledger.sqlite");
    let old = definition(dir.path(), "old");
    let new = definition(dir.path(), "new");
    let mut first = Ledger::open(&path)?;
    first.ingest_output(
        "tool",
        &old,
        &old.file,
        record("r1", 5).into_bytes(),
        external::reader(),
    )?;
    let mut second = Ledger::open(&path)?;
    second.forget("tool")?;
    second.ingest_output(
        "tool",
        &new,
        &new.file,
        record("r1", 9).into_bytes(),
        external::reader(),
    )?;
    let after_new = rows(&second)?;
    let stale = first.ingest_output(
        "tool",
        &old,
        &old.file,
        record("r2", 50).into_bytes(),
        external::reader(),
    );
    assert!(matches!(stale, Err(Error::Changed { .. })), "{stale:?}");
    assert_eq!(rows(&second)?, after_new);
    Ok(())
}

/// `messages` are assistant messages as (entry ID, timestamp, response ID).
fn pi_session(header: &serde_json::Value, messages: &[(&str, &str, Option<&str>)]) -> String {
    let mut lines = vec![header.clone()];
    for &(id, timestamp, response_id) in messages {
        let mut line = json!({
            "type": "message",
            "id": id,
            "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "provider": "nebius",
                "model": "moonshotai/Kimi-K3",
                "usage": {"input": 1, "output": 2, "cacheRead": 10, "cacheWrite": 0},
            },
        });
        if let Some(response_id) = response_id {
            line["message"]["responseId"] = json!(response_id);
        }
        lines.push(line);
    }
    let mut text = lines
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    text
}

/// Each call's ID, session and directory.
type CallRows = Vec<(String, String, String)>;

/// `parent_name` and `fork_name` set the order the files are read in. With
/// `parent_later`, the parent's file appears only for a second ingestion.
/// The `u64` is the number of sessions.
fn pi_fork_rows(
    parent_name: &str,
    fork_name: &str,
    parent_later: bool,
) -> TestResult<(CallRows, u64)> {
    let dir = tempfile::tempdir()?;
    let sessions = dir.path().join("sessions");
    fs::create_dir_all(&sessions)?;
    let pi = Pi::new(sessions.clone());
    let parent = sessions.join(parent_name);
    let copied = [
        ("a1", "2026-09-01T10:00:05Z", Some("resp-1")),
        ("a2", "2026-09-01T10:00:06Z", None),
    ];
    let parent_text = pi_session(
        &json!({"type": "session", "id": "p", "timestamp": "2026-09-01T10:00:00Z", "cwd": "/work/parent"}),
        &copied,
    );
    let mut forked = copied.to_vec();
    forked.push(("b1", "2026-09-01T11:00:05Z", Some("resp-2")));
    let fork_text = pi_session(
        &json!({"type": "session", "id": "f", "timestamp": "2026-09-01T11:00:00Z",
                "cwd": "/work/fork", "parentSession": parent}),
        &forked,
    );
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    append(&sessions.join(fork_name), &fork_text)?;
    if parent_later {
        ledger.ingest(&pi)?;
    }
    append(&parent, &parent_text)?;
    ledger.ingest(&pi)?;
    let mut statement = ledger
        .conn
        .prepare("SELECT call_id, session_id, cwd FROM calls ORDER BY call_id")?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let sessions = ledger
        .calls(None, None, None)?
        .first()
        .map_or(0, |entry| entry.sessions);
    Ok((rows, sessions))
}

#[test]
fn a_pi_forks_copies_belong_to_the_parent_whichever_file_is_read_first() -> TestResult {
    let expected = vec![
        ("p#a2".to_owned(), "p".to_owned(), "/work/parent".to_owned()),
        (
            "resp-1".to_owned(),
            "p".to_owned(),
            "/work/parent".to_owned(),
        ),
        ("resp-2".to_owned(), "f".to_owned(), "/work/fork".to_owned()),
    ];
    for (parent_name, fork_name, parent_later) in [
        ("a.jsonl", "b.jsonl", false),
        ("b.jsonl", "a.jsonl", false),
        ("a.jsonl", "b.jsonl", true),
    ] {
        assert_eq!(
            pi_fork_rows(parent_name, fork_name, parent_later)?,
            (expected.clone(), 2),
            "parent {parent_name}, fork {fork_name}, parent later {parent_later}"
        );
    }
    Ok(())
}

fn shaped(id: &str, model: &str, cache_read: u64, uncached_input: u64, output: u64) -> String {
    json!({
        "id": id, "timestamp": "2026-09-01T10:00:00Z", "provider": "p", "model": model,
        "session_id": "s",
        "tokens": {"uncached_input": uncached_input, "cache_write": 1, "cache_read": cache_read,
                   "output": output},
    })
    .to_string()
}

#[test]
fn usage_sums_the_pairs_of_each_call_not_the_product_of_sums() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    let defined = definition(dir.path(), "a");
    // A long context with little new input, then a short one with much:
    // n_new is uncached input plus the one cache-write token.
    let output = format!(
        "{}\n{}\n{}\n",
        shaped("r1", "m", 1000, 9, 2),
        shaped("r2", "m", 10, 99, 20),
        shaped("r3", "other", 0, 0, 1),
    );
    ledger.ingest_output(
        "tool",
        &defined,
        &defined.file,
        output.into_bytes(),
        external::reader(),
    )?;
    let usage = ledger.usage(None, None, None, &Split::default())?;
    assert_eq!(usage.len(), 2);
    let first = &usage[0];
    assert_eq!(
        (
            first.model.as_str(),
            first.calls,
            first.cache_read,
            first.new_input,
            first.output
        ),
        ("m", 2, Tokens(1010), Tokens(110), Tokens(22))
    );
    // r1: 10 x (1000 + 10 / 2) = 10,050; r2: 100 x (10 + 100 / 2) = 6,000.
    // From the sums it would be 110 x (1010 + 110 / 2) = 117,150.
    assert!((first.prefill_pairs - 16_050.0).abs() < 1e-9);
    // r1: 2 x (1000 + 10 + 2 / 2) = 2,022; r2: 20 x (10 + 100 + 20 / 2) = 2,400.
    assert!((first.decode_pairs - 4_422.0).abs() < 1e-9);
    // r3: n_new is the cache-write token alone, 1 x (0 + 1 / 2) = 0.5, and
    // 1 x (0 + 1 + 1 / 2) = 1.5.
    assert!((usage[1].prefill_pairs - 0.5).abs() < 1e-9);
    assert!((usage[1].decode_pairs - 1.5).abs() < 1e-9);
    let later = ledger.usage(
        Some("2026-09-02T00:00:00Z".parse()?),
        None,
        None,
        &Split::default(),
    )?;
    assert_eq!(later, []);
    Ok(())
}

#[test]
fn usage_splits_calls_by_the_highest_threshold_their_prompt_exceeds() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    let defined = definition(dir.path(), "a");
    // Prompts of 100 (at the first threshold, not above it), 101, 250 and
    // 1,000 tokens: cache reads plus uncached input plus the cache write.
    let output = format!(
        "{}\n{}\n{}\n{}\n",
        shaped("r1", "m", 90, 9, 1),
        shaped("r2", "m", 90, 10, 1),
        shaped("r3", "m", 200, 49, 1),
        shaped("r4", "m", 900, 99, 1),
    );
    ledger.ingest_output(
        "tool",
        &defined,
        &defined.file,
        output.into_bytes(),
        external::reader(),
    )?;
    let split = |thresholds: &[u64]| -> Result<Vec<(u64, u64, Tokens)>, Error> {
        Ok(ledger
            .usage(None, None, None, &prompt(thresholds))?
            .into_iter()
            .map(|row| (row.prompt_above, row.calls, row.cache_read))
            .collect())
    };
    assert_eq!(
        split(&[500, 100, 100])?,
        [
            (0, 1, Tokens(90)),
            (100, 2, Tokens(290)),
            (500, 1, Tokens(900))
        ]
    );
    assert_eq!(split(&[])?, [(0, 4, Tokens(1280))]);
    let contexts = ledger.contexts(None, None)?;
    let attributions: Vec<Attribution> = contexts
        .iter()
        .map(|context| Attribution {
            client: &context.client,
            session_id: &context.session_id,
            cwd: context.cwd.as_deref(),
            project: Some("/work"),
            resolution: "checkout",
        })
        .collect();
    ledger.attribute(&attributions)?;
    let by_project = ledger.usage_by_project(None, None, &prompt(&[100]))?;
    let rows: Vec<(u64, u64)> = by_project
        .iter()
        .map(|row| (row.usage.prompt_above, row.usage.calls))
        .collect();
    assert_eq!(rows, [(0, 1), (100, 3)]);
    Ok(())
}

fn prompt(thresholds: &[u64]) -> Split<'_> {
    Split {
        prompt_above: thresholds,
        cached_above: None,
    }
}

#[test]
fn usage_splits_calls_by_role_and_by_cache_reads_above_a_threshold() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    let defined = definition(dir.path(), "a");
    let with_role = |id: &str, cache_read: u64, role: &str| {
        let mut record: serde_json::Value =
            serde_json::from_str(&shaped(id, "m", cache_read, 0, 1))?;
        record["role"] = json!(role);
        Ok::<String, serde_json::Error>(record.to_string())
    };
    // 100 cache-read tokens are at the threshold, not above it.
    let output = format!(
        "{}\n{}\n{}\n{}\n",
        with_role("r1", 100, "interactive")?,
        with_role("r2", 101, "interactive")?,
        with_role("r3", 500, "subagent")?,
        with_role("r4", 0, "subagent")?,
    );
    ledger.ingest_output(
        "tool",
        &defined,
        &defined.file,
        output.into_bytes(),
        external::reader(),
    )?;
    let split = Split {
        prompt_above: &[],
        cached_above: Some(100),
    };
    let rows: Vec<(String, bool, u64)> = ledger
        .usage(None, None, None, &split)?
        .into_iter()
        .map(|row| (row.role, row.cached_above, row.calls))
        .collect();
    assert_eq!(
        rows,
        [
            ("interactive".to_owned(), false, 1),
            ("interactive".to_owned(), true, 1),
            ("subagent".to_owned(), false, 1),
            ("subagent".to_owned(), true, 1),
        ]
    );
    let unsplit: Vec<(String, bool, u64)> = ledger
        .usage(None, None, None, &Split::default())?
        .into_iter()
        .map(|row| (row.role, row.cached_above, row.calls))
        .collect();
    assert_eq!(
        unsplit,
        [
            ("interactive".to_owned(), false, 2),
            ("subagent".to_owned(), false, 2),
        ]
    );
    Ok(())
}

#[test]
fn providers_count_calls_and_sessions_across_clients() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    for (client, records) in [
        ("one", [record("r1", 5), record("r2", 7)]),
        ("two", [record("r1", 1), record("r3", 2)]),
    ] {
        let defined = definition(dir.path(), client);
        let defined = Definition {
            file: dir.path().join(format!("{client}.toml")),
            ..defined
        };
        ledger.ingest_output(
            client,
            &defined,
            &defined.file,
            format!("{}\n{}\n", records[0], records[1]).into_bytes(),
            external::reader(),
        )?;
    }
    let providers = ledger.providers(None, None, None)?;
    assert_eq!(providers.len(), 1);
    // Every record is provider p in session s: one session per client.
    assert_eq!(
        (
            providers[0].provider.as_str(),
            providers[0].calls,
            providers[0].sessions
        ),
        ("p", 4, 2)
    );
    assert_eq!(providers[0].tokens.output, Tokens(15));
    Ok(())
}

#[test]
fn a_snapshot_keeps_calls_ingested_meanwhile_out_of_every_query() -> TestResult {
    let fixture = Fixture::new()?;
    append(
        &fixture.log("a.jsonl"),
        &call("m1", "2026-09-01T10:00:00Z", "s1", 5),
    )?;
    let mut ledger = fixture.ingest()?;
    ledger.begin_snapshot()?;
    let contexts = ledger.contexts(None, None)?;
    append(
        &fixture.log("b.jsonl"),
        &call("m2", "2026-09-01T10:01:00Z", "s2", 7),
    )?;
    fixture.ingest()?;
    let attributions: Vec<Attribution> = contexts
        .iter()
        .map(|context| Attribution {
            client: &context.client,
            session_id: &context.session_id,
            cwd: context.cwd.as_deref(),
            project: Some("/work"),
            resolution: "checkout",
        })
        .collect();
    ledger.attribute(&attributions)?;
    let total = |ledger: &Ledger| -> TestResult<u64> {
        Ok(ledger
            .calls(None, None, None)?
            .iter()
            .map(|client| client.calls)
            .sum())
    };
    assert_eq!(total(&ledger)?, 1);
    let by_project: u64 = ledger
        .usage_by_project(None, None, &Split::default())?
        .iter()
        .map(|row| row.usage.calls)
        .sum();
    assert_eq!(by_project, 1);
    ledger.end_snapshot()?;
    assert_eq!(total(&ledger)?, 2);
    Ok(())
}

/// One subagent line of the call `id` with its newline; `stop_reason` is
/// `None` on a line without the final usage.
fn subagent_line(id: &str, output: u64, stop_reason: Option<&str>) -> String {
    let line = json!({
        "type": "assistant",
        "sessionId": "s",
        "timestamp": "2026-09-01T10:00:00Z",
        "cwd": "/work",
        "entrypoint": "cli",
        "message": {
            "id": id,
            "model": "claude-opus-5-5",
            "stop_reason": stop_reason,
            "usage": {
                "input_tokens": 1,
                "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 100,
                "output_tokens": output,
            }
        }
    });
    format!("{line}\n")
}

fn subagent_log(fixture: &Fixture, name: &str) -> io::Result<PathBuf> {
    let dir = fixture.log("s/subagents");
    fs::create_dir_all(&dir)?;
    Ok(dir.join(name))
}

/// `count` complete calls whose output grows by `growth` from their first
/// line to their final line, which stops to use a tool.
fn complete_calls(count: u64, growth: u64) -> String {
    (0..count)
        .map(|index| {
            let id = format!("done{index}");
            subagent_line(&id, 10, None) + &subagent_line(&id, 10 + growth, Some("tool_use"))
        })
        .collect()
}

fn completion(
    ledger: &Ledger,
    id: &str,
) -> TestResult<(Option<bool>, Option<String>, Option<u64>)> {
    Ok(ledger.conn.query_row(
        "SELECT final_usage, stop_reason, first_output FROM calls WHERE call_id = ?1",
        params![id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?)
}

#[test]
fn a_call_is_final_once_any_copy_is_whatever_the_order() -> TestResult {
    let fixture = Fixture::new()?;
    let log = subagent_log(&fixture, "a.jsonl")?;
    append(&log, &subagent_line("m1", 5, None))?;
    append(&log, &subagent_line("m1", 90, Some("tool_use")))?;
    append(&log, &subagent_line("m2", 90, Some("end_turn")))?;
    append(&log, &subagent_line("m2", 5, None))?;
    append(&log, &subagent_line("m3", 5, None))?;
    append(&log, &subagent_line("m3", 8, None))?;
    let ledger = fixture.ingest()?;
    assert_eq!(
        completion(&ledger, "m1")?,
        (Some(true), Some("tool_use".to_owned()), Some(5))
    );
    assert_eq!(
        completion(&ledger, "m2")?,
        (Some(true), Some("end_turn".to_owned()), Some(5))
    );
    assert_eq!(completion(&ledger, "m3")?, (Some(false), None, Some(5)));
    Ok(())
}

#[test]
fn a_call_without_its_final_usage_gets_the_mean_growth_of_comparable_calls() -> TestResult {
    let fixture = Fixture::new()?;
    let log = subagent_log(&fixture, "a.jsonl")?;
    append(&log, &complete_calls(30, 100))?;
    append(&log, &subagent_line("cut", 5, None))?;
    // Growth already logged beyond the mean is kept, not added to.
    append(&log, &subagent_line("long", 5, None))?;
    append(&log, &subagent_line("long", 300, None))?;
    let ledger = fixture.ingest()?;
    let calls = ledger.calls(None, None, None)?;
    assert_eq!(calls[0].tokens.output, Tokens(30 * 110 + 5 + 300));
    assert_eq!(
        calls[0].final_usage,
        FinalUsage {
            missing: 2,
            missing_logged_output: Tokens(305),
            estimated_missing_output: Tokens(100),
            missing_without_estimate: 0,
            unknown: 0,
        }
    );
    let usage = ledger.usage(None, None, None, &Split::default())?;
    assert_eq!(
        (usage[0].output, usage[0].estimated_output),
        (Tokens(30 * 110 + 105 + 300), Tokens(100))
    );
    // n_out x (L + n_new + n_out / 2) with L = 100 and n_new = 11.
    let pairs = |n_out: f64| n_out * (111.0 + n_out / 2.0);
    let expected = 30.0 * pairs(110.0) + pairs(105.0) + pairs(300.0);
    assert!((usage[0].decode_pairs - expected).abs() < 1e-6);
    Ok(())
}

#[test]
fn fewer_than_30_comparable_calls_give_no_estimate() -> TestResult {
    let fixture = Fixture::new()?;
    let log = subagent_log(&fixture, "a.jsonl")?;
    append(&log, &complete_calls(29, 100))?;
    append(&log, &subagent_line("cut", 5, None))?;
    let ledger = fixture.ingest()?;
    let calls = ledger.calls(None, None, None)?;
    assert_eq!(
        calls[0].final_usage,
        FinalUsage {
            missing: 1,
            missing_logged_output: Tokens(5),
            estimated_missing_output: Tokens(0),
            missing_without_estimate: 1,
            unknown: 0,
        }
    );
    let usage = ledger.usage(None, None, None, &Split::default())?;
    assert_eq!(usage[0].estimated_output, Tokens(0));
    Ok(())
}

#[test]
fn a_schema_4_ledger_reads_claude_code_logs_again_and_keeps_what_it_cannot_tell() -> TestResult {
    let fixture = Fixture::new()?;
    let kept = subagent_log(&fixture, "kept.jsonl")?;
    let deleted = subagent_log(&fixture, "deleted.jsonl")?;
    append(&kept, &complete_calls(30, 100))?;
    append(&kept, &subagent_line("cut", 5, None))?;
    // The final copy of `resumed` is in a log deleted after schema 4 kept
    // its output; only a partial copy survives.
    append(&kept, &subagent_line("resumed", 5, None))?;
    append(&deleted, &subagent_line("resumed", 600, Some("tool_use")))?;
    append(&deleted, &subagent_line("gone", 7, Some("end_turn")))?;
    let mut ledger = fixture.ingest()?;
    let defined = definition(fixture.dir.path(), "a");
    ledger.ingest_output(
        "tool",
        &defined,
        &defined.file,
        record("r1", 9).into_bytes(),
        external::reader(),
    )?;
    let before = ledger.calls(None, None, None)?;
    let sources = ledger.sources()?;
    ledger.conn.execute_batch(&format!(
        "{SCHEMA_6_CALLS} {SCHEMA_5_CALLS} {SCHEMA_4_CALLS} PRAGMA user_version = 4;"
    ))?;
    drop(ledger);
    fs::remove_file(&deleted)?;
    let ledger = fixture.ingest()?;
    let version: i32 = ledger
        .conn
        .pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, SCHEMA_VERSION);
    let after = ledger.calls(None, None, None)?;
    for (before, after) in before.iter().zip(&after) {
        assert_eq!(
            (&before.client, before.calls, before.tokens),
            (&after.client, after.calls, after.tokens)
        );
    }
    assert_eq!(ledger.sources()?, sources);
    assert_eq!(
        completion(&ledger, "done0")?,
        (Some(true), Some("tool_use".to_owned()), Some(10))
    );
    assert_eq!(completion(&ledger, "cut")?, (Some(false), None, Some(5)));
    assert_eq!(completion(&ledger, "resumed")?.0, None);
    assert_eq!(completion(&ledger, "gone")?, (None, None, None));
    assert_eq!(completion(&ledger, "r1")?, (Some(true), None, Some(9)));
    assert_eq!(
        after[0].final_usage,
        FinalUsage {
            missing: 1,
            missing_logged_output: Tokens(5),
            estimated_missing_output: Tokens(100),
            missing_without_estimate: 0,
            unknown: 2,
        }
    );
    Ok(())
}

fn codex_session(
    dir: &Path,
    session: &str,
    model: &str,
    lines: &[serde_json::Value],
) -> io::Result<()> {
    let sessions = dir.join("sessions");
    fs::create_dir_all(&sessions)?;
    let header = json!({
        "timestamp": "2026-07-01T10:00:00Z",
        "type": "session_meta",
        "payload": {"id": session, "timestamp": "2026-07-01T10:00:00Z", "model_provider": "openai", "source": "exec"},
    });
    let turn = json!({
        "timestamp": "2026-07-01T10:00:00Z",
        "type": "turn_context",
        "payload": {"model": model, "cwd": "/work"},
    });
    let text: String = [header, turn]
        .iter()
        .chain(lines)
        .map(|line| line.to_string() + "\n")
        .collect();
    append(
        &sessions.join(format!("rollout-2026-07-01T10-00-00-{session}.jsonl")),
        &text,
    )
}

fn codex_usage(input: u64, cached: u64, output: u64) -> serde_json::Value {
    json!({
        "input_tokens": input,
        "cached_input_tokens": cached,
        "output_tokens": output,
        "total_tokens": input + output,
    })
}

fn recorded_compactions(
    count: u64,
    input: u64,
    cached: u64,
    output: u64,
) -> Vec<serde_json::Value> {
    (0..count)
        .flat_map(|index| {
            let id = format!("resp_{index}");
            [
                json!({
                    "timestamp": "2026-09-30T12:00:00Z",
                    "type": "token_usage_record",
                    "payload": {"response_id": id, "usage": codex_usage(input, cached, output)},
                }),
                json!({
                    "timestamp": "2026-09-30T12:00:00Z",
                    "type": "compacted",
                    "payload": {"message": "", "compaction_response_id": id},
                }),
            ]
        })
        .collect()
}

/// The totals event after a call with `input` tokens of input.
fn codex_increase(input: u64) -> serde_json::Value {
    json!({
        "timestamp": "2026-07-01T10:00:01Z",
        "type": "event_msg",
        "payload": {"type": "token_count", "info": {"total_token_usage": codex_usage(input, 0, 5)}},
    })
}

fn unrecorded_compaction() -> serde_json::Value {
    json!({"timestamp": "2026-07-01T10:00:02Z", "type": "compacted", "payload": {"message": ""}})
}

/// Rollouts with `records` recorded `gpt-6-astra` compactions (a quarter of
/// their input uncached, 60 output tokens), and compactions without a
/// record: after a 400,000-token `gpt-6-astra` call, after a 100,000-token
/// `gpt-5.5` call, and a `gpt-5.5` one without a previous call.
fn codex_compactions(dir: &Path, records: u64) -> TestResult<Ledger> {
    let mut recorded = recorded_compactions(records, 1000, 750, 60);
    recorded.push(json!({
        "timestamp": "2026-09-30T12:00:01Z",
        "type": "token_usage_record",
        "payload": {"response_id": "resp_other", "usage": codex_usage(1000, 0, 999)},
    }));
    codex_session(dir, "recorded", "gpt-6-astra", &recorded)?;
    codex_session(
        dir,
        "long",
        "gpt-6-astra",
        &[codex_increase(400_000), unrecorded_compaction()],
    )?;
    codex_session(
        dir,
        "short",
        "gpt-5.5",
        &[codex_increase(100_000), unrecorded_compaction()],
    )?;
    codex_session(dir, "first", "gpt-5.5", &[unrecorded_compaction()])?;
    let mut ledger = Ledger::open(&dir.join("ledger.sqlite"))?;
    ledger.ingest(&Codex::new(dir.join("sessions")))?;
    Ok(ledger)
}

#[test]
fn a_compaction_without_a_record_gets_the_previous_calls_input_split_like_recorded_ones()
-> TestResult {
    let dir = tempfile::tempdir()?;
    let mut ledger = codex_compactions(dir.path(), 30)?;
    let calls = ledger.calls(None, None, None)?;
    assert_eq!(calls[0].calls, 30 + 1 + 2 * 2 + 1);
    assert_eq!(
        calls[0].tokens.uncached_input,
        Tokens(30 * 250 + 1000 + 400_000 + 100_000)
    );
    assert_eq!(calls[0].tokens.output, Tokens(30 * 60 + 999 + 5 + 5));
    assert_eq!(
        calls[0].unrecorded,
        Unrecorded {
            calls: 3,
            estimated_uncached_input: Tokens(100_000 + 25_000),
            estimated_cache_read: Tokens(300_000 + 75_000),
            estimated_output: Tokens(60),
            without_input_estimate: 1,
            without_output_estimate: 2,
        }
    );
    // The 400,000-token compaction falls above the threshold with the call
    // before it, the 100,000-token one below it with the other `gpt-5.5`
    // calls.
    let above = |rows: Vec<Usage>| -> Vec<(String, u64, u64, Tokens, Tokens, Tokens, Tokens)> {
        rows.into_iter()
            .filter(|row| row.estimated_input.0 > 0)
            .map(|row| {
                (
                    row.model,
                    row.prompt_above,
                    row.calls,
                    row.cache_read,
                    row.new_input,
                    row.estimated_input,
                    row.estimated_output,
                )
            })
            .collect()
    };
    let expected = [
        (
            "gpt-5.5".to_owned(),
            0,
            3,
            Tokens(75_000),
            Tokens(100_000 + 25_000),
            Tokens(100_000),
            Tokens(0),
        ),
        (
            "gpt-6-astra".to_owned(),
            272_000,
            2,
            Tokens(300_000),
            Tokens(400_000 + 100_000),
            Tokens(400_000),
            Tokens(60),
        ),
    ];
    assert_eq!(
        above(ledger.usage(None, None, None, &prompt(&[272_000]))?),
        expected
    );
    let contexts = ledger.contexts(None, None)?;
    let attributions: Vec<Attribution> = contexts
        .iter()
        .map(|context| Attribution {
            client: &context.client,
            session_id: &context.session_id,
            cwd: context.cwd.as_deref(),
            project: Some("/work"),
            resolution: "checkout",
        })
        .collect();
    ledger.attribute(&attributions)?;
    let by_project = ledger
        .usage_by_project(None, None, &prompt(&[272_000]))?
        .into_iter()
        .map(|row| row.usage)
        .collect();
    assert_eq!(above(by_project), expected);
    Ok(())
}

#[test]
fn a_compaction_counts_above_the_cached_threshold_with_its_estimated_cache_read() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = codex_compactions(dir.path(), 30)?;
    let split = Split {
        prompt_above: &[],
        cached_above: Some(200_000),
    };
    // The 400,000-token compaction reads an estimated 300,000 cached
    // tokens; the call before it read none, as its log records.
    let above: Vec<(u64, Tokens, Tokens)> = ledger
        .usage(None, None, None, &split)?
        .into_iter()
        .filter(|row| row.cached_above)
        .map(|row| (row.calls, row.cache_read, row.estimated_input))
        .collect();
    assert_eq!(above, [(1, Tokens(300_000), Tokens(400_000))]);
    Ok(())
}

#[test]
fn fewer_than_30_recorded_compactions_give_no_estimate() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = codex_compactions(dir.path(), 29)?;
    let calls = ledger.calls(None, None, None)?;
    assert_eq!(
        calls[0].unrecorded,
        Unrecorded {
            calls: 3,
            without_input_estimate: 3,
            without_output_estimate: 3,
            ..Unrecorded::default()
        }
    );
    assert!(
        ledger
            .usage(None, None, None, &prompt(&[272_000]))?
            .iter()
            .all(|row| row.estimated_input.0 == 0 && row.estimated_output.0 == 0)
    );
    Ok(())
}

#[test]
fn a_schema_5_ledger_reads_codex_logs_again_to_find_compactions() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = codex_compactions(dir.path(), 30)?;
    let before = ledger.calls(None, None, None)?;
    let sources = ledger.sources()?;
    ledger.conn.execute_batch(&format!(
        "DELETE FROM calls WHERE context_input IS NOT NULL; {SCHEMA_6_CALLS} {SCHEMA_5_CALLS} PRAGMA user_version = 5;"
    ))?;
    drop(ledger);
    let mut ledger = Ledger::open(&dir.path().join("ledger.sqlite"))?;
    ledger.ingest(&Codex::new(dir.path().join("sessions")))?;
    let version: i32 = ledger
        .conn
        .pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, SCHEMA_VERSION);
    assert_eq!(ledger.calls(None, None, None)?, before);
    assert_eq!(ledger.sources()?, sources);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_is_skipped_and_read_in_full_once_readable() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new()?;
    let open = fixture.log("open.jsonl");
    let closed = fixture.log("closed.jsonl");
    append(&open, &call("m1", "2026-09-01T10:00:00Z", "s", 5))?;
    append(&closed, &call("m2", "2026-09-01T10:01:00Z", "t", 7))?;
    append(&closed, &call("m3", "2026-09-01T10:02:00Z", "t", 11))?;
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o000))?;
    // A superuser reads it regardless, so there is nothing to test.
    if File::open(&closed).is_ok() {
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o644))?;
        return Ok(());
    }
    let mut ledger = fixture.ledger()?;
    let walk = ledger.ingest(&fixture.adapter)?;
    assert_eq!(walk.files, std::slice::from_ref(&open));
    assert_eq!(walk.failures.len(), 1);
    assert_eq!(walk.failures[0].path, closed);
    assert_eq!(totals(&ledger)?, (1, 5, 100));
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o644))?;
    let walk = ledger.ingest(&fixture.adapter)?;
    assert!(walk.failures.is_empty(), "{:?}", walk.failures);
    assert_eq!(totals(&ledger)?, (3, 23, 300));
    ledger.ingest(&fixture.adapter)?;
    assert_eq!(totals(&ledger)?, (3, 23, 300));
    Ok(())
}

/// [`call`] written by Claude Code `version`, or by one that records none.
fn versioned_call(id: &str, timestamp: &str, version: Option<&str>) -> TestResult<String> {
    let mut line: serde_json::Value = serde_json::from_str(&call(id, timestamp, "s", 5))?;
    line["version"] = json!(version);
    Ok(format!("{line}\n"))
}

fn versions(ledger: &Ledger) -> TestResult<Vec<(String, Option<String>)>> {
    let mut statement = ledger
        .conn
        .prepare("SELECT call_id, client_version FROM calls ORDER BY call_id")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[test]
fn a_call_keeps_the_version_of_its_earliest_copy_else_of_any_copy() -> TestResult {
    let fixture = Fixture::new()?;
    let (earliest, later) = ("2026-09-01T10:00:00Z", "2026-09-02T10:00:00Z");
    // Each order of reading the copies occurs whichever file is read first.
    append(
        &fixture.log("a.jsonl"),
        &[
            versioned_call("both", earliest, Some("2.0.0"))?,
            versioned_call("both-reversed", later, Some("2.1.0"))?,
            versioned_call("later-only", earliest, None)?,
            versioned_call("later-only-reversed", later, Some("2.1.0"))?,
        ]
        .concat(),
    )?;
    append(
        &fixture.log("b.jsonl"),
        &[
            versioned_call("both", later, Some("2.1.0"))?,
            versioned_call("both-reversed", earliest, Some("2.0.0"))?,
            versioned_call("later-only", later, Some("2.1.0"))?,
            versioned_call("later-only-reversed", earliest, None)?,
        ]
        .concat(),
    )?;
    let ledger = fixture.ingest()?;
    let version = |version: &str| Some(version.to_owned());
    assert_eq!(
        versions(&ledger)?,
        [
            ("both".to_owned(), version("2.0.0")),
            ("both-reversed".to_owned(), version("2.0.0")),
            ("later-only".to_owned(), version("2.1.0")),
            ("later-only-reversed".to_owned(), version("2.1.0")),
        ]
    );
    Ok(())
}

#[test]
fn client_versions_count_the_calls_of_each_version_latest_first() -> TestResult {
    let fixture = Fixture::new()?;
    append(
        &fixture.log("a.jsonl"),
        &[
            versioned_call("m1", "2026-09-01T09:00:00Z", None)?,
            versioned_call("m2", "2026-09-01T10:00:00Z", Some("2.0.0"))?,
            versioned_call("m3", "2026-09-01T10:01:00Z", Some("2.1.0"))?,
            versioned_call("m4", "2026-09-01T10:02:00Z", Some("2.0.0"))?,
        ]
        .concat(),
    )?;
    let ledger = fixture.ingest()?;
    let counts = |since: Option<&str>| -> TestResult<Vec<(Option<String>, u64)>> {
        let since = since.map(str::parse).transpose()?;
        Ok(ledger
            .client_versions(since, None, None)?
            .into_iter()
            .map(|entry| (entry.version, entry.calls))
            .collect())
    };
    let version = |version: &str| Some(version.to_owned());
    assert_eq!(
        counts(None)?,
        [(version("2.0.0"), 2), (version("2.1.0"), 1), (None, 1)]
    );
    assert_eq!(
        counts(Some("2026-09-01T10:01:00Z"))?,
        [(version("2.0.0"), 1), (version("2.1.0"), 1)]
    );
    Ok(())
}

#[test]
fn a_schema_6_ledger_reads_claude_code_and_codex_logs_again_for_their_versions() -> TestResult {
    let fixture = Fixture::new()?;
    let kept = fixture.log("kept.jsonl");
    let deleted = fixture.log("deleted.jsonl");
    append(
        &kept,
        &versioned_call("m1", "2026-09-01T10:00:00Z", Some("2.1.0"))?,
    )?;
    append(
        &deleted,
        &versioned_call("m2", "2026-09-01T10:01:00Z", Some("2.1.0"))?,
    )?;
    let (codex, rollout) = codex_rollout(fixture.dir.path())?;
    append(&rollout, &codex_total(5))?;
    let mut ledger = fixture.ingest()?;
    ledger.ingest(&codex)?;
    let before = ledger.calls(None, None, None)?;
    let sources = ledger.sources()?;
    ledger
        .conn
        .execute_batch(&format!("{SCHEMA_6_CALLS} PRAGMA user_version = 6;"))?;
    drop(ledger);
    fs::remove_file(&deleted)?;
    let mut ledger = fixture.ingest()?;
    ledger.ingest(&codex)?;
    let version: i32 = ledger
        .conn
        .pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(version, SCHEMA_VERSION);
    assert_eq!(ledger.calls(None, None, None)?, before);
    assert_eq!(ledger.sources()?, sources);
    assert_eq!(
        versions(&ledger)?,
        [
            ("m1".to_owned(), Some("2.1.0".to_owned())),
            ("m2".to_owned(), None),
            ("s#1".to_owned(), Some("0.200.0".to_owned())),
        ]
    );
    Ok(())
}

#[test]
fn a_schema_6_call_takes_the_version_of_its_earliest_copy_in_either_order() -> TestResult {
    let fixture = Fixture::new()?;
    let (earliest, later) = ("2026-09-01T10:00:00Z", "2026-09-02T10:00:00Z");
    append(
        &fixture.log("a.jsonl"),
        &[
            versioned_call("earliest-first", earliest, Some("2.0.0"))?,
            versioned_call("later-first", later, Some("2.1.0"))?,
        ]
        .concat(),
    )?;
    append(
        &fixture.log("b.jsonl"),
        &[
            versioned_call("earliest-first", later, Some("2.1.0"))?,
            versioned_call("later-first", earliest, Some("2.0.0"))?,
        ]
        .concat(),
    )?;
    let ledger = fixture.ingest()?;
    ledger
        .conn
        .execute_batch(&format!("{SCHEMA_6_CALLS} PRAGMA user_version = 6;"))?;
    drop(ledger);
    let ledger = fixture.ingest()?;
    let version = || Some("2.0.0".to_owned());
    assert_eq!(
        versions(&ledger)?,
        [
            ("earliest-first".to_owned(), version()),
            ("later-first".to_owned(), version()),
        ]
    );
    Ok(())
}
