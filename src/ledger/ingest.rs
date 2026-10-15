use std::fs::{File, Metadata};
use std::io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::schema::{LOAD_CHECKPOINT, SAVE_CHECKPOINT, UPSERT_CALL};
use super::{Error, Ledger};
use crate::adapters::walk::{self, Walk};
use crate::adapters::{Adapter, Call, Completion, Definition, Line, Reader};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forgotten {
    pub calls: u64,
    pub files: u64,
}

/// The file facts a checkpoint is compared with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    device: i64,
    inode: i64,
    size: u64,
    mtime_ns: i64,
}

#[derive(Clone, Debug)]
struct Checkpoint {
    snapshot: Snapshot,
    offset: u64,
    lines: u64,
    unreadable_lines: u64,
    usage_lines: u64,
    /// What the file's reader saved after the line before `offset`.
    state: Option<String>,
}

impl Checkpoint {
    fn start(snapshot: Snapshot) -> Self {
        Self {
            snapshot,
            offset: 0,
            lines: 0,
            unreadable_lines: 0,
            usage_lines: 0,
            state: None,
        }
    }
}

impl Ledger {
    pub fn ingest(&mut self, adapter: &dyn Adapter) -> Result<Walk, Error> {
        self.ingest_walk(adapter, adapter.discover())
    }

    /// Returns `walk` with the files that could not be read moved to its
    /// failures.
    ///
    /// Files that were deleted keep their calls. A file that shrank or was
    /// replaced is read again from the start; call identity keeps that from
    /// counting a call twice. A file that cannot be opened or read keeps its
    /// checkpoint and none of what this run read of it, so a later run reads
    /// it in full once it is readable.
    ///
    /// # Errors
    ///
    /// Files ingested before an error stay ingested.
    pub fn ingest_walk(&mut self, adapter: &dyn Adapter, mut walk: Walk) -> Result<Walk, Error> {
        // Claimed up front too, so a changed definition is refused even when
        // no log changed.
        if let Some(definition) = adapter.definition() {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| Error::Database {
                    path: self.path.clone(),
                    error,
                })?;
            claim(&tx, adapter.client(), definition, &self.path)?;
            tx.commit().map_err(|error| self.database(error))?;
        }
        for path in std::mem::take(&mut walk.files) {
            let opened = File::open(&path).and_then(|file| {
                let metadata = file.metadata()?;
                Ok((file, metadata))
            });
            let (file, metadata) = match opened {
                Ok(opened) => opened,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    walk.failures.push(walk::Error { path, error });
                    continue;
                }
            };
            match self.ingest_source(adapter, &path, snapshot(&metadata), file) {
                Ok(()) => walk.files.push(path),
                Err(Error::Read { path, error }) => {
                    walk.failures.push(walk::Error { path, error });
                }
                Err(error) => return Err(error),
            }
        }
        Ok(walk)
    }

    pub fn has_read_files(&self) -> Result<bool, Error> {
        self.conn
            .query_row("SELECT EXISTS (SELECT 1 FROM files)", [], |row| row.get(0))
            .map_err(|error| self.database(error))
    }

    pub(super) fn ingest_source<R: Read + Seek>(
        &mut self,
        adapter: &dyn Adapter,
        path: &Path,
        snapshot: Snapshot,
        source: R,
    ) -> Result<(), Error> {
        let client = adapter.client();
        let key = path.to_string_lossy();
        // Checked outside a write transaction first, so unchanged files never
        // wait for another process's write lock.
        let previous = load_checkpoint(&self.conn, client, &key).map_err(|e| self.database(e))?;
        if previous.is_some_and(|checkpoint| checkpoint.snapshot == snapshot) {
            return Ok(());
        }
        let ledger_path = &self.path;
        let database = |error| Error::Database {
            path: ledger_path.clone(),
            error,
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database)?;
        if let Some(definition) = adapter.definition() {
            claim(&tx, client, definition, ledger_path)?;
        }
        let previous = load_checkpoint(&tx, client, &key).map_err(database)?;
        let mut checkpoint = match previous {
            Some(checkpoint)
                if checkpoint.snapshot.device == snapshot.device
                    && checkpoint.snapshot.inode == snapshot.inode
                    && checkpoint.offset <= snapshot.size =>
            {
                checkpoint
            }
            _ => Checkpoint::start(snapshot),
        };
        checkpoint.snapshot = snapshot;
        let resumed = match checkpoint.state.as_deref() {
            Some(saved) => adapter.resume(path, saved),
            None => Some(adapter.start(path)),
        };
        let reader = resumed.unwrap_or_else(|| {
            checkpoint = Checkpoint::start(snapshot);
            adapter.start(path)
        });
        read_lines(&tx, adapter.client(), reader, source, &mut checkpoint).map_err(|failure| {
            match failure {
                Failure::Read(error) => Error::Read {
                    path: path.to_path_buf(),
                    error,
                },
                Failure::Database(error) => database(error),
            }
        })?;
        save_checkpoint(&tx, client, &key, &checkpoint).map_err(database)?;
        tx.commit().map_err(database)
    }

    /// `output` replaces what `source` read before: its lines, unreadable
    /// lines and usage lines are those of this output. A last line without
    /// its newline counts, because the command has finished.
    pub fn ingest_output(
        &mut self,
        client: &str,
        definition: &Definition,
        source: &Path,
        mut output: Vec<u8>,
        reader: Box<dyn Reader>,
    ) -> Result<(), Error> {
        if output.last().is_some_and(|last| *last != b'\n') {
            output.push(b'\n');
        }
        let ledger_path = &self.path;
        let database = |error| Error::Database {
            path: ledger_path.clone(),
            error,
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database)?;
        claim(&tx, client, definition, ledger_path)?;
        let mut checkpoint = Checkpoint::start(Snapshot {
            device: 0,
            inode: 0,
            size: output.len() as u64,
            mtime_ns: 0,
        });
        let key = source.to_string_lossy();
        read_lines(&tx, client, reader, Cursor::new(output), &mut checkpoint).map_err(
            |failure| match failure {
                Failure::Read(error) => Error::Read {
                    path: source.to_path_buf(),
                    error,
                },
                Failure::Database(error) => database(error),
            },
        )?;
        save_checkpoint(&tx, client, &key, &checkpoint).map_err(database)?;
        tx.commit().map_err(database)
    }

    /// The client's logs are then read again from the start; calls whose
    /// logs are gone are lost.
    pub fn forget(&mut self, client: &str) -> Result<Forgotten, Error> {
        let ledger_path = &self.path;
        let database = |error| Error::Database {
            path: ledger_path.clone(),
            error,
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database)?;
        let calls = tx
            .execute("DELETE FROM calls WHERE client = ?1", params![client])
            .map_err(database)?;
        let files = tx
            .execute("DELETE FROM files WHERE client = ?1", params![client])
            .map_err(database)?;
        tx.execute("DELETE FROM definitions WHERE client = ?1", params![client])
            .map_err(database)?;
        tx.commit().map_err(database)?;
        Ok(Forgotten {
            calls: calls as u64,
            files: files as u64,
        })
    }
}

