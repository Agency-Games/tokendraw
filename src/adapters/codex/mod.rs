//! A rollout records usage in two ways. Newer versions write a
//! `token_usage_record` per response, just before the `token_count` event
//! that adds it to the session's cumulative totals. Older versions write
//! only the `token_count` events, so a call is an increase of the totals.
//! A record is preferred because some responses never reach the totals:
//! compactions, whose `compacted` line follows their record and names it.
//! A `compacted` line that names no record is a compaction whose usage the
//! rollout never records.

use std::env;
use std::mem;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::adapters::walk;
use crate::adapters::{Adapter, Call, Completion, Line, Reader, Role};
use crate::quantity::{TokenClasses, Tokens};

pub struct Codex {
    sessions: PathBuf,
    template: Option<&'static str>,
}

impl Codex {
    pub fn from_env() -> Option<Self> {
        let (home, template) = match env::var_os("CODEX_HOME").filter(|dir| !dir.is_empty()) {
            Some(dir) => (PathBuf::from(dir), "$CODEX_HOME/sessions"),
            None => (env::home_dir()?.join(".codex"), "~/.codex/sessions"),
        };
        Some(Self {
            template: Some(template),
            ..Self::new(home.join("sessions"))
        })
    }

    pub fn new(sessions: PathBuf) -> Self {
        Self {
            sessions,
            template: None,
        }
    }
}

impl Adapter for Codex {
    fn client(&self) -> &'static str {
        "codex"
    }

    fn location(&self) -> &Path {
        &self.sessions
    }

    fn template(&self) -> Option<&str> {
        self.template
    }

    fn discover(&self) -> walk::Walk {
        let mut walk = walk::files_with_extension(&self.sessions, "jsonl");
        walk.files.retain(|file| {
            file.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("rollout-"))
        });
        walk
    }

    fn start(&self, _file: &Path) -> Box<dyn Reader> {
        Box::new(Rollout::default())
    }

    fn resume(&self, _file: &Path, saved: &str) -> Option<Box<dyn Reader>> {
        serde_json::from_str::<Rollout>(saved)
            .ok()
            .map(|rollout| Box::new(rollout) as Box<dyn Reader>)
    }
}

/// The reader's state, saved between ingestions.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Rollout {
    session: Option<Session>,
    /// From the latest turn context.
    model: Option<String>,
    /// From the latest turn context, else the session header.
    cwd: Option<String>,
    /// The cumulative totals of the latest `token_count` event.
    total: Option<Usage>,
    /// How many increases of the totals came so far; numbers the calls of
    /// a session without records.
    increases: u64,
    /// A record came after the latest `token_count` event, so the next
    /// increase is already a call.
    recorded: bool,
    /// Whether a fork's own first turn has started.
    own_turn: bool,
    /// The latest record, which the `compacted` line after it can name.
    #[serde(default)]
    latest_record: Option<LatestRecord>,
    /// The input of the latest call, recorded or an increase, copied
    /// calls of a fork included.
    #[serde(default)]
    latest_input: Option<u64>,
    /// How many `compacted` lines without a record came so far; numbers
    /// their calls.
    #[serde(default)]
    unrecorded_compactions: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct LatestRecord {
    response_id: String,
    timestamp: Timestamp,
    usage: Usage,
}

#[derive(Debug, Serialize, Deserialize)]
struct Session {
    id: String,
    provider: String,
    role: Role,
    /// When a fork was created, in whole seconds since the Unix epoch;
    /// `None` for a session that is not a fork.
    forked_at: Option<i64>,
    #[serde(default)]
    version: Option<String>,
}

/// Token counts as Codex records them: `input_tokens` includes
/// `cached_input_tokens` and `cache_write_input_tokens`, and
/// `output_tokens` includes `reasoning_output_tokens`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Usage {
    #[serde(rename = "input_tokens")]
    input: u64,
    #[serde(rename = "cached_input_tokens", default)]
    cached_input: u64,
    #[serde(rename = "cache_write_input_tokens", default)]
    cache_write_input: u64,
    #[serde(rename = "output_tokens")]
    output: u64,
    #[serde(rename = "reasoning_output_tokens")]
    reasoning_output: Option<u64>,
    #[serde(rename = "total_tokens")]
    total: u64,
}

