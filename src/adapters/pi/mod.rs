//! Each assistant message records its own usage, provider and model. The
//! session's ID and working directory are in the header on the first line.
//! A fork's header names its parent's file, and the fork copies the
//! parent's entries with their original timestamps, which are at or before
//! the fork's header.

use std::env;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::adapters::walk;
use crate::adapters::{Adapter, Call, Completion, Line, Reader, Role};
use crate::quantity::{TokenClasses, Tokens};

pub struct Pi {
    sessions: PathBuf,
    template: Option<&'static str>,
}

impl Pi {
    /// The same lookup, in the same order, as pi's own, which skips
    /// variables that are unset or empty.
    pub fn from_env() -> Option<Self> {
        let set = |name| env::var_os(name).filter(|dir| !dir.is_empty());
        let (sessions, template) = if let Some(dir) = set("PI_CODING_AGENT_SESSION_DIR") {
            (PathBuf::from(dir), "$PI_CODING_AGENT_SESSION_DIR")
        } else if let Some(dir) = set("PI_CODING_AGENT_DIR") {
            (
                PathBuf::from(dir).join("sessions"),
                "$PI_CODING_AGENT_DIR/sessions",
            )
        } else {
            (
                env::home_dir()?.join(".pi/agent/sessions"),
                "~/.pi/agent/sessions",
            )
        };
        Some(Self {
            template: Some(template),
            ..Self::new(sessions)
        })
    }

    pub fn new(sessions: PathBuf) -> Self {
        Self {
            sessions,
            template: None,
        }
    }
}

impl Adapter for Pi {
    fn client(&self) -> &'static str {
        "pi"
    }

    fn location(&self) -> &Path {
        &self.sessions
    }

    fn template(&self) -> Option<&str> {
        self.template
    }

    fn discover(&self) -> walk::Walk {
        walk::files_with_extension(&self.sessions, "jsonl")
    }

    fn start(&self, _file: &Path) -> Box<dyn Reader> {
        Box::new(SessionFile::default())
    }

    fn resume(&self, _file: &Path, saved: &str) -> Option<Box<dyn Reader>> {
        serde_json::from_str::<SessionFile>(saved)
            .ok()
            .map(|file| Box::new(file) as Box<dyn Reader>)
    }
}

/// The reader's state, saved between ingestions.
#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionFile {
    session: Option<Session>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Session {
    id: String,
    cwd: Option<String>,
    forked_at: Option<Timestamp>,
}

#[derive(Deserialize)]
struct Kind {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    id: String,
    timestamp: Timestamp,
    cwd: Option<String>,
    #[serde(rename = "parentSession")]
    parent_session: Option<String>,
}

#[derive(Deserialize)]
struct Entry {
    /// Unique within its session only.
    id: String,
    timestamp: Timestamp,
    message: Message,
}

#[derive(Deserialize)]
struct Author {
    role: Option<String>,
}

#[derive(Deserialize)]
struct Said {
    message: Author,
}

#[derive(Deserialize)]
struct Message {
    provider: String,
    model: String,
    usage: Usage,
    #[serde(rename = "responseId")]
    response_id: Option<String>,
}

/// Token counts as pi records them: the classes do not overlap, and no
/// reasoning count is recorded.
#[derive(Deserialize)]
struct Usage {
    input: u64,
    output: u64,
    #[serde(rename = "cacheRead")]
    cache_read: u64,
    #[serde(rename = "cacheWrite")]
    cache_write: u64,
}

impl Reader for SessionFile {
    fn parse_line(&mut self, line: &[u8]) -> Line {
        // Most lines are conversation entries, so only the type is read
        // before the line's own shape.
        let Ok(Kind { kind }) = serde_json::from_slice::<Kind>(line) else {
            return Line::Unreadable;
        };
        match kind.as_deref() {
            Some("session") => self.header(line),
            Some("message") => self.message(line),
            _ => Line::Other,
        }
    }

    fn save(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }
}

impl SessionFile {
    fn header(&mut self, line: &[u8]) -> Line {
        let Ok(header) = serde_json::from_slice::<Header>(line) else {
            return Line::Unreadable;
        };
        self.session = Some(Session {
            id: header.id,
            cwd: header.cwd,
            forked_at: header.parent_session.map(|_| header.timestamp),
        });
        Line::Other
    }

    fn message(&self, line: &[u8]) -> Line {
        match serde_json::from_slice::<Said>(line) {
            Ok(Said {
                message: Author { role: Some(role) },
            }) if role == "assistant" => {}
            Ok(_) => return Line::Other,
            Err(_) => return Line::Unreadable,
        }
        let (Ok(entry), Some(session)) = (serde_json::from_slice::<Entry>(line), &self.session)
        else {
            return Line::Unreadable;
        };
        let usage = entry.message.usage;
        // pi records a failed or aborted request as a message without usage.
        if usage.input == 0 && usage.output == 0 && usage.cache_read == 0 && usage.cache_write == 0
        {
            return Line::Other;
        }
        // A copy is counted from its parent's file, so its session and
        // directory do not depend on which file is read first.
        if session
            .forked_at
            .is_some_and(|forked_at| entry.timestamp <= forked_at)
        {
            return Line::Other;
        }
        let id = entry
            .message
            .response_id
            .unwrap_or_else(|| format!("{}#{}", session.id, entry.id));
        Line::Call(Call {
            id,
            provider: entry.message.provider,
            model: entry.message.model,
            timestamp: entry.timestamp,
            session_id: session.id.clone(),
            cwd: session.cwd.clone(),
            role: Role::Interactive,
            client_version: None,
            tokens: TokenClasses {
                uncached_input: Tokens(usage.input),
                cache_write: Tokens(usage.cache_write),
                cache_read: Tokens(usage.cache_read),
                output: Tokens(usage.output),
                reasoning: None,
            },
            completion: Completion::FINAL,
            compaction: false,
        })
    }
}

#[cfg(test)]
mod tests;
