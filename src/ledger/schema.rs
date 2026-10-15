pub(super) const SCHEMA_VERSION: i32 = 8;

pub(super) const SCHEMA: &str = "
CREATE TABLE calls (
    client TEXT NOT NULL,
    call_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    cwd TEXT,
    role TEXT NOT NULL,
    uncached_input INTEGER NOT NULL,
    cache_write INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    output INTEGER NOT NULL,
    reasoning INTEGER,
    final_usage INTEGER,
    stop_reason TEXT,
    first_output INTEGER,
    compaction INTEGER NOT NULL DEFAULT 0,
    context_input INTEGER,
    client_version TEXT,
    agent_id TEXT,
    PRIMARY KEY (client, call_id)
);
CREATE INDEX calls_by_time ON calls (timestamp_ms);
CREATE INDEX calls_by_thread ON calls (client, session_id, agent_id, timestamp_ms);
CREATE TABLE files (
    client TEXT NOT NULL,
    path TEXT NOT NULL,
    device INTEGER NOT NULL,
    inode INTEGER NOT NULL,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    offset INTEGER NOT NULL,
    lines INTEGER NOT NULL,
    unreadable_lines INTEGER NOT NULL,
    usage_lines INTEGER NOT NULL,
    state TEXT,
    PRIMARY KEY (client, path)
);
CREATE TABLE definitions (
    client TEXT PRIMARY KEY,
    definition TEXT NOT NULL
);
CREATE TABLE directories (
    path TEXT PRIMARY KEY,
    checkout TEXT NOT NULL,
    observed_ms INTEGER NOT NULL
);
CREATE TABLE labels (
    kind TEXT NOT NULL,
    client TEXT NOT NULL,
    target TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (kind, client, target, key)
);
";

/// Where each context of calls in a report is attributed, written by
/// [`Ledger::attribute`], and the calls with their estimated tokens; both
/// live as long as the connection.
///
/// A call without its final usage (`final_usage = 0`) gets its first
/// output plus the mean growth from first to final output of the complete
/// calls of the same client, role and model that stopped to use a tool,
/// over the whole ledger, or keeps its logged output when that is higher.
/// The calls without a final usage line seen so far all end in a tool
/// call. A group with fewer than 30 such calls gives no estimate.
///
/// A compaction whose usage the log never records (`context_input` set)
/// gets the input of the session's previous call, split into uncached
/// input and cache read by the uncached share of the recorded compactions
/// of the same client and model, else of the same client, and the mean
/// output of the recorded compactions of the same client and model, over
/// the whole ledger. A group with fewer than 30 recorded compactions gives
/// no estimate, and a compaction without a previous call no input.
///
/// The `fixed` view, read through [`IN_RANGE_FIXED`] and
/// [`IN_PROJECT_FIXED`], gives each call its fixed context `f`: the context
/// (`n_uncached_input + cache_write + n_cache_read`) of the first call of
/// its thread, up to its own. A thread is a session, or a subagent of one
/// ([`crate::adapters::Call`]'s `agent_id`). Its first call is found over
/// the whole ledger, so a range that begins within a session still finds
/// it; of calls in the same millisecond, the smaller counts. Only a first
/// call without recorded usage, such as a fork's compaction, reads its
/// estimate, which is slow to compute for every call. `r_cache_read` and
/// `r_new_input` are the call's tokens without `f`, taken from the
/// cache-read tokens first.
pub(super) const ATTRIBUTION: &str = "
CREATE TEMP TABLE attribution (
    a_client TEXT NOT NULL,
    a_session TEXT NOT NULL,
    a_cwd TEXT,
    project TEXT,
    resolution TEXT NOT NULL
);
CREATE INDEX temp.attribution_by_context ON attribution (a_client, a_session, a_cwd);
CREATE TEMP VIEW estimated AS
SELECT *,
       CASE WHEN input_estimable THEN CAST(round(context_input * share) AS INTEGER)
            ELSE uncached_input END AS n_uncached_input,
       CASE WHEN input_estimable THEN context_input - CAST(round(context_input * share) AS INTEGER)
            ELSE cache_read END AS n_cache_read
