use jiff::Timestamp;
use serde::Serialize;

use super::schema::{IN_PROJECT, IN_RANGE};
use super::{Error, Ledger};
use crate::quantity::{TokenClasses, Tokens};

const NANOS_PER_MILLISECOND: i128 = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientCalls {
    pub client: String,
    pub calls: u64,
    pub sessions: u64,
    pub tokens: TokenClasses,
    /// How many of the calls record their reasoning tokens.
    pub calls_with_reasoning: u64,
    pub final_usage: FinalUsage,
    pub unrecorded: Unrecorded,
    pub earliest: Timestamp,
    pub latest: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionCalls {
    pub client: String,
    /// `None` for the calls whose logs record no version.
    pub version: Option<String>,
    pub calls: u64,
    pub latest: Timestamp,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FinalUsage {
    /// Calls whose logs never record the final usage.
    pub missing: u64,
    pub missing_logged_output: Tokens,
    pub estimated_missing_output: Tokens,
    /// Of `missing`, the calls with too few comparable calls for an
    /// estimate.
    pub missing_without_estimate: u64,
    /// Calls read before final usage was recorded whose logs are gone, so
    /// whether it is final is unknown.
    pub unknown: u64,
}

/// The compactions whose logs record none of their usage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Unrecorded {
    pub calls: u64,
    pub estimated_uncached_input: Tokens,
    pub estimated_cache_read: Tokens,
    pub estimated_output: Tokens,
    /// Of `calls`, those without a previous call or with too few
    /// comparable compactions for an input estimate.
    pub without_input_estimate: u64,
    /// Of `calls`, those with too few comparable compactions for an output
    /// estimate.
    pub without_output_estimate: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCalls {
    /// The provider's name as the logs record it.
    pub provider: String,
    pub calls: u64,
    /// Sessions with a call to the provider; a session that calls two
    /// providers counts under both.
    pub sessions: u64,
    pub tokens: TokenClasses,
}

/// The sums the energy model needs. `L` is a call's cache-read tokens,
/// `n_new` its uncached input plus cache-write tokens and `n_out` its output
/// tokens, each with the estimate for a call whose logs do not record it.
#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub client: String,
    pub provider: String,
    pub model: String,
    /// As the ledger stores [`crate::adapters::Role`].
    pub role: String,
    /// The highest threshold the query was given that the prompt of these
    /// calls, `L + n_new`, exceeds; 0 when it exceeds none.
    pub prompt_above: u64,
    /// Whether `L` of these calls exceeds [`Split::cached_above`].
    pub cached_above: bool,
    pub calls: u64,
    pub cache_read: Tokens,
    /// The sum of `n_new`.
    pub new_input: Tokens,
    /// The sum of `n_out`, with the estimated output of calls without
    /// their final usage and of compactions without their usage.
    pub output: Tokens,
    pub estimated_output: Tokens,
    /// Of `cache_read` and `new_input`, the part estimated.
    pub estimated_input: Tokens,
    /// The sum over calls of `n_new * (L + n_new / 2)`, pairs of a new
    /// token and a token before it.
    pub prefill_pairs: f64,
    /// The sum over calls of `n_out * (L + n_new + n_out / 2)`, pairs of an
    /// output token and a token before it.
    pub decode_pairs: f64,
}

/// How [`Ledger::usage`] splits the calls besides client, provider, model
/// and role; the default splits them no further.
#[derive(Clone, Copy, Debug, Default)]
pub struct Split<'a> {
    /// Prompt thresholds, as [`Usage::prompt_above`].
    pub prompt_above: &'a [u64],
    /// A threshold of cache-read tokens, as [`Usage::cached_above`].
    pub cached_above: Option<u64>,
}

/// The calls of one session in one working directory over a time range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub client: String,
    pub session_id: String,
    /// `None` when the logs record no working directory.
    pub cwd: Option<String>,
    pub calls: u64,
    pub earliest: Timestamp,
    pub latest: Timestamp,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectUsage {
    /// As [`Attribution::project`].
    pub project: Option<String>,
    pub resolution: String,
    pub usage: Usage,
}

/// Over all time, not only a range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources {
    pub client: String,
    /// Every file ever read, including files deleted since.
    pub files: u64,
    pub unreadable_lines: u64,
    /// Files with lines but no usage: a sign of a changed log format.
    pub files_without_usage: u64,
}

impl Ledger {
    /// The range is `[since, until)`. Call times are kept to the
    /// millisecond, the precision of the logs.
    pub fn calls(
        &self,
        since: Option<Timestamp>,
        until: Option<Timestamp>,
        project: Option<&str>,
    ) -> Result<Vec<ClientCalls>, Error> {
        self.query_calls(&Selection::new(since, until, project))
            .map_err(|error| self.database(error))
    }

