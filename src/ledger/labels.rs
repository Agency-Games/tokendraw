use jiff::Timestamp;
use rusqlite::{TransactionBehavior, params};

use super::{Error, Ledger};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribution<'a> {
    pub client: &'a str,
    pub session_id: &'a str,
    pub cwd: Option<&'a str>,
    /// The project, or the directory of an unresolved context; `None` for
    /// an unresolved context without a working directory.
    pub project: Option<&'a str>,
    /// How the project was found, or `unresolved`.
    pub resolution: &'static str,
}

/// What a label is on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// An absolute directory, and everything below it.
    Directory(String),
    Session {
        client: String,
        session_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelRecord {
    pub target: Target,
    pub key: String,
    pub value: String,
}

impl Ledger {
    /// For this connection only. [`Ledger::calls`], [`Ledger::providers`]
    /// and [`Ledger::usage`] select a project's calls by it, and
    /// [`Ledger::usage_by_project`] groups by it.
    pub fn attribute(&mut self, attributions: &[Attribution]) -> Result<(), Error> {
        self.write_attribution(attributions)
            .map_err(|error| self.database(error))
    }

    fn write_attribution(&mut self, attributions: &[Attribution]) -> rusqlite::Result<()> {
        // A savepoint, so it can run inside a snapshot.
        let tx = self.conn.savepoint()?;
        tx.execute("DELETE FROM attribution", [])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO attribution (a_client, a_session, a_cwd, project, resolution)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for entry in attributions {
                insert.execute(params![
                    entry.client,
                    entry.session_id,
                    entry.cwd,
                    entry.project,
                    entry.resolution
                ])?;
            }
        }
        tx.commit()
    }

    pub fn working_directories(&self) -> Result<Vec<String>, Error> {
        self.query_working_directories()
            .map_err(|error| self.database(error))
    }

    fn query_working_directories(&self) -> rusqlite::Result<Vec<String>> {
        let mut statement = self
            .conn
            .prepare("SELECT DISTINCT cwd FROM calls WHERE cwd IS NOT NULL ORDER BY cwd")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// A directory whose checkout is unchanged keeps its record.
    pub fn record_checkouts(
        &mut self,
        checkouts: &[(String, String)],
        observed: Timestamp,
    ) -> Result<(), Error> {
        self.write_checkouts(checkouts, observed.as_millisecond())
            .map_err(|error| self.database(error))
    }

    fn write_checkouts(
        &mut self,
        checkouts: &[(String, String)],
        observed_ms: i64,
    ) -> rusqlite::Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut upsert = tx.prepare(
                "INSERT INTO directories (path, checkout, observed_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT (path) DO UPDATE SET
                     checkout = excluded.checkout, observed_ms = excluded.observed_ms
                 WHERE checkout IS NOT excluded.checkout",
            )?;
            for (path, checkout) in checkouts {
                upsert.execute(params![path, checkout, observed_ms])?;
            }
        }
        tx.commit()
    }

    pub fn recorded_checkouts(&self) -> Result<Vec<(String, String)>, Error> {
        self.query_recorded().map_err(|error| self.database(error))
    }

    fn query_recorded(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let mut statement = self
            .conn
            .prepare("SELECT path, checkout FROM directories ORDER BY path")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Directories first, by target and key.
    pub fn labels(&self) -> Result<Vec<LabelRecord>, Error> {
        self.query_labels().map_err(|error| self.database(error))
    }

    fn query_labels(&self) -> rusqlite::Result<Vec<LabelRecord>> {
        let mut statement = self
            .conn
            .prepare("SELECT kind, client, target, key, value FROM labels")?;
        let rows = statement.query_map([], |row| {
            let kind: String = row.get(0)?;
            let target = if kind == SESSION {
                Target::Session {
                    client: row.get(1)?,
                    session_id: row.get(2)?,
                }
            } else {
                Target::Directory(row.get(2)?)
            };
            Ok(LabelRecord {
                target,
                key: row.get(3)?,
                value: row.get(4)?,
            })
        })?;
        let mut labels = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        labels.sort_by(|a, b| (&a.target, &a.key).cmp(&(&b.target, &b.key)));
        Ok(labels)
    }

    /// A `None` value removes the key.
    pub fn set_labels(
        &mut self,
        target: &Target,
        changes: &[(String, Option<String>)],
    ) -> Result<(), Error> {
        self.write_labels(target, changes)
            .map_err(|error| self.database(error))
    }

    fn write_labels(
        &mut self,
        target: &Target,
        changes: &[(String, Option<String>)],
    ) -> rusqlite::Result<()> {
        let (kind, client, target) = match target {
            Target::Directory(path) => (DIRECTORY, "", path.as_str()),
            Target::Session { client, session_id } => {
                (SESSION, client.as_str(), session_id.as_str())
            }
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (key, value) in changes {
            match value {
                Some(value) => tx.execute(
                    "INSERT OR REPLACE INTO labels (kind, client, target, key, value)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![kind, client, target, key, value],
                )?,
                None => tx.execute(
                    "DELETE FROM labels
                     WHERE kind = ?1 AND client = ?2 AND target = ?3 AND key = ?4",
                    params![kind, client, target, key],
                )?,
            };
        }
        tx.commit()
    }
}

/// The `kind` of a directory label in the ledger.
const DIRECTORY: &str = "directory";

/// The `kind` of a session label in the ledger.
const SESSION: &str = "session";