FROM (
    SELECT calls.*,
           CASE WHEN context_input IS NOT NULL THEN coalesce(CAST(round(m_output) AS INTEGER), 0)
                WHEN final_usage = 0 AND growth IS NOT NULL
                THEN max(output, first_output + CAST(round(growth) AS INTEGER))
                ELSE output END AS n_out,
           growth IS NOT NULL AS estimable,
           coalesce(m_share, c_share) AS share,
           context_input > 0 AND coalesce(m_share, c_share) IS NOT NULL AS input_estimable,
           m_output IS NOT NULL AS output_estimable
    FROM calls LEFT JOIN (
        SELECT client AS g_client, role AS g_role, model AS g_model,
               avg(output - first_output) AS growth
        FROM calls WHERE final_usage = 1 AND stop_reason = 'tool_use'
        GROUP BY client, role, model HAVING count(*) >= 30
    ) ON g_client = client AND g_role = role AND g_model = model
    LEFT JOIN (
        SELECT client AS m_client, model AS m_model, avg(output) AS m_output,
               CAST(sum(uncached_input) AS REAL) / sum(uncached_input + cache_write + cache_read) AS m_share
        FROM calls WHERE compaction = 1 AND context_input IS NULL
        GROUP BY client, model HAVING count(*) >= 30
    ) ON m_client = client AND m_model = model
    LEFT JOIN (
        SELECT client AS c_client,
               CAST(sum(uncached_input) AS REAL) / sum(uncached_input + cache_write + cache_read) AS c_share
        FROM calls WHERE compaction = 1 AND context_input IS NULL
        GROUP BY client HAVING count(*) >= 30
    ) ON c_client = client
);
CREATE TEMP VIEW fixed AS
SELECT *, n_cache_read - min(f, n_cache_read) AS r_cache_read,
       n_uncached_input + cache_write - (f - min(f, n_cache_read)) AS r_new_input
FROM (
    SELECT estimated.*, min(start, n_cache_read + n_uncached_input + cache_write) AS f
    FROM estimated JOIN (
        SELECT c.client AS t_client, c.session_id AS t_session, c.agent_id AS t_agent,
               min(CASE WHEN c.context_input IS NULL
                        THEN c.uncached_input + c.cache_write + c.cache_read
                        ELSE (SELECT n_uncached_input + cache_write + n_cache_read
                              FROM estimated AS e
                              WHERE e.client = c.client AND e.call_id = c.call_id) END) AS start
        FROM (SELECT client, session_id, agent_id, min(timestamp_ms) AS first_ms
              FROM calls GROUP BY client, session_id, agent_id) AS t
        JOIN calls AS c ON c.client = t.client AND c.session_id = t.session_id
            AND c.agent_id IS t.agent_id AND c.timestamp_ms = t.first_ms
        GROUP BY c.client, c.session_id, c.agent_id
    ) ON t_client = client AND t_session = session_id AND t_agent IS agent_id
);
";

pub(super) const IN_RANGE: &str = "estimated WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2";

/// A working directory matches with `IS`, so calls without one are
/// attributed too.
pub(super) const IN_PROJECT: &str = "estimated JOIN attribution
    ON a_client = client AND a_session = session_id AND a_cwd IS cwd
    WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2 AND project = ?3";

pub(super) const IN_RANGE_FIXED: &str = "fixed WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2";

pub(super) const IN_PROJECT_FIXED: &str = "fixed JOIN attribution
    ON a_client = client AND a_session = session_id AND a_cwd IS cwd
    WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2 AND project = ?3";

/// `MIGRATIONS[n]` takes schema version `n + 1` to `n + 2`.
pub(super) const MIGRATIONS: [&str; 7] = [
    "ALTER TABLE files ADD COLUMN state TEXT;",
    "CREATE TABLE definitions (client TEXT PRIMARY KEY, definition TEXT NOT NULL);",
    "CREATE TABLE directories (path TEXT PRIMARY KEY, checkout TEXT NOT NULL,
                               observed_ms INTEGER NOT NULL);
     CREATE TABLE labels (kind TEXT NOT NULL, client TEXT NOT NULL, target TEXT NOT NULL,
                          key TEXT NOT NULL, value TEXT NOT NULL,
                          PRIMARY KEY (kind, client, target, key));",
    // Schema 5 records whether a call's usage is final. Every other client
    // writes one final record per call. Claude Code's logs are read again
    // from the start, as replaced files, to fill the columns; calls of logs
    // deleted since stay unknown.
    "ALTER TABLE calls ADD COLUMN final_usage INTEGER;
     ALTER TABLE calls ADD COLUMN stop_reason TEXT;
     ALTER TABLE calls ADD COLUMN first_output INTEGER;
     UPDATE calls SET final_usage = 1, first_output = output WHERE client <> 'claude-code';
     UPDATE files SET device = -1, inode = -1 WHERE client = 'claude-code';",
    // Schema 6 records compactions, and those whose usage the log never
    // records. Codex's logs are read again from the start, as replaced
    // files, to find them; compactions of logs deleted since stay unseen.
    "ALTER TABLE calls ADD COLUMN compaction INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE calls ADD COLUMN context_input INTEGER;
     UPDATE files SET device = -1, inode = -1 WHERE client = 'codex';",
    // Schema 7 records the version of the client that wrote each call.
    // Claude Code's and Codex's logs are read again from the start, as
    // replaced files, to fill it; calls of logs deleted since keep none.
    "ALTER TABLE calls ADD COLUMN client_version TEXT;
     UPDATE files SET device = -1, inode = -1 WHERE client IN ('claude-code', 'codex');",
    // Schema 8 records the subagent of each Claude Code subagent call, whose
    // transcript carries its parent's session ID. Claude Code's logs are
    // read again from the start, as replaced files, to fill it; calls of
    // logs deleted since keep none, so their subagents share one thread.
    "ALTER TABLE calls ADD COLUMN agent_id TEXT;
     CREATE INDEX calls_by_thread ON calls (client, session_id, agent_id, timestamp_ms);
     UPDATE files SET device = -1, inode = -1 WHERE client = 'claude-code';",
];