    fn query_calls(&self, selection: &Selection) -> rusqlite::Result<Vec<ClientCalls>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT client, count(*), count(DISTINCT session_id),
                    sum(uncached_input), sum(cache_write), sum(cache_read), sum(output),
                    sum(reasoning), count(reasoning), min(timestamp_ms), max(timestamp_ms),
                    count(CASE WHEN final_usage = 0 THEN 1 END),
                    coalesce(sum(CASE WHEN final_usage = 0 THEN output END), 0),
                    coalesce(sum(CASE WHEN final_usage = 0 THEN n_out - output END), 0),
                    count(CASE WHEN final_usage = 0 AND NOT estimable THEN 1 END),
                    count(CASE WHEN final_usage IS NULL THEN 1 END),
                    count(context_input),
                    coalesce(sum(CASE WHEN context_input IS NOT NULL
                                      THEN n_uncached_input - uncached_input END), 0),
                    coalesce(sum(CASE WHEN context_input IS NOT NULL
                                      THEN n_cache_read - cache_read END), 0),
                    coalesce(sum(CASE WHEN context_input IS NOT NULL THEN n_out - output END), 0),
                    count(CASE WHEN context_input IS NOT NULL AND NOT input_estimable THEN 1 END),
                    count(CASE WHEN context_input IS NOT NULL AND NOT output_estimable THEN 1 END)
             FROM {} GROUP BY client ORDER BY client",
            selection.from()
        ))?;
        let rows = statement.query_map(selection.params(), |row| {
            Ok(ClientCalls {
                client: row.get(0)?,
                calls: row.get(1)?,
                sessions: row.get(2)?,
                tokens: TokenClasses {
                    uncached_input: Tokens(row.get(3)?),
                    cache_write: Tokens(row.get(4)?),
                    cache_read: Tokens(row.get(5)?),
                    output: Tokens(row.get(6)?),
                    reasoning: row.get::<_, Option<u64>>(7)?.map(Tokens),
                },
                calls_with_reasoning: row.get(8)?,
                final_usage: FinalUsage {
                    missing: row.get(11)?,
                    missing_logged_output: Tokens(row.get(12)?),
                    estimated_missing_output: Tokens(row.get(13)?),
                    missing_without_estimate: row.get(14)?,
                    unknown: row.get(15)?,
                },
                unrecorded: Unrecorded {
                    calls: row.get(16)?,
                    estimated_uncached_input: Tokens(row.get(17)?),
                    estimated_cache_read: Tokens(row.get(18)?),
                    estimated_output: Tokens(row.get(19)?),
                    without_input_estimate: row.get(20)?,
                    without_output_estimate: row.get(21)?,
                },
                earliest: timestamp(row, 9)?,
                latest: timestamp(row, 10)?,
            })
        })?;
        rows.collect()
    }

    /// By client, then the version of the latest call first.
    pub fn client_versions(
        &self,
        since: Option<Timestamp>,
        until: Option<Timestamp>,
        project: Option<&str>,
    ) -> Result<Vec<VersionCalls>, Error> {
        self.query_client_versions(&Selection::new(since, until, project))
            .map_err(|error| self.database(error))
    }

    fn query_client_versions(&self, selection: &Selection) -> rusqlite::Result<Vec<VersionCalls>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT client, client_version, count(*), max(timestamp_ms)
             FROM {} GROUP BY client, client_version
             ORDER BY client, max(timestamp_ms) DESC, client_version",
            selection.from()
        ))?;
        let rows = statement.query_map(selection.params(), |row| {
            Ok(VersionCalls {
                client: row.get(0)?,
                version: row.get(1)?,
                calls: row.get(2)?,
                latest: timestamp(row, 3)?,
            })
        })?;
        rows.collect()
    }

    pub fn providers(
        &self,
        since: Option<Timestamp>,
        until: Option<Timestamp>,
        project: Option<&str>,
    ) -> Result<Vec<ProviderCalls>, Error> {
        self.query_providers(&Selection::new(since, until, project))
            .map_err(|error| self.database(error))
    }

    fn query_providers(&self, selection: &Selection) -> rusqlite::Result<Vec<ProviderCalls>> {
        // Session IDs are unique within a client, so a session is the pair.
        let mut statement = self.conn.prepare(&format!(
            "SELECT provider, count(*), count(DISTINCT client || char(0) || session_id),
                    sum(uncached_input), sum(cache_write), sum(cache_read), sum(output),
                    sum(reasoning)
             FROM {} GROUP BY provider ORDER BY provider",
            selection.from()
        ))?;
        let rows = statement.query_map(selection.params(), |row| {
            Ok(ProviderCalls {
                provider: row.get(0)?,
                calls: row.get(1)?,
                sessions: row.get(2)?,
                tokens: TokenClasses {
                    uncached_input: Tokens(row.get(3)?),
                    cache_write: Tokens(row.get(4)?),
                    cache_read: Tokens(row.get(5)?),
                    output: Tokens(row.get(6)?),
                    reasoning: row.get::<_, Option<u64>>(7)?.map(Tokens),
                },
            })
        })?;
        rows.collect()
    }

    /// The pair sums are computed per call and summed, because context and
    /// output length are correlated; products of sums or means would be
    /// wrong. They are floating point, so a long history cannot overflow.
    ///
    /// The calls are also split by the highest prompt threshold they
    /// exceed, so a price tier applies per call, and by whether their
    /// cache-read tokens exceed the threshold of `split`.
    pub fn usage(
        &self,
        since: Option<Timestamp>,
        until: Option<Timestamp>,
        project: Option<&str>,
        split: &Split,
    ) -> Result<Vec<Usage>, Error> {
        self.query_usage(&Selection::new(since, until, project), split)
            .map_err(|error| self.database(error))
    }

    fn query_usage(&self, selection: &Selection, split: &Split) -> rusqlite::Result<Vec<Usage>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {}, {USAGE_SUMS}
             FROM {} GROUP BY {USAGE_GROUPS} ORDER BY {USAGE_GROUPS}",
            usage_groups(split),
            selection.from()
        ))?;
        let rows = statement.query_map(selection.params(), |row| usage(row, 0))?;
        rows.collect()
    }

    /// Calls of a context [`Ledger::attribute`] was not given are left out.
    pub fn usage_by_project(
        &self,
        since: Option<Timestamp>,
        until: Option<Timestamp>,
        split: &Split,
    ) -> Result<Vec<ProjectUsage>, Error> {
        self.query_usage_by_project(&Selection::new(since, until, None), split)
            .map_err(|error| self.database(error))
    }

    fn query_usage_by_project(
        &self,
        selection: &Selection,
        split: &Split,
    ) -> rusqlite::Result<Vec<ProjectUsage>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT project, resolution, {}, {USAGE_SUMS}
             FROM estimated JOIN attribution
                 ON a_client = client AND a_session = session_id AND a_cwd IS cwd
             WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2
             GROUP BY project, resolution, {USAGE_GROUPS}
             ORDER BY project, resolution, {USAGE_GROUPS}",
            usage_groups(split)
        ))?;
        let rows = statement.query_map(selection.params(), |row| {
            Ok(ProjectUsage {
                project: row.get(0)?,
                resolution: row.get(1)?,
                usage: usage(row, 2)?,
            })
        })?;
        rows.collect()
    }

    pub fn contexts(
        &self,
        since: Option<Timestamp>,
        until: Option<Timestamp>,
    ) -> Result<Vec<Context>, Error> {
        self.query_contexts(&Selection::new(since, until, None))
            .map_err(|error| self.database(error))
    }

    fn query_contexts(&self, selection: &Selection) -> rusqlite::Result<Vec<Context>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT client, session_id, cwd, count(*), min(timestamp_ms), max(timestamp_ms)
             FROM {} GROUP BY client, session_id, cwd ORDER BY client, session_id, cwd",
            selection.from()
        ))?;
        let rows = statement.query_map(selection.params(), |row| {
            Ok(Context {
                client: row.get(0)?,
                session_id: row.get(1)?,
                cwd: row.get(2)?,
                calls: row.get(3)?,
                earliest: timestamp(row, 4)?,
                latest: timestamp(row, 5)?,
            })
        })?;
        rows.collect()
    }

    /// Starts a snapshot: from the next query until
    /// [`Ledger::end_snapshot`], every query sees the ledger as it was at
    /// that query, whatever other processes write meanwhile, so figures
    /// computed from several queries agree. [`Ledger::attribute`] may run
    /// inside it; another snapshot may not.
    pub fn begin_snapshot(&self) -> Result<(), Error> {
        self.conn
            .execute_batch("BEGIN DEFERRED")
            .map_err(|error| self.database(error))
    }

    pub fn end_snapshot(&self) -> Result<(), Error> {
        self.conn
            .execute_batch("COMMIT")
            .map_err(|error| self.database(error))
    }

    pub fn sources(&self) -> Result<Vec<Sources>, Error> {
        self.query_sources().map_err(|error| self.database(error))
    }

    fn query_sources(&self) -> rusqlite::Result<Vec<Sources>> {
        let mut statement = self.conn.prepare(
            "SELECT client, count(*), sum(unreadable_lines),
                    sum(lines > 0 AND usage_lines = 0)
             FROM files GROUP BY client ORDER BY client",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Sources {
                client: row.get(0)?,
                files: row.get(1)?,
                unreadable_lines: row.get(2)?,
                files_without_usage: row.get(3)?,
            })
        })?;
        rows.collect()
    }
}

