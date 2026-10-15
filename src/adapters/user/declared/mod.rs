//! The adapter a user's file declares with `[files]`. Fields are named by
//! JSON Pointers (RFC 6901).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::adapters::walk;
use crate::adapters::{Adapter, Call, Completion, Definition, Line, Reader, Role};
use crate::quantity::{TokenClasses, Tokens};

/// How a call line records usage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Usage {
    /// Each call line holds one call's own usage. Lines with the same ID are
    /// one call, with the highest value of each token class, as a response
    /// logged again while it streams or copied into another file is.
    #[default]
    PerCall,
    /// Each call line holds the file's running totals; a call is the
    /// increase over the previous call line.
    Cumulative,
}

/// The lines that are calls, and the fields that make up a call.
#[derive(Debug)]
pub struct Spec {
    /// JSON Pointer to the value it must equal.
    pub when: BTreeMap<String, Value>,
    /// Without it, IDs are generated from the file and the line.
    pub id: Option<String>,
    /// An RFC 3339 time.
    pub timestamp: String,
    /// Without it, the session is the file's path below the root.
    pub session: Option<String>,
    pub cwd: Option<String>,
    pub provider: Text,
    pub model: Text,
    pub role: Role,
    pub tokens: TokenFields,
    pub usage: Usage,
}

/// A text field read from each line, or the same for every call.
#[derive(Debug)]
pub enum Text {
    Field(String),
    Value(String),
}

/// JSON Pointers to the token counts of a call line, and how the log
/// counts them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenFields {
    pub input: String,
    pub output: String,
    pub cache_read: Option<String>,
    pub cache_write: Option<String>,
    pub reasoning: Option<String>,
    #[serde(default)]
    pub input_includes_cache: bool,
    #[serde(default = "yes")]
    pub output_includes_reasoning: bool,
}

fn yes() -> bool {
    true
}

pub struct Declared {
    client: String,
    root: PathBuf,
    extension: String,
    spec: Rc<Spec>,
    definition: Definition,
}

impl Declared {
    pub fn new(
        client: String,
        root: PathBuf,
        extension: String,
        spec: Spec,
        definition: Definition,
    ) -> Self {
        Self {
            client,
            root,
            extension,
            spec: Rc::new(spec),
            definition,
        }
    }

    pub fn file(&self) -> &Path {
        &self.definition.file
    }

    fn reader(&self, file: &Path, state: State) -> LogFile {
        LogFile {
            spec: Rc::clone(&self.spec),
            file: file
                .strip_prefix(&self.root)
                .unwrap_or(file)
                .to_string_lossy()
                .into_owned(),
            state,
        }
    }
}

impl Adapter for Declared {
    fn client(&self) -> &str {
        &self.client
    }

    fn definition(&self) -> Option<&Definition> {
        Some(&self.definition)
    }

    fn location(&self) -> &Path {
        &self.root
    }

    fn discover(&self) -> walk::Walk {
        walk::files_with_extension(&self.root, &self.extension)
    }

    fn start(&self, file: &Path) -> Box<dyn Reader> {
        Box::new(self.reader(file, State::default()))
    }

    fn resume(&self, file: &Path, saved: &str) -> Option<Box<dyn Reader>> {
        serde_json::from_str::<State>(saved)
            .ok()
            .map(|state| Box::new(self.reader(file, state)) as Box<dyn Reader>)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    lines: u64,
    /// Only for `cumulative` usage.
    totals: Option<Counts>,
    /// How many increases of the totals came so far.
    increases: u64,
}

/// Token counts as the log records them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Counts {
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
    reasoning: Option<u64>,
}

impl Counts {
    /// When the sum of the token classes falls, the totals restarted, and
    /// all of `self` is new.
    fn since(self, earlier: Option<Counts>, tokens: &TokenFields) -> Counts {
        let Some(earlier) = earlier.filter(|earlier| self.sum(tokens) >= earlier.sum(tokens))
        else {
            return self;
        };
        Counts {
            input: self.input.saturating_sub(earlier.input),
            cache_read: self.cache_read.saturating_sub(earlier.cache_read),
            cache_write: self.cache_write.saturating_sub(earlier.cache_write),
            output: self.output.saturating_sub(earlier.output),
            reasoning: self
                .reasoning
                .map(|now| now.saturating_sub(earlier.reasoning.unwrap_or(0))),
        }
    }

    /// The sum of the token classes, so a count the log keeps apart, such
    /// as reasoning outside the output, takes part.
    fn sum(self, tokens: &TokenFields) -> u128 {
        let classes = self.classes(tokens);
        [
            classes.uncached_input,
            classes.cache_write,
            classes.cache_read,
            classes.output,
        ]
        .into_iter()
        .map(|class| u128::from(class.0))
        .sum()
    }

    fn classes(self, tokens: &TokenFields) -> TokenClasses {
        let uncached_input = if tokens.input_includes_cache {
            self.input
                .saturating_sub(self.cache_read)
                .saturating_sub(self.cache_write)
        } else {
            self.input
        };
        let output = if tokens.output_includes_reasoning {
            self.output
        } else {
            self.output.saturating_add(self.reasoning.unwrap_or(0))
        };
        TokenClasses {
            uncached_input: Tokens(uncached_input),
            cache_write: Tokens(self.cache_write),
            cache_read: Tokens(self.cache_read),
            output: Tokens(output),
            reasoning: self.reasoning.map(Tokens),
        }
    }
}