/// Records `definition` as the client's when it has none, inside the
/// caller's write transaction, so a process holding another definition
/// cannot write between the check and its rows.
fn claim(
    tx: &Transaction,
    client: &str,
    definition: &Definition,
    ledger: &Path,
) -> Result<(), Error> {
    let database = |error| Error::Database {
        path: ledger.to_path_buf(),
        error,
    };
    let stored: Option<String> = tx
        .query_row(
            "SELECT definition FROM definitions WHERE client = ?1",
            params![client],
            |row| row.get(0),
        )
        .optional()
        .map_err(database)?;
    match stored {
        Some(stored) if stored == definition.text => Ok(()),
        Some(_) => Err(Error::Changed {
            client: client.to_owned(),
            file: definition.file.clone(),
        }),
        None => {
            tx.execute(
                "INSERT INTO definitions (client, definition) VALUES (?1, ?2)",
                params![client, definition.text],
            )
            .map_err(database)?;
            Ok(())
        }
    }
}

enum Failure {
    Read(io::Error),
    Database(rusqlite::Error),
}

/// Reads up to the size in the checkpoint's snapshot. A last line without
/// its newline may still be being written, so it waits for the next
/// ingestion.
fn read_lines<R: Read + Seek>(
    tx: &Transaction,
    client: &str,
    mut reader: Box<dyn Reader>,
    mut source: R,
    checkpoint: &mut Checkpoint,
) -> Result<(), Failure> {
    if checkpoint.offset >= checkpoint.snapshot.size {
        return Ok(());
    }
    source
        .seek(SeekFrom::Start(checkpoint.offset))
        .map_err(Failure::Read)?;
    let mut lines = BufReader::new(source.take(checkpoint.snapshot.size - checkpoint.offset));
    let mut upsert = tx.prepare(UPSERT_CALL).map_err(Failure::Database)?;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = lines.read_until(b'\n', &mut line).map_err(Failure::Read)?;
        if line.last() != Some(&b'\n') {
            checkpoint.state = reader.save();
            return Ok(());
        }
        checkpoint.offset += read as u64;
        checkpoint.lines += 1;
        let content = line.strip_suffix(b"\n").unwrap_or(&line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        match reader.parse_line(content) {
            Line::Call(call) => {
                checkpoint.usage_lines += 1;
                upsert_call(&mut upsert, client, &call).map_err(Failure::Database)?;
            }
            Line::Other => {}
            Line::Unreadable => checkpoint.unreadable_lines += 1,
        }
    }
}