/// The sums of [`Usage`] after its client, provider and model.
const USAGE_SUMS: &str = "count(*),
    sum(n_cache_read), sum(n_uncached_input + cache_write), sum(n_out),
    total((n_uncached_input + cache_write) * (n_cache_read + (n_uncached_input + cache_write) / 2.0)),
    total(n_out * (n_cache_read + n_uncached_input + cache_write + n_out / 2.0)),
    sum(n_out - output),
    sum(n_uncached_input - uncached_input + n_cache_read - cache_read)";

/// The columns [`usage_groups`] names.
const USAGE_GROUPS: &str = "client, provider, model, role, prompt_above, cached_above";

/// The thresholds are numbers, so they are written into the query.
fn usage_groups(split: &Split) -> String {
    let cached_above = match split.cached_above {
        Some(threshold) => format!("n_cache_read > {threshold}"),
        None => "0".to_owned(),
    };
    format!(
        "client, provider, model, role, {} AS prompt_above, {cached_above} AS cached_above",
        prompt_above(split.prompt_above)
    )
}

fn prompt_above(thresholds: &[u64]) -> String {
    use std::fmt::Write as _;
    let mut thresholds = thresholds.to_vec();
    thresholds.sort_unstable();
    thresholds.dedup();
    if thresholds.is_empty() {
        return "0".to_owned();
    }
    let mut sql = "CASE".to_owned();
    for threshold in thresholds.iter().rev() {
        let _ = write!(
            sql,
            " WHEN n_cache_read + n_uncached_input + cache_write > {threshold} THEN {threshold}"
        );
    }
    sql.push_str(" ELSE 0 END");
    sql
}