// A copy of a call keeps the highest value per token field, because output
// grows while a response streams, and the earliest timestamp with that
// copy's session, directory, role and client version, because resumed and
// forked sessions copy earlier calls; a copy without a version takes any
// other copy's until a copy as early as the call has one, as when a ledger
// of schema 6 reads its logs again. The agent follows the earliest copy
// too; a copy as early fills a missing agent only from the same session, so
// session and agent come from one copy, as when a ledger of schema 7 reads
// its logs again. The usage is final once a copy is; a call of unknown
// completion (read before schema 5) becomes partial only when a partial copy
// accounts for all the output it kept, which may have come from a final copy
// in a log deleted since. A call is a compaction once a copy is. SQLite
// evaluates every expression against the row as it was before the update.
pub(super) const UPSERT_CALL: &str = "
INSERT INTO calls (client, call_id, provider, model, timestamp_ms, session_id, cwd, role,
                   uncached_input, cache_write, cache_read, output, reasoning,
                   final_usage, stop_reason, first_output, compaction, context_input,
                   client_version, agent_id)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?12, ?16, ?17, ?18, ?19)
ON CONFLICT (client, call_id) DO UPDATE SET
    provider = iif(excluded.timestamp_ms < timestamp_ms, excluded.provider, provider),
    model = iif(excluded.timestamp_ms < timestamp_ms, excluded.model, model),
    session_id = iif(excluded.timestamp_ms < timestamp_ms, excluded.session_id, session_id),
    agent_id = CASE WHEN excluded.timestamp_ms < timestamp_ms THEN excluded.agent_id
                    WHEN excluded.timestamp_ms = timestamp_ms AND excluded.session_id = session_id
                    THEN coalesce(agent_id, excluded.agent_id)
                    ELSE agent_id END,
    cwd = iif(excluded.timestamp_ms < timestamp_ms, excluded.cwd, cwd),
    role = iif(excluded.timestamp_ms < timestamp_ms, excluded.role, role),
    client_version = iif(excluded.timestamp_ms <= timestamp_ms,
                         coalesce(excluded.client_version, client_version),
                         coalesce(client_version, excluded.client_version)),
    timestamp_ms = min(timestamp_ms, excluded.timestamp_ms),
    uncached_input = max(uncached_input, excluded.uncached_input),
    cache_write = max(cache_write, excluded.cache_write),
    cache_read = max(cache_read, excluded.cache_read),
    output = max(output, excluded.output),
    reasoning = max(coalesce(reasoning, excluded.reasoning), coalesce(excluded.reasoning, reasoning)),
    final_usage = CASE
        WHEN final_usage = 1 OR excluded.final_usage = 1 THEN 1
        WHEN final_usage = 0 OR output <= excluded.output THEN 0
    END,
    stop_reason = coalesce(stop_reason, excluded.stop_reason),
    first_output = min(coalesce(first_output, excluded.first_output), excluded.first_output),
    compaction = max(compaction, excluded.compaction),
    context_input = max(coalesce(context_input, excluded.context_input),
                        coalesce(excluded.context_input, context_input))
";

pub(super) const SAVE_CHECKPOINT: &str = "
INSERT OR REPLACE INTO files (client, path, device, inode, size, mtime_ns, offset,
                              lines, unreadable_lines, usage_lines, state)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
";

pub(super) const LOAD_CHECKPOINT: &str = "
SELECT device, inode, size, mtime_ns, offset, lines, unreadable_lines, usage_lines, state
FROM files WHERE client = ?1 AND path = ?2
";