impl Usage {
    /// When the total falls, the session restarted counting, and all of
    /// `self` is new.
    fn since(&self, earlier: Option<&Usage>) -> Usage {
        let Some(earlier) = earlier.filter(|earlier| self.total >= earlier.total) else {
            return self.clone();
        };
        Usage {
            input: self.input.saturating_sub(earlier.input),
            cached_input: self.cached_input.saturating_sub(earlier.cached_input),
            cache_write_input: self
                .cache_write_input
                .saturating_sub(earlier.cache_write_input),
            output: self.output.saturating_sub(earlier.output),
            reasoning_output: self
                .reasoning_output
                .zip(earlier.reasoning_output)
                .map(|(now, before)| now.saturating_sub(before)),
            total: self.total - earlier.total,
        }
    }

    fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0
    }

    fn classes(&self) -> TokenClasses {
        TokenClasses {
            uncached_input: Tokens(
                self.input
                    .saturating_sub(self.cached_input)
                    .saturating_sub(self.cache_write_input),
            ),
            cache_write: Tokens(self.cache_write_input),
            cache_read: Tokens(self.cached_input),
            output: Tokens(self.output),
            reasoning: self.reasoning_output.map(Tokens),
        }
    }
}

#[derive(Deserialize)]
struct Kind {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Event {
    payload: Kind,
}

#[derive(Deserialize)]
struct Entry<T> {
    timestamp: Timestamp,
    payload: T,
}

#[derive(Deserialize)]
struct SessionMeta {
    id: String,
    timestamp: Timestamp,
    model_provider: String,
    cwd: Option<String>,
    source: Option<serde_json::Value>,
    thread_source: Option<String>,
    forked_from_id: Option<String>,
    cli_version: Option<String>,
}

#[derive(Deserialize)]
struct TurnContext {
    model: String,
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct TaskStarted {
    /// Seconds since the Unix epoch.
    started_at: Option<i64>,
}

#[derive(Deserialize)]
struct TokenCount {
    info: Option<Info>,
}

#[derive(Deserialize)]
struct Info {
    total_token_usage: Usage,
}

#[derive(Deserialize)]
struct Compacted {
    compaction_response_id: Option<String>,
}

#[derive(Deserialize)]
struct Record {
    response_id: String,
    usage: Usage,
}

impl Reader for Rollout {
    fn parse_line(&mut self, line: &[u8]) -> Line {
        // Most lines are conversation items, so only the type is read before
        // the line's own shape.
        let Ok(Kind { kind }) = serde_json::from_slice::<Kind>(line) else {
            return Line::Unreadable;
        };
        match kind.as_deref() {
            Some("session_meta") => self.session_meta(line),
            Some("turn_context") => self.turn_context(line),
            Some("token_usage_record") => self.record(line),
            Some("compacted") => self.compacted(line),
            Some("event_msg") => match serde_json::from_slice::<Event>(line) {
                Ok(event) => match event.payload.kind.as_deref() {
                    Some("token_count") => self.token_count(line),
                    Some("task_started") => self.task_started(line),
                    _ => Line::Other,
                },
                Err(_) => Line::Unreadable,
            },
            _ => Line::Other,
        }
    }

    fn save(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }
}

impl Rollout {
    fn session_meta(&mut self, line: &[u8]) -> Line {
        // A fork copies its parent's header after its own.
        if self.session.is_some() {
            return Line::Other;
        }
        let Ok(Entry { payload: meta, .. }) = serde_json::from_slice::<Entry<SessionMeta>>(line)
        else {
            return Line::Unreadable;
        };
        let role = if meta.thread_source.as_deref() == Some("subagent") {
            Role::Subagent
        } else if meta.source.as_ref().and_then(|source| source.as_str()) == Some("exec") {
            Role::Headless
        } else {
            Role::Interactive
        };
        self.session = Some(Session {
            id: meta.id,
            provider: meta.model_provider,
            role,
            forked_at: meta.forked_from_id.map(|_| meta.timestamp.as_second()),
            version: meta.cli_version,
        });
        self.cwd = meta.cwd;
        Line::Other
    }

