//! User adapters: one TOML file each in the `adapters` directory of the
//! configuration directory, read by the released binary without a rebuild.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use crate::adapters::user::declared::{Declared, Spec, Text, TokenFields, Usage};
use crate::adapters::user::external::External;
use crate::adapters::{Adapter, Definition, Role};

pub(crate) mod declared;
pub(crate) mod external;

#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot list the adapter files in {}: {error}; check that the directory is readable", dir.display())]
    List { dir: PathBuf, error: io::Error },
    #[error("cannot read the adapter file {}: {error}; check that the file is readable", file.display())]
    Read { file: PathBuf, error: io::Error },
    #[error("adapter file {}: {problem}; fix or remove the file", file.display())]
    Invalid { file: PathBuf, problem: String },
}

pub enum UserAdapter {
    Declared(Declared),
    External(External),
}

impl UserAdapter {
    pub fn client(&self) -> &str {
        match self {
            UserAdapter::Declared(adapter) => adapter.client(),
            UserAdapter::External(adapter) => adapter.client(),
        }
    }

    pub fn file(&self) -> &Path {
        match self {
            UserAdapter::Declared(adapter) => adapter.file(),
            UserAdapter::External(adapter) => adapter.file(),
        }
    }
}

/// A missing directory declares none; other files, such as a command's
/// script, are left alone.
pub fn load_dir(dir: &Path, shipped: &[&str]) -> Result<Vec<UserAdapter>, Error> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(Error::List {
                dir: dir.to_path_buf(),
                error,
            });
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| Error::List {
            dir: dir.to_path_buf(),
            error,
        })?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
            && path.is_file()
        {
            files.push(path);
        }
    }
    files.sort();
    let mut adapters: Vec<UserAdapter> = Vec::new();
    for file in files {
        let adapter = load(&file, shipped)?;
        if let Some(other) = adapters
            .iter()
            .find(|other| other.client() == adapter.client())
        {
            return Err(invalid(
                &file,
                format!(
                    "client {} is also declared by {}",
                    adapter.client(),
                    other.file().display()
                ),
            ));
        }
        adapters.push(adapter);
    }
    Ok(adapters)
}

pub fn load(file: &Path, shipped: &[&str]) -> Result<UserAdapter, Error> {
    let text = fs::read_to_string(file).map_err(|error| Error::Read {
        file: file.to_path_buf(),
        error,
    })?;
    parse(file, &text, shipped).map_err(|problem| invalid(file, problem))
}