fn upsert_call(
    statement: &mut rusqlite::Statement,
    client: &str,
    call: &Call,
) -> rusqlite::Result<()> {
    statement.execute(params![
        client,
        call.id,
        call.provider,
        call.model,
        call.timestamp.as_millisecond(),
        call.session_id,
        call.cwd,
        call.role.as_str(),
        call.tokens.uncached_input.0,
        call.tokens.cache_write.0,
        call.tokens.cache_read.0,
        call.tokens.output.0,
        call.tokens.reasoning.map(|tokens| tokens.0),
        !matches!(call.completion, Completion::Partial),
        match &call.completion {
            Completion::Final { stop_reason } => stop_reason.as_deref(),
            Completion::Partial | Completion::Unrecorded { .. } => None,
        },
        call.compaction,
        match call.completion {
            Completion::Unrecorded { context_input } => Some(context_input.0),
            Completion::Final { .. } | Completion::Partial => None,
        },
        call.client_version,
        call.agent_id,
    ])?;
    Ok(())
}

fn load_checkpoint(
    conn: &Connection,
    client: &str,
    key: &str,
) -> rusqlite::Result<Option<Checkpoint>> {
    conn.query_row(LOAD_CHECKPOINT, params![client, key], |row| {
        Ok(Checkpoint {
            snapshot: Snapshot {
                device: row.get(0)?,
                inode: row.get(1)?,
                size: row.get(2)?,
                mtime_ns: row.get(3)?,
            },
            offset: row.get(4)?,
            lines: row.get(5)?,
            unreadable_lines: row.get(6)?,
            usage_lines: row.get(7)?,
            state: row.get(8)?,
        })
    })
    .optional()
}

fn save_checkpoint(
    tx: &Transaction,
    client: &str,
    key: &str,
    checkpoint: &Checkpoint,
) -> rusqlite::Result<()> {
    tx.execute(
        SAVE_CHECKPOINT,
        params![
            client,
            key,
            checkpoint.snapshot.device,
            checkpoint.snapshot.inode,
            checkpoint.snapshot.size,
            checkpoint.snapshot.mtime_ns,
            checkpoint.offset,
            checkpoint.lines,
            checkpoint.unreadable_lines,
            checkpoint.usage_lines,
            checkpoint.state,
        ],
    )?;
    Ok(())
}

pub(super) fn snapshot(metadata: &Metadata) -> Snapshot {
    let (device, inode) = identity(metadata);
    Snapshot {
        device,
        inode,
        size: metadata.len(),
        mtime_ns: metadata.modified().map_or(0, nanos_since_epoch),
    }
}

/// Device and inode, stored as the same 64 bits; only equality matters.
#[cfg(unix)]
fn identity(metadata: &Metadata) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev().cast_signed(), metadata.ino().cast_signed())
}

/// Stable Rust exposes no file index on other systems; a replaced file has
/// a new creation time.
#[cfg(not(unix))]
fn identity(metadata: &Metadata) -> (i64, i64) {
    (0, metadata.created().map_or(0, nanos_since_epoch))
}

fn nanos_since_epoch(time: std::time::SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_nanos()).unwrap_or(i64::MAX)
    })
}
