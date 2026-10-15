use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::quantity::{TokenClasses, Tokens};

pub(crate) mod claude_code;
pub(crate) mod codex;
pub(crate) mod coverage;
pub(crate) mod pi;
pub(crate) mod user;
pub(crate) mod walk;

/// How the session that made a call was run, as its logs show it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    Interactive,
    /// Such as `claude -p` or an SDK.
    Headless,
    Subagent,
}

impl Role {
    /// Stored in the ledger; must stay stable across releases.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Interactive => "interactive",
            Role::Headless => "headless",
            Role::Subagent => "subagent",
        }
    }
}

/// One model call: numbers and identifiers only, never content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// Unique per client across all of its files; copies of a call share it.
    pub id: String,
    pub provider: String,
    pub model: String,
    pub timestamp: Timestamp,
    pub session_id: String,
    /// The subagent within the session, for a client whose subagents share
    /// their parent's session ID; `None` otherwise.
    pub agent_id: Option<String>,
    pub cwd: Option<String>,
    pub role: Role,
    pub client_version: Option<String>,
    pub tokens: TokenClasses,
    pub completion: Completion,
    /// Whether the call summarises the session's context to replace it.
    pub compaction: bool,
}

/// Whether a line records the call's final usage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Completion {
    Final {
        stop_reason: Option<String>,
    },
    /// The usage at the moment the line was written; a later line of the
    /// same call carries more, and may never be written.
    Partial,
    /// The log records the call but none of its usage. `context_input` is
    /// the input, cache included, of the session's previous call, 0 when
    /// there is none.
    Unrecorded {
        context_input: Tokens,
    },
}

impl Completion {
    pub const FINAL: Completion = Completion::Final { stop_reason: None };
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a line is read and stored one at a time, so boxing would only add an allocation per call"
)]
pub enum Line {
    Call(Call),
    /// A line that is readable but records no usage.
    Other,
    /// Counted and reported, never dropped silently.
    Unreadable,
}

/// What a user adapter's file declares, so the ledger can tell when calls
/// were read under a different declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub file: PathBuf,
    /// The file's content without comments or formatting, in a canonical
    /// form: equal exactly when the declarations are.
    pub text: String,
}

pub trait Adapter {
    /// Stored with every call; must stay stable across releases.
    fn client(&self) -> &str;

    /// The declaration of a user adapter; `None` for a shipped adapter,
    /// whose rules change only with a release.
    fn definition(&self) -> Option<&Definition> {
        None
    }

    fn location(&self) -> &Path;

    /// [`Adapter::location`] as the adapter writes it before expansion,
    /// such as `~/.claude/projects`, which names no value of the user's;
    /// `None` when the location was given as a path.
    fn template(&self) -> Option<&str> {
        None
    }

    /// A missing log directory yields no files, not an error.
    fn discover(&self) -> walk::Walk;

    fn start(&self, file: &Path) -> Box<dyn Reader>;

    /// A reader for `file` from the line after the last one read by the
    /// reader that saved `saved`. Returns `None` when `saved` cannot be
    /// read, so the caller reads the file again with [`Adapter::start`].
    fn resume(&self, file: &Path, saved: &str) -> Option<Box<dyn Reader>>;
}

pub trait Reader {
    /// `line` is complete, without its line ending.
    fn parse_line(&mut self, line: &[u8]) -> Line;

    /// What a later reader needs to continue after the lines read so far;
    /// `None` when it needs nothing.
    fn save(&self) -> Option<String>;
}