fn invalid(file: &Path, problem: String) -> Error {
    Error::Invalid {
        file: file.to_path_buf(),
        problem,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterFile {
    client: String,
    command: Option<Vec<String>>,
    usage: Option<Usage>,
    files: Option<Files>,
    when: Option<BTreeMap<String, Value>>,
    fields: Option<Fields>,
    values: Option<Values>,
    tokens: Option<TokenFields>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Files {
    root: String,
    #[serde(default = "jsonl")]
    extension: String,
}

fn jsonl() -> String {
    "jsonl".to_owned()
}

/// JSON Pointers to the fields of a call line.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    id: Option<String>,
    timestamp: Option<String>,
    session: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    provider: Option<String>,
}

/// Values that are the same for every call.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Values {
    provider: Option<String>,
    model: Option<String>,
    role: Option<Role>,
}

fn parse(file: &Path, text: &str, shipped: &[&str]) -> Result<UserAdapter, String> {
    let parsed: AdapterFile = toml::from_str(text).map_err(|error| toml_problem(text, &error))?;
    let generic: Value = toml::from_str(text).map_err(|error| toml_problem(text, &error))?;
    let definition = Definition {
        file: file.to_path_buf(),
        text: canonical(&generic),
    };
    let client = parsed.client.clone();
    if client.trim().is_empty() {
        return Err("client is empty; name the client the calls are stored under".to_owned());
    }
    if shipped.contains(&client.as_str()) {
        return Err(format!(
            "client {client} is a shipped adapter's; choose another name"
        ));
    }
    match (parsed.command, parsed.files) {
        (Some(command), None) => {
            if parsed.usage.is_some()
                || parsed.when.is_some()
                || parsed.fields.is_some()
                || parsed.values.is_some()
                || parsed.tokens.is_some()
            {
                return Err(
                    "a command adapter takes no usage, [when], [fields], [values] or [tokens]"
                        .to_owned(),
                );
            }
            let mut command = command.into_iter();
            let Some(program) = command.next().filter(|program| !program.is_empty()) else {
                return Err("command is empty; name the program and its arguments".to_owned());
            };
            Ok(UserAdapter::External(External::new(
                client,
                program,
                command.collect(),
                definition,
            )))
        }
        (None, Some(files)) => {
            let spec = spec(
                parsed.usage.unwrap_or_default(),
                parsed.when.unwrap_or_default(),
                parsed.fields.unwrap_or_default(),
                parsed.values.unwrap_or_default(),
                parsed.tokens,
            )?;
            if files.extension.is_empty() {
                return Err("[files] extension is empty; name the log files' extension".to_owned());
            }
            Ok(UserAdapter::Declared(Declared::new(
                client,
                root(&files.root)?,
                files.extension,
                spec,
                definition,
            )))
        }
        (Some(_), Some(_)) => {
            Err("both command and [files] are set; keep [files] to read logs, or command to run a program".to_owned())
        }
        (None, None) => Err(
            "neither command nor [files] is set; set [files] to read logs, or command to run a program"
                .to_owned(),
        ),
    }
}

fn spec(
    usage: Usage,
    when: BTreeMap<String, Value>,
    fields: Fields,
    values: Values,
    tokens: Option<TokenFields>,
) -> Result<Spec, String> {
    let Some(tokens) = tokens else {
        return Err("[tokens] is missing; point to at least input and output".to_owned());
    };
    for pointer in when.keys() {
        pointer_in("[when]", pointer)?;
    }
    let field_pointers = [
        &fields.id,
        &fields.timestamp,
        &fields.session,
        &fields.cwd,
        &fields.model,
        &fields.provider,
    ];
    for pointer in field_pointers.into_iter().flatten() {
        pointer_in("[fields]", pointer)?;
    }
    let token_pointers = [
        Some(&tokens.input),
        Some(&tokens.output),
        tokens.cache_read.as_ref(),
        tokens.cache_write.as_ref(),
        tokens.reasoning.as_ref(),
    ];
    for pointer in token_pointers.into_iter().flatten() {
        pointer_in("[tokens]", pointer)?;
    }
    let Some(timestamp) = fields.timestamp else {
        return Err("[fields] timestamp is missing; point to each call's RFC 3339 time".to_owned());
    };
    Ok(Spec {
        when,
        id: fields.id,
        timestamp,
        session: fields.session,
        cwd: fields.cwd,
        provider: text("provider", fields.provider, values.provider)?,
        model: text("model", fields.model, values.model)?,
        role: values.role.unwrap_or_default(),
        tokens,
        usage,
    })
}

fn pointer_in(table: &str, pointer: &str) -> Result<(), String> {
    if pointer.is_empty() || pointer.starts_with('/') {
        Ok(())
    } else {
        Err(format!(
            "{table} has {pointer:?}, which is not a JSON Pointer; write it as \"/{pointer}\", with / before each step"
        ))
    }
}

fn text(name: &str, field: Option<String>, value: Option<String>) -> Result<Text, String> {
    match (field, value) {
        (Some(pointer), None) => Ok(Text::Field(pointer)),
        (None, Some(constant)) => Ok(Text::Value(constant)),
        (Some(_), Some(_)) => Err(format!(
            "{name} is set in both [fields] and [values]; keep one"
        )),
        (None, None) => Err(format!(
            "{name} is missing; point to it in [fields], or set it in [values]"
        )),
    }
}

/// A root starting with `~/` is in the home directory, so one file works
/// for every user; other relative roots would depend on the directory
/// tokendraw runs in.
fn root(root: &str) -> Result<PathBuf, String> {
    let home = || {
        env::home_dir().ok_or_else(|| {
            "[files] root starts with ~ but no home directory is known; set HOME".to_owned()
        })
    };
    let path = if root == "~" {
        home()?
    } else if let Some(rest) = root.strip_prefix("~/") {
        home()?.join(rest)
    } else {
        PathBuf::from(root)
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!(
            "[files] root {root:?} is relative; make it absolute or start it with ~/"
        ))
    }
}

fn toml_problem(text: &str, error: &toml::de::Error) -> String {
    let message = error.message().trim_end_matches('.').to_owned();
    match error.span() {
        Some(span) => {
            let line = text
                .get(..span.start)
                .map_or(1, |before| before.matches('\n').count() + 1);
            format!("line {line}: {message}")
        }
        None => message,
    }
}

/// JSON with object keys in sorted order and no whitespace, so equal
/// declarations have equal text whatever their order, comments or
/// formatting in the file.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            let entries: Vec<String> = entries
                .into_iter()
                .map(|(key, value)| format!("{}:{}", Value::from(key.as_str()), canonical(value)))
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