/// What a call line records besides its usage.
struct Facts {
    id: Option<String>,
    timestamp: Timestamp,
    session: String,
    cwd: Option<String>,
    provider: String,
    model: String,
}

struct LogFile {
    spec: Rc<Spec>,
    /// The file's path below the root.
    file: String,
    state: State,
}

impl Reader for LogFile {
    fn parse_line(&mut self, line: &[u8]) -> Line {
        self.state.lines += 1;
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return Line::Unreadable;
        };
        let call_line = self
            .spec
            .when
            .iter()
            .all(|(pointer, expected)| value.pointer(pointer) == Some(expected));
        if !call_line {
            return Line::Other;
        }
        let (Some(facts), Some(counts)) = (self.facts(&value), self.counts(&value)) else {
            return Line::Unreadable;
        };
        let usage = match self.spec.usage {
            Usage::PerCall => counts,
            Usage::Cumulative => {
                let increase = counts.since(self.state.totals, &self.spec.tokens);
                self.state.totals = Some(counts);
                increase
            }
        };
        let tokens = usage.classes(&self.spec.tokens);
        // No tokens: a request that failed or was aborted before the model
        // ran, or totals that did not increase.
        if [
            tokens.uncached_input,
            tokens.cache_write,
            tokens.cache_read,
            tokens.output,
        ]
        .iter()
        .all(|class| class.0 == 0)
        {
            return Line::Other;
        }
        let position = match self.spec.usage {
            Usage::PerCall => self.state.lines,
            Usage::Cumulative => {
                self.state.increases += 1;
                self.state.increases
            }
        };
        let id = facts
            .id
            .clone()
            .unwrap_or_else(|| self.generated_id(position, &facts, counts));
        Line::Call(Call {
            id,
            provider: facts.provider,
            model: facts.model,
            timestamp: facts.timestamp,
            session_id: facts.session,
            cwd: facts.cwd,
            role: self.spec.role,
            agent_id: None,
            client_version: None,
            tokens,
            completion: Completion::FINAL,
            compaction: false,
        })
    }

    fn save(&self) -> Option<String> {
        serde_json::to_string(&self.state).ok()
    }
}

impl LogFile {
    fn facts(&self, value: &Value) -> Option<Facts> {
        let spec = &self.spec;
        let id = match &spec.id {
            Some(pointer) => Some(identifier(value.pointer(pointer)?)?),
            None => None,
        };
        let timestamp = value.pointer(&spec.timestamp)?.as_str()?.parse().ok()?;
        let session = match &spec.session {
            Some(pointer) => identifier(value.pointer(pointer)?)?,
            None => self.file.clone(),
        };
        let cwd = match spec
            .cwd
            .as_deref()
            .and_then(|pointer| value.pointer(pointer))
        {
            None | Some(Value::Null) => None,
            Some(cwd) => Some(cwd.as_str()?.to_owned()),
        };
        Some(Facts {
            id,
            timestamp,
            session,
            cwd,
            provider: text(&spec.provider, value)?,
            model: text(&spec.model, value)?,
        })
    }

    /// A count that is negative or fractional makes the line unreadable.
    fn counts(&self, value: &Value) -> Option<Counts> {
        let tokens = &self.spec.tokens;
        let optional = |pointer: &Option<String>| -> Option<Option<u64>> {
            match pointer
                .as_deref()
                .and_then(|pointer| value.pointer(pointer))
            {
                None | Some(Value::Null) => Some(None),
                Some(count) => count.as_u64().map(Some),
            }
        };
        Some(Counts {
            input: value.pointer(&tokens.input)?.as_u64()?,
            cache_read: optional(&tokens.cache_read)?.unwrap_or(0),
            cache_write: optional(&tokens.cache_write)?.unwrap_or(0),
            output: value.pointer(&tokens.output)?.as_u64()?,
            reasoning: optional(&tokens.reasoning)?,
        })
    }

    /// Every fact the line records goes into the ID, so a different call at
    /// the same position of a file that was replaced gets a new ID, while
    /// the same line read again keeps its ID. A JSON array keeps the facts
    /// apart whatever characters they contain.
    fn generated_id(&self, position: u64, facts: &Facts, counts: Counts) -> String {
        serde_json::json!([
            self.file,
            position,
            facts.timestamp,
            facts.session,
            facts.cwd,
            facts.provider,
            facts.model,
            counts.input,
            counts.cache_read,
            counts.cache_write,
            counts.output,
            counts.reasoning,
        ])
        .to_string()
    }
}

fn text(text: &Text, value: &Value) -> Option<String> {
    match text {
        Text::Field(pointer) => Some(value.pointer(pointer)?.as_str()?.to_owned()),
        Text::Value(constant) => Some(constant.clone()),
    }
}

/// IDs are strings or whole numbers in logs.
fn identifier(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) if number.is_u64() || number.is_i64() => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