/// A [`Usage`] from a row holding the columns of [`USAGE_GROUPS`] from
/// column `first`, then [`USAGE_SUMS`].
fn usage(row: &rusqlite::Row, first: usize) -> rusqlite::Result<Usage> {
    Ok(Usage {
        client: row.get(first)?,
        provider: row.get(first + 1)?,
        model: row.get(first + 2)?,
        role: row.get(first + 3)?,
        prompt_above: row.get(first + 4)?,
        cached_above: row.get(first + 5)?,
        calls: row.get(first + 6)?,
        cache_read: Tokens(row.get(first + 7)?),
        new_input: Tokens(row.get(first + 8)?),
        output: Tokens(row.get(first + 9)?),
        prefill_pairs: row.get(first + 10)?,
        decode_pairs: row.get(first + 11)?,
        estimated_output: Tokens(row.get(first + 12)?),
        estimated_input: Tokens(row.get(first + 13)?),
    })
}

struct Selection<'a> {
    since: i64,
    until: i64,
    project: Option<&'a str>,
}

impl<'a> Selection<'a> {
    fn new(since: Option<Timestamp>, until: Option<Timestamp>, project: Option<&'a str>) -> Self {
        Self {
            since: since.map_or(i64::MIN, ceil_millisecond),
            until: until.map_or(i64::MAX, ceil_millisecond),
            project,
        }
    }

    fn from(&self) -> &'static str {
        if self.project.is_some() {
            IN_PROJECT
        } else {
            IN_RANGE
        }
    }

    fn params(&self) -> rusqlite::ParamsFromIter<Vec<rusqlite::types::Value>> {
        let mut values = vec![
            rusqlite::types::Value::Integer(self.since),
            rusqlite::types::Value::Integer(self.until),
        ];
        if let Some(project) = self.project {
            values.push(rusqlite::types::Value::Text(project.to_owned()));
        }
        rusqlite::params_from_iter(values)
    }
}

fn timestamp(row: &rusqlite::Row, index: usize) -> rusqlite::Result<Timestamp> {
    Timestamp::from_millisecond(row.get(index)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

/// For a time `t` stored in whole milliseconds, `t >= bound` exactly when
/// `t >= ceil_millisecond(bound)`, so a bound finer than a millisecond keeps
/// the range half-open.
fn ceil_millisecond(bound: Timestamp) -> i64 {
    let nanos = bound.as_nanosecond();
    let millis = nanos.div_euclid(NANOS_PER_MILLISECOND)
        + i128::from(nanos.rem_euclid(NANOS_PER_MILLISECOND) > 0);
    // Every Timestamp is within about 10^14 ms of the epoch.
    i64::try_from(millis).unwrap_or(i64::MAX)
}
