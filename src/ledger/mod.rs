//! The per-machine ledger: one row per model call, numbers only, and a
//! checkpoint per source file.
//!
//! Each file is ingested in one transaction that writes its calls and its
//! checkpoint together, so an interruption leaves either both or neither.
//! Concurrent processes are serialized by SQLite's write lock.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, TransactionBehavior};
use thiserror::Error;

mod ingest;
mod labels;
mod query;
mod schema;

pub use ingest::Forgotten;
pub use labels::{Attribution, LabelRecord, Target};
pub use query::{
    ClientCalls, Context, FinalUsage, ProjectUsage, ProviderCalls, Sources, Split, Unrecorded,
    Usage, VersionCalls, WithoutFixed,
};
use schema::{ATTRIBUTION, MIGRATIONS, SCHEMA, SCHEMA_VERSION};

/// One transaction holds the lock while a single log file is parsed, which
/// takes well under a second for the largest files seen; the margin covers
/// slow disks and a first ingestion of a large file.
const BUSY_TIMEOUT: Duration = Duration::from_secs(60);

/// A new file waits to switch to WAL only while another process creates
/// the schema, a short transaction.
const BUSY_RETRY: Duration = Duration::from_millis(5);

#[derive(Debug, Error)]
pub enum Error {
    #[error(
        "cannot create the directory of the ledger {}: {error}; check that its parent directory is writable",
        path.display()
    )]
    Create { path: PathBuf, error: io::Error },
    #[error("cannot open the ledger {}: {error}; check that its directory is writable", path.display())]
    Open {
        path: PathBuf,
        error: rusqlite::Error,
    },
    #[error(
        "the ledger {} has schema version {version}, newer than this tokendraw reads; upgrade tokendraw",
        path.display()
    )]
    Newer { path: PathBuf, version: i32 },
    #[error(
        "ledger {}: {error}; if another tokendraw process is using it, retry when that process has finished",
        path.display()
    )]
    Database {
        path: PathBuf,
        error: rusqlite::Error,
    },
    #[error("cannot read {}: {error}; check that the file is readable", path.display())]
    Read { path: PathBuf, error: io::Error },
    #[error(
        "the adapter file {} changed since the calls of {client} were read with it; restore the file, or run `tokendraw adapter reset {client}` to forget those calls and read the logs again, which loses the calls of logs deleted since",
        file.display()
    )]
    Changed { client: String, file: PathBuf },
}

pub struct Ledger {
    path: PathBuf,
    conn: Connection,
}

impl Ledger {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|error| Error::Create {
                path: path.to_path_buf(),
                error,
            })?;
        }
        let mut ledger = Self {
            path: path.to_path_buf(),
            conn: Connection::open(path).map_err(|error| Error::Open {
                path: path.to_path_buf(),
                error,
            })?,
        };
        ledger.prepare().map_err(|error| ledger.database(error))?;
        let version = ledger.migrate().map_err(|error| ledger.database(error))?;
        if version > SCHEMA_VERSION {
            return Err(Error::Newer {
                path: ledger.path,
                version,
            });
        }
        ledger
            .conn
            .execute_batch(ATTRIBUTION)
            .map_err(|error| ledger.database(error))?;
        Ok(ledger)
    }

    /// For reading an adapter without keeping what it read.
    pub fn in_memory() -> Result<Self, Error> {
        let path = PathBuf::from(":memory:");
        let mut ledger = Self {
            conn: Connection::open_in_memory().map_err(|error| Error::Open {
                path: path.clone(),
                error,
            })?,
            path,
        };
        ledger.migrate().map_err(|error| ledger.database(error))?;
        ledger
            .conn
            .execute_batch(ATTRIBUTION)
            .map_err(|error| ledger.database(error))?;
        Ok(ledger)
    }

    fn prepare(&self) -> rusqlite::Result<()> {
        self.conn.busy_timeout(BUSY_TIMEOUT)?;
        // Switching a new file to WAL fails at once while another process
        // holds a lock on it, without waiting on the busy timeout.
        let start = Instant::now();
        loop {
            match self
                .conn
                .query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
            {
                Err(error)
                    if error.sqlite_error_code() == Some(ErrorCode::DatabaseBusy)
                        && start.elapsed() < BUSY_TIMEOUT =>
                {
                    thread::sleep(BUSY_RETRY);
                }
                result => break result?,
            }
        }
        // In WAL mode NORMAL never corrupts the database; a power loss can
        // undo the last transactions, which the next ingestion reads again.
        self.conn.pragma_update(None, "synchronous", "NORMAL")
    }

    /// A version newer than [`SCHEMA_VERSION`] is returned as found.
    fn migrate(&mut self) -> rusqlite::Result<i32> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version == 0 {
            tx.execute_batch(SCHEMA)?;
        } else {
            let Some(migrations) = usize::try_from(version - 1)
                .ok()
                .and_then(|done| MIGRATIONS.get(done..))
                .filter(|migrations| !migrations.is_empty())
            else {
                return Ok(version);
            };
            for migration in migrations {
                tx.execute_batch(migration)?;
            }
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(SCHEMA_VERSION)
    }

    fn database(&self, error: rusqlite::Error) -> Error {
        Error::Database {
            path: self.path.clone(),
            error,
        }
    }
}

#[cfg(test)]
mod tests;
