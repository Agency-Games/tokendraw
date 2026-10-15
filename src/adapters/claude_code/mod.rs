use std::env;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Deserialize;

use crate::adapters::walk;
use crate::adapters::{Adapter, Call, Completion, Line, Reader, Role};
use crate::quantity::{TokenClasses, Tokens};

/// Claude Code writes `model: "<synthetic>"` for messages it creates itself,
/// such as error notices; no model ran for them.
const SYNTHETIC_MODEL: &str = "<synthetic>";

pub struct ClaudeCode {
    projects: PathBuf,
    template: Option<&'static str>,
}

impl ClaudeCode {
    pub fn from_env() -> Option<Self> {
        let (config, template) =
            match env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
                Some(dir) => (PathBuf::from(dir), "$CLAUDE_CONFIG_DIR/projects"),
                None => (env::home_dir()?.join(".claude"), "~/.claude/projects"),
            };
        Some(Self {
            template: Some(template),
            ..Self::new(config.join("projects"))
        })
    }

    pub fn new(projects: PathBuf) -> Self {
        Self {
            projects,
            template: None,
        }
    }

    /// Subagent transcripts live in a `subagents/` directory beside their
    /// parent session's transcript.
    fn is_subagent(&self, file: &Path) -> bool {
        file.strip_prefix(&self.projects)
            .unwrap_or(file)
            .components()
            .any(|part| part.as_os_str() == "subagents")
    }
}

#[derive(Deserialize)]
struct Kind {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Assistant {
    message: Message,
    timestamp: Timestamp,
    #[serde(rename = "sessionId")]
    session_id: String,
    /// In subagent transcripts, which carry their parent's session ID.
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
    cwd: Option<String>,
    entrypoint: Option<String>,
    version: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    id: String,
    model: String,
    usage: Usage,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    output_tokens: u64,
    output_tokens_details: Option<OutputDetails>,
}

#[derive(Deserialize)]
struct OutputDetails {
    thinking_tokens: Option<u64>,
}

impl Adapter for ClaudeCode {
    fn client(&self) -> &'static str {
        "claude-code"
    }

    fn location(&self) -> &Path {
        &self.projects
    }

    fn template(&self) -> Option<&str> {
        self.template
    }

    fn discover(&self) -> walk::Walk {
        walk::files_with_extension(&self.projects, "jsonl")
    }

    fn start(&self, file: &Path) -> Box<dyn Reader> {
        Box::new(Transcript {
            subagent: self.is_subagent(file),
        })
    }

    /// Transcripts save no state, so every reader starts the same way.
    fn resume(&self, file: &Path, _saved: &str) -> Option<Box<dyn Reader>> {
        Some(self.start(file))
    }
}

struct Transcript {
    subagent: bool,
}

impl Reader for Transcript {
    fn parse_line(&mut self, line: &[u8]) -> Line {
        // Most lines are not assistant messages, and their shapes vary by
        // type, so only the type is read before the full message.
        match serde_json::from_slice::<Kind>(line) {
            Ok(Kind {
                kind: Some(kind), ..
            }) if kind == "assistant" => {}
            Ok(_) => return Line::Other,
            Err(_) => return Line::Unreadable,
        }
        let Ok(entry) = serde_json::from_slice::<Assistant>(line) else {
            return Line::Unreadable;
        };
        if entry.message.model == SYNTHETIC_MODEL {
            return Line::Other;
        }
        let role = if self.subagent {
            Role::Subagent
        } else if entry
            .entrypoint
            .is_some_and(|entrypoint| entrypoint.starts_with("sdk"))
        {
            // `claude -p` records `sdk-cli`; the Agent SDKs record `sdk-py`
            // and `sdk-ts`.
            Role::Headless
        } else {
            Role::Interactive
        };
        let usage = entry.message.usage;
        // Each content block of a response is a line of its own, and only
        // the last, with the stop reason, carries the final usage.
        let completion = match entry.message.stop_reason {
            Some(reason) => Completion::Final {
                stop_reason: Some(reason),
            },
            None => Completion::Partial,
        };
        Line::Call(Call {
            id: entry.message.id,
            provider: "anthropic".to_owned(),
            model: entry.message.model,
            timestamp: entry.timestamp,
            session_id: entry.session_id,
            agent_id: entry.agent_id.filter(|_| self.subagent),
            cwd: entry.cwd,
            role,
            client_version: entry.version,
            tokens: TokenClasses {
                uncached_input: Tokens(usage.input_tokens),
                cache_write: Tokens(usage.cache_creation_input_tokens),
                cache_read: Tokens(usage.cache_read_input_tokens),
                output: Tokens(usage.output_tokens),
                reasoning: usage
                    .output_tokens_details
                    .and_then(|details| details.thinking_tokens)
                    .map(Tokens),
            },
            completion,
            compaction: false,
        })
    }

    fn save(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests;