    fn turn_context(&mut self, line: &[u8]) -> Line {
        let Ok(Entry { payload: turn, .. }) = serde_json::from_slice::<Entry<TurnContext>>(line)
        else {
            return Line::Unreadable;
        };
        self.model = Some(turn.model);
        if turn.cwd.is_some() {
            self.cwd = turn.cwd;
        }
        Line::Other
    }

    /// A fork's copied history keeps the original start times of its turns,
    /// which are before the fork was created.
    fn task_started(&mut self, line: &[u8]) -> Line {
        let Ok(Entry { payload: task, .. }) = serde_json::from_slice::<Entry<TaskStarted>>(line)
        else {
            return Line::Unreadable;
        };
        let forked_at = self.session.as_ref().and_then(|session| session.forked_at);
        if let (Some(forked_at), Some(started_at)) = (forked_at, task.started_at)
            && started_at >= forked_at
        {
            self.own_turn = true;
        }
        Line::Other
    }

    fn record(&mut self, line: &[u8]) -> Line {
        let Ok(Entry { timestamp, payload }) = serde_json::from_slice::<Entry<Record>>(line) else {
            return Line::Unreadable;
        };
        self.recorded = true;
        self.latest_input = Some(payload.usage.input);
        let line = self.call(
            payload.response_id.clone(),
            timestamp,
            payload.usage.classes(),
            Completion::FINAL,
            false,
        );
        self.latest_record = Some(LatestRecord {
            response_id: payload.response_id,
            timestamp,
            usage: payload.usage,
        });
        line
    }

    /// A compaction that names the record before it marks that record's
    /// call; one that names none is a call without usage.
    fn compacted(&mut self, line: &[u8]) -> Line {
        let Ok(Entry { timestamp, payload }) = serde_json::from_slice::<Entry<Compacted>>(line)
        else {
            return Line::Unreadable;
        };
        if let Some(id) = payload.compaction_response_id {
            return match &self.latest_record {
                Some(record) if record.response_id == id => self.call(
                    id,
                    record.timestamp,
                    record.usage.classes(),
                    Completion::FINAL,
                    true,
                ),
                _ => Line::Other,
            };
        }
        self.unrecorded_compactions += 1;
        let Some(session) = &self.session else {
            return Line::Unreadable;
        };
        let id = format!(
            "{}#compaction-{}@{timestamp}",
            session.id, self.unrecorded_compactions
        );
        let context_input = Tokens(self.latest_input.unwrap_or(0));
        self.call(
            id,
            timestamp,
            TokenClasses::default(),
            Completion::Unrecorded { context_input },
            true,
        )
    }

    fn token_count(&mut self, line: &[u8]) -> Line {
        let Ok(Entry { timestamp, payload }) = serde_json::from_slice::<Entry<TokenCount>>(line)
        else {
            return Line::Unreadable;
        };
        let Some(info) = payload.info else {
            return Line::Other;
        };
        let total = info.total_token_usage;
        let increase = total.since(self.total.as_ref());
        self.total = Some(total);
        let recorded = mem::take(&mut self.recorded);
        if increase.is_empty() {
            return Line::Other;
        }
        self.increases += 1;
        if recorded {
            return Line::Other;
        }
        let Some(session) = &self.session else {
            return Line::Unreadable;
        };
        let id = format!("{}#{}", session.id, self.increases);
        self.latest_input = Some(increase.input);
        self.call(id, timestamp, increase.classes(), Completion::FINAL, false)
    }

    /// Usage in a fork before its own first turn is its parent's, copied,
    /// and may come before any turn context.
    fn call(
        &self,
        id: String,
        timestamp: Timestamp,
        tokens: TokenClasses,
        completion: Completion,
        compaction: bool,
    ) -> Line {
        let Some(session) = &self.session else {
            return Line::Unreadable;
        };
        if session.forked_at.is_some() && !self.own_turn {
            return Line::Other;
        }
        let Some(model) = &self.model else {
            return Line::Unreadable;
        };
        Line::Call(Call {
            id,
            provider: session.provider.clone(),
            model: model.clone(),
            timestamp,
            session_id: session.id.clone(),
            cwd: self.cwd.clone(),
            role: session.role,
            agent_id: None,
            client_version: session.version.clone(),
            tokens,
            completion,
            compaction,
        })
    }
}

#[cfg(test)]
mod tests;
