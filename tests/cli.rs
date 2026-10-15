use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;

use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn tokendraw(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tokendraw"))
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("cannot run tokendraw: {error}"))
}

/// The directory `/work`, which the synthetic logs record working
/// directories in, made absolute on the system running the test: on
/// Windows a path without a drive is not absolute.
fn work_root() -> PathBuf {
    std::path::absolute("/work")
        .unwrap_or_else(|error| panic!("cannot make /work absolute: {error}"))
}

fn work(name: &str) -> String {
    work_root().join(name).to_string_lossy().into_owned()
}

fn fixtures() -> PathBuf {
    static COPY: OnceLock<PathBuf> = OnceLock::new();
    COPY.get_or_init(|| {
        prepare_fixtures().unwrap_or_else(|error| panic!("cannot copy the fixtures: {error}"))
    })
    .clone()
}

/// The synthetic logs of `tests/fixtures` with every JSON string that
/// starts `/work/` starting with [`work_root`] instead, in a directory of
/// `CARGO_TARGET_TMPDIR` named after their content, which later test runs
/// reuse.
fn prepare_fixtures() -> std::io::Result<PathBuf> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let root = serde_json::to_string(&work_root().to_string_lossy())?;
    let replacement = format!("{}/", root.trim_end_matches('"'));
    let mut files = Vec::new();
    read_logs(&source, Path::new(""), &mut files)?;
    for (_, text) in &mut files {
        *text = text.replace("\"/work/", &replacement);
    }
    let mut hasher = DefaultHasher::new();
    files.hash(&mut hasher);
    let target =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fixtures-{:016x}", hasher.finish()));
    if target.exists() {
        return Ok(target);
    }
    let staging = TempDir::new_in(env!("CARGO_TARGET_TMPDIR"))?;
    for (relative, text) in &files {
        let path = staging.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
    }
    // Another test process may have put the same copy in place first.
    match std::fs::rename(staging.path(), &target) {
        Err(_) if target.exists() => Ok(target),
        result => result.map(|()| target),
    }
}

/// Every file under `dir` with its path relative to the fixtures and its
/// text, in path order.
fn read_logs(
    dir: &Path,
    relative: &Path,
    files: &mut Vec<(PathBuf, String)>,
) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = relative.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            read_logs(&entry.path(), &path, files)?;
        } else {
            files.push((path, std::fs::read_to_string(entry.path())?));
        }
    }
    Ok(())
}

/// A command that reads the synthetic Claude Code, Codex and pi logs, keeps
/// its ledger in `state` and reads user adapters from [`adapters_dir`] of
/// `state`, whichever platform the test runs on.
fn tokendraw_with_state(state: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tokendraw"));
    command
        .args(args)
        .env("CLAUDE_CONFIG_DIR", fixtures().join("claude-code"))
        .env("CODEX_HOME", fixtures().join("codex"))
        .env("PI_CODING_AGENT_DIR", fixtures().join("pi"))
        .env_remove("PI_CODING_AGENT_SESSION_DIR")
        .env("HOME", state)
        .env("USERPROFILE", state)
        .env("XDG_STATE_HOME", state)
        .env("LOCALAPPDATA", state)
        .env("XDG_CONFIG_HOME", state)
        .env("XDG_DATA_HOME", state)
        .env("APPDATA", state)
        .env_remove("TOKENDRAW_CONFIG");
    command
}

fn run(state: &Path, args: &[&str]) -> Result<Output, std::io::Error> {
    tokendraw_with_state(state, args).output()
}

/// Standard output with the fixtures directory, as the tool shows a
/// location, written `<fixtures>`, so snapshots hold on every machine.
fn stdout(output: &Output) -> String {
    let fixtures = fixtures().to_string_lossy().replace('\\', "/");
    without_tomorrow(&String::from_utf8_lossy(&output.stdout).replace(&fixtures, "<fixtures>"))
}

#[test]
fn version_flag_prints_the_name_and_version() {
    let output = tokendraw(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("tokendraw {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_flag_is_a_usage_error_with_exit_code_2() {
    let output = tokendraw(&["--no-such-flag"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert!(String::from_utf8_lossy(&output.stderr).contains("--no-such-flag"));
}

#[test]
fn tokens_prints_every_call_per_client_with_what_was_read() -> TestResult {
    let state = TempDir::new()?;
    let output = run(state.path(), &["tokens"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(stdout(&output));
    Ok(())
}

#[test]
fn tokens_json_counts_the_calls_in_the_range() -> TestResult {
    let state = TempDir::new()?;
    let output = run(
        state.path(),
        &[
            "tokens",
            "--since",
            "2026-09-01",
            "--until",
            "2026-10-01",
            "--json",
        ],
    )?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(stdout(&output));
    Ok(())
}

#[test]
fn a_second_run_reads_nothing_new_and_counts_the_same() -> TestResult {
    let state = TempDir::new()?;
    let first = run(state.path(), &["tokens", "--json"])?;
    let second = run(state.path(), &["tokens", "--json"])?;
    assert_eq!(stdout(&first), stdout(&second));
    Ok(())
}

#[test]
fn two_processes_ingesting_at_once_count_every_call_once() -> TestResult {
    let state = TempDir::new()?;
    let spawn = || -> Result<Child, std::io::Error> {
        tokendraw_with_state(state.path(), &["tokens", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    };
    let children = [spawn()?, spawn()?];
    for child in children {
        let output = child.wait_with_output()?;
        assert_eq!(output.status.code(), Some(0), "{output:?}");
    }
    let alone = TempDir::new()?;
    assert_eq!(
        stdout(&run(state.path(), &["tokens", "--json"])?),
        stdout(&run(alone.path(), &["tokens", "--json"])?)
    );
    Ok(())
}

#[test]
fn a_date_that_cannot_be_read_is_a_usage_error() -> TestResult {
    let state = TempDir::new()?;
    let output = run(state.path(), &["tokens", "--since", "September"])?;
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "");
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected a date such as 2026-09-01"));
    Ok(())
}

#[test]
fn a_ledger_that_cannot_be_created_fails_with_exit_code_1() -> TestResult {
    let state = TempDir::new()?;
    let blocker = state.path().join("not-a-directory");
    std::fs::write(&blocker, "")?;
    let output = run(&blocker, &["tokens"])?;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("tokendraw: cannot create the directory of the ledger"),
        "{stderr}"
    );
    assert!(
        stderr
            .trim_end()
            .ends_with("check that its parent directory is writable"),
        "{stderr}"
    );
    Ok(())
}

fn adapters_dir(state: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        state.join("Library/Application Support/tokendraw/adapters")
    } else {
        state.join("tokendraw/adapters")
    }
}

/// A TOML literal string, so Windows paths need no escapes.
fn toml_path(path: &Path) -> String {
    format!("'{}'", path.display())
}

/// An adapter file for the synthetic example agent's logs, whose output
/// tokens are at `output`.
fn agent_adapter(output: &str) -> String {
    format!(
        r#"# The example agent's logs: one line per response, streamed responses
# logged again under the same ID.
client = "example-agent"

[files]
root = {root}

[when]
"/type" = "response"

[fields]
id = "/id"
timestamp = "/time"
session = "/session"
cwd = "/cwd"
model = "/model"

[values]
provider = "example-cloud"

[tokens]
input = "/usage/prompt_tokens"
input_includes_cache = true
cache_read = "/usage/cached_tokens"
output = "{output}"
reasoning = "/usage/reasoning_tokens"
"#,
        root = toml_path(&fixtures().join("example-agent/logs")),
    )
}

fn command_adapter() -> String {
    let command = if cfg!(windows) {
        r#"["cmd", "/C", "type", "records.jsonl"]"#
    } else {
        r#"["cat", "records.jsonl"]"#
    };
    format!("client = \"example-command\"\ncommand = {command}\n")
}

fn install_adapters(state: &Path) -> Result<PathBuf, std::io::Error> {
    let dir = adapters_dir(state);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("example-agent.toml"),
        agent_adapter("/usage/completion_tokens"),
    )?;
    std::fs::write(dir.join("example-command.toml"), command_adapter())?;
    std::fs::copy(
        fixtures().join("example-command/records.jsonl"),
        dir.join("records.jsonl"),
    )?;
    Ok(dir)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn user_adapters_of_each_kind_are_read_by_the_built_binary() -> TestResult {
    let state = TempDir::new()?;
    install_adapters(state.path())?;
    let output = run(state.path(), &["tokens", "--json"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(stdout(&output));
    let again = run(state.path(), &["tokens", "--json"])?;
    assert_eq!(stdout(&again), stdout(&output));
    Ok(())
}

#[test]
fn adapter_check_counts_an_adapter_without_writing_the_ledger() -> TestResult {
    let state = TempDir::new()?;
    let file = state.path().join("example-agent.toml");
    std::fs::write(&file, agent_adapter("/usage/completion_tokens"))?;
    let file = file.to_string_lossy().into_owned();
    let output = run(state.path(), &["adapter", "check", &file])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(stdout(&output));
    let tokens = run(state.path(), &["tokens", "--json"])?;
    assert!(!stdout(&tokens).contains("example-agent"), "{tokens:?}");
    Ok(())
}

#[test]
fn an_edited_adapter_file_is_refused_until_it_is_reset() -> TestResult {
    let state = TempDir::new()?;
    let dir = install_adapters(state.path())?;
    let file = dir.join("example-agent.toml");
    assert_eq!(run(state.path(), &["tokens"])?.status.code(), Some(0));
    std::fs::write(&file, agent_adapter("/usage/reasoning_tokens"))?;
    let refused = run(state.path(), &["tokens"])?;
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(stdout(&refused), "");
    let message = stderr(&refused);
    assert!(
        message.contains("changed since the calls of example-agent"),
        "{message}"
    );
    assert!(
        message.contains("tokendraw adapter reset example-agent"),
        "{message}"
    );
    let reset = run(state.path(), &["adapter", "reset", "example-agent"])?;
    assert_eq!(reset.status.code(), Some(0), "{reset:?}");
    assert_eq!(
        stdout(&reset),
        "forgot 3 calls and 2 files of example-agent; the next `tokendraw tokens` reads its logs again\n"
    );
    let reread = run(state.path(), &["tokens", "--json"])?;
    assert_eq!(reread.status.code(), Some(0), "{reread:?}");
    let report: serde_json::Value = serde_json::from_slice(&reread.stdout)?;
    let agent = report["clients"].as_array().and_then(|clients| {
        clients
            .iter()
            .find(|client| client["client"] == "example-agent")
    });
    assert_eq!(
        agent.map(|agent| &agent["tokens"]["output"]),
        Some(&serde_json::json!(20))
    );
    Ok(())
}

#[test]
fn an_adapter_command_that_cannot_run_fails_naming_its_file() -> TestResult {
    let state = TempDir::new()?;
    let dir = adapters_dir(state.path());
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("broken.toml"),
        "client = \"broken\"\ncommand = [\"tokendraw-test-no-such-program\"]\n",
    )?;
    let output = run(state.path(), &["tokens"])?;
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(
        message.contains("broken.toml: cannot run tokendraw-test-no-such-program"),
        "{message}"
    );
    Ok(())
}

#[test]
fn an_adapter_command_that_fails_keeps_nothing_it_printed() -> TestResult {
    let state = TempDir::new()?;
    let dir = install_adapters(state.path())?;
    let command = if cfg!(windows) {
        r#"["cmd", "/C", "type records.jsonl & exit 3"]"#
    } else {
        r#"["sh", "-c", "cat records.jsonl; exit 3"]"#
    };
    std::fs::write(
        dir.join("example-command.toml"),
        format!("client = \"example-command\"\ncommand = {command}\n"),
    )?;
    let output = run(state.path(), &["tokens"])?;
    assert_eq!(output.status.code(), Some(1));
    let message = stderr(&output);
    assert!(
        message.contains("example-command.toml: the command failed"),
        "{message}"
    );
    std::fs::write(dir.join("example-command.toml"), command_adapter())?;
    let report = stdout(&run(state.path(), &["tokens", "--json"])?);
    assert!(report.contains("example-command"), "{report}");
    Ok(())
}

#[test]
fn an_invalid_adapter_file_fails_naming_the_file_and_the_problem() -> TestResult {
    let state = TempDir::new()?;
    let dir = adapters_dir(state.path());
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("typo.toml"),
        "client = \"typo\"\ncomand = [\"tool\"]\n",
    )?;
    let output = run(state.path(), &["tokens"])?;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    let message = stderr(&output);
    assert!(
        message.contains("typo.toml: line 2: unknown field `comand`"),
        "{message}"
    );
    assert!(
        message.trim_end().ends_with("fix or remove the file"),
        "{message}"
    );
    Ok(())
}

#[test]
fn a_shipped_client_cannot_be_reset() -> TestResult {
    let state = TempDir::new()?;
    let output = run(state.path(), &["adapter", "reset", "codex"])?;
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("codex is a shipped adapter's client"),
        "{output:?}"
    );
    Ok(())
}

fn data_dir(state: &Path) -> PathBuf {
    adapters_dir(state).with_file_name("data")
}

/// The output without the stale section, which depends on today's date.
fn without_stale(text: &str) -> String {
    let mut kept = Vec::new();
    let mut in_stale = false;
    for line in text.lines() {
        if in_stale && line.starts_with("  ") {
            continue;
        }
        in_stale = line.starts_with("stale:");
        kept.push(if in_stale {
            "stale: <depends on the date>"
        } else {
            line
        });
    }
    kept.join("\n")
}

#[test]
fn data_prints_the_version_files_and_records() -> TestResult {
    let state = TempDir::new()?;
    let output = run(state.path(), &["data"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(without_stale(&stdout(&output)));
    Ok(())
}

#[test]
fn data_json_holds_every_record_and_source() -> TestResult {
    let state = TempDir::new()?;
    let output = run(state.path(), &["data", "--json"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["version"]["files"].as_array().map(Vec::len), Some(10));
    assert_eq!(json["findings"]["long-context"]["cached_tokens"], 200_000);
    assert!(json["prices"]["claude-opus-5-5"]["input_usd_per_mtok"].is_number());
    assert!(json["price_calibrations"]["open-weight-hosts"]["wh_per_usd"]["base_case"].is_number());
    assert!(
        json["priced_sets"]["frontier@claude-haiku-5-5>100000"]["b_in_j_per_token"]["base_case"]
            .is_number()
    );
    assert_eq!(json["price_review_interval_days"], 30);
    assert_eq!(json["version"]["overrides"], serde_json::json!([]));
    let geographies = json["geographies"]
        .as_object()
        .map_or(0, serde_json::Map::len);
    assert!(geographies > 200, "{geographies} geographies");
    assert_eq!(json["geographies"]["ember:FIN"]["basis"], "lifecycle");
    assert_eq!(json["providers"]["unknown"]["sites"], "named");
    assert!(json["providers"]["unknown"]["grid"]["base_case"].is_number());
    assert!(json["sources"]["ember-yearly"]["sha256"].is_string());
    assert!(json["stale"].is_array());
    Ok(())
}

#[test]
fn an_override_file_replaces_a_record_and_is_named_in_the_version() -> TestResult {
    let state = TempDir::new()?;
    let dir = data_dir(state.path());
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("self-hosted.toml"),
        r#"[sources.my-meter]
title = "Our own meter readings"
publisher = "Example studio"
accessed = 2026-10-01

[[provider]]
id = "ollama"
sites = ["ember:FRA"]
label = "M"
sources = ["my-meter"]
basis = "Our workstation in Paris"
reviewed = 2026-10-01

[provider.pue]
lower_bound = 1
low_case = 1
base_case = 1
high_case = 1
upper_bound = 1
label = "M"
sources = ["my-meter"]
basis = "No data centre"
"#,
    )?;
    let output = run(state.path(), &["data"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("self-hosted.toml"), "{text}");
    assert!(
        text.contains("    adds source my-meter\n    adds provider ollama\n"),
        "{text}"
    );
    assert!(
        text.contains("  ollama         grid 41.2 / 41.2 / 41.2 / 41.2 / 41.2  M"),
        "{text}"
    );
    Ok(())
}

#[test]
fn an_invalid_override_file_fails_naming_the_file() -> TestResult {
    let state = TempDir::new()?;
    let dir = data_dir(state.path());
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("broken.toml"), "[[geography]]\nid = \"x\"\n")?;
    let output = run(state.path(), &["data"])?;
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = stderr(&output);
    assert!(stderr.starts_with("tokendraw: data file "), "{stderr}");
    assert!(stderr.contains("broken.toml"), "{stderr}");
    assert!(
        stderr.trim_end().ends_with("fix or remove the file"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn data_export_writes_the_shipped_files_byte_for_byte_and_never_overwrites() -> TestResult {
    let state = TempDir::new()?;
    let target = state.path().join("exported");
    let target_text = target.to_string_lossy().into_owned();
    let output = run(state.path(), &["data", "export", &target_text])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    for path in [
        "data.toml",
        "energy/parameter-sets.toml",
        "energy/models.toml",
        "providers.toml",
        "grid/ember.toml",
    ] {
        assert_eq!(
            std::fs::read(target.join(path))?,
            std::fs::read(shipped.join(path))?,
            "{path}"
        );
    }
    let again = run(state.path(), &["data", "export", &target_text])?;
    assert_eq!(again.status.code(), Some(1), "{again:?}");
    assert!(stderr(&again).contains("choose an empty directory"));
    Ok(())
}

#[test]
fn report_prints_the_l1_estimate_of_every_client() -> TestResult {
    let state = TempDir::new()?;
    let output = run(state.path(), &["report"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(stdout(&output));
    Ok(())
}

#[test]
fn report_verbose_adds_the_figures_the_emissions_are_computed_from() -> TestResult {
    let state = TempDir::new()?;
    let output = run(state.path(), &["report", "--verbose"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(stdout(&output));
    Ok(())
}

#[test]
fn report_json_holds_the_estimates_of_the_calls_in_the_range() -> TestResult {
    let state = TempDir::new()?;
    let output = run(
        state.path(),
        &[
            "report",
            "--json",
            "--since",
            "2026-09-30",
            "--until",
            "2026-10-01",
        ],
    )?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(without_platform(&stdout(&output))?);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let splits: Vec<f64> = json["breakdown"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["shares_of_base_case"].as_object())
        .map(|shares| shares.values().filter_map(serde_json::Value::as_f64).sum())
        .collect();
    assert_eq!(splits.len(), 2, "{json}");
    for sum in splits {
        assert!((sum - 1.0).abs() < 1e-9, "{sum}");
    }
    Ok(())
}

/// The report's JSON with the version and platform of tokendraw, which
/// differ between builds and machines, as placeholders.
fn without_platform(json: &str) -> TestResult<String> {
    let block = |version: &str, os: &str, arch: &str| {
        format!(
            "\"tokendraw\": {{\n    \"version\": \"{version}\",\n    \"os\": \"{os}\",\n    \"arch\": \"{arch}\"\n  }}"
        )
    };
    let actual = block(
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    if !json.contains(&actual) {
        return Err(format!("no {actual} in {json}").into());
    }
    Ok(json.replace(&actual, &block("<version>", "<os>", "<arch>")))
}

#[test]
fn report_json_lists_only_what_was_estimated() -> TestResult {
    let state = TempDir::new()?;
    let output = run(state.path(), &["report", "--json"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let quantities: Vec<&str> = json["estimates"]
        .as_array()
        .map(|estimates| {
            estimates
                .iter()
                .filter_map(|estimate| estimate["quantity"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(quantities, ["energy", "energy_facility", "emissions"]);
    for estimate in json["estimates"].as_array().into_iter().flatten() {
        assert_eq!(estimate["layer"], "L1");
        assert!(estimate.get("reason").is_none(), "{estimate}");
        assert!(
            five(estimate).iter().all(|value| value.is_finite()),
            "{estimate}"
        );
    }
    assert_eq!(json["sums"], serde_json::json!([]));
    assert!(json.get("explanation").is_none(), "{json}");
    Ok(())
}

#[test]
fn a_range_without_calls_reports_zero_with_no_factor() -> TestResult {
    let state = TempDir::new()?;
    let range = ["--since", "2020-01-01", "--until", "2020-01-02"];
    let output = run(state.path(), &[&["report", "--json"][..], &range].concat())?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let estimates = json["estimates"].as_array().cloned().unwrap_or_default();
    assert_eq!(estimates.len(), 3, "{json}");
    for estimate in &estimates {
        assert_five(&five(estimate), [0.0; 5]);
        assert_eq!(estimate["factor_quality"], serde_json::Value::Null);
    }
    assert_eq!(json["sums"], serde_json::json!([]));
    assert_eq!(json["l1_groups"], serde_json::json!([]));
    let text = run(state.path(), &[&["report"][..], &range].concat())?;
    assert_eq!(text.status.code(), Some(0), "{text:?}");
    let text = stdout(&text);
    let emissions: Vec<&str> = text
        .lines()
        .skip_while(|line| !line.starts_with("Emissions"))
        .collect();
    assert!(emissions[0].starts_with("Emissions, g CO2e"), "{text}");
    assert_eq!(emissions[1], "", "{text}");
    assert!(emissions[2].starts_with("  total"), "{text}");
    Ok(())
}

/// Synthetic calls with exact token counts, the last outside the range of
/// [`report_computes_hand_worked_values_with_overridden_data`].
const ACCEPTANCE_RECORDS: &str = r#"{"id":"a1","timestamp":"2026-08-01T10:00:00Z","provider":"anthropic","model":"claude-test","session_id":"s1","tokens":{"uncached_input":1000,"cache_read":100000,"output":500}}
{"id":"o1","timestamp":"2026-08-01T11:00:00Z","provider":"openai","model":"gpt-test","session_id":"s1","tokens":{"uncached_input":2000,"output":1000}}
{"id":"x1","timestamp":"2026-08-01T12:00:00Z","provider":"nowhere","model":"mystery","session_id":"s2","tokens":{"uncached_input":100,"output":10}}
{"id":"late","timestamp":"2026-08-02T00:00:00Z","provider":"anthropic","model":"claude-test","session_id":"s3","tokens":{"uncached_input":5000,"output":5000}}
"#;

/// Data overrides with simple values: every parameter's five values are a
/// base value times 0.5 / 1 / 1 / 2 / 4, and `idle` is 1 / 1 / 1.5 / 1.5 /
/// 2, so IT energy is node energy at the base values times 0.5 / 1 / 1.5 /
/// 3 / 8. `us_in_label` is the label of the Indiana geography.
fn acceptance_overrides(us_in_label: &str) -> String {
    let factor = |name: &str, base: f64| {
        format!(
            "[parameter_set.{name}]\nlower_bound = {}\nlow_case = {base}\nbase_case = {base}\nhigh_case = {}\nupper_bound = {}\nlabel = \"M\"\nsources = [\"test-meter\"]\nbasis = \"test\"\n",
            base * 0.5,
            base * 2.0,
            base * 4.0
        )
    };
    let pue = |values: &str| {
        let values: Vec<&str> = values.split(' ').collect();
        format!(
            "[provider.pue]\nlower_bound = {}\nlow_case = {}\nbase_case = {}\nhigh_case = {}\nupper_bound = {}\nlabel = \"M\"\nsources = [\"test-meter\"]\nbasis = \"test\"\n",
            values[0], values[1], values[2], values[3], values[4]
        )
    };
    let provider = |id: &str, sites: &str, values: &str| {
        format!(
            "[[provider]]\nid = \"{id}\"\nsites = {sites}\nlabel = \"M\"\nsources = [\"test-meter\"]\nbasis = \"test\"\nreviewed = 2026-10-01\n{}",
            pue(values)
        )
    };
    let geography = |id: &str, value: u32, year: u32, label: &str| {
        format!(
            "[[geography]]\nid = \"{id}\"\nname = \"{id}\"\nyear = {year}\ng_co2e_per_kwh = {value}\nbasis = \"metered\"\nlabel = \"{label}\"\nsources = [\"test-meter\"]\nreviewed = 2026-10-01\n"
        )
    };
    let mut text = "[sources.test-meter]\ntitle = \"Test meter readings\"\npublisher = \"Example studio\"\naccessed = 2026-10-01\n\n[[parameter_set]]\nid = \"frontier\"\ndescription = \"test\"\nreviewed = 2026-10-01\n".to_owned();
    for (name, base) in [
        ("a_p_uj_per_pair", 2.0),
        ("a_d_uj_per_pair", 20.0),
        ("b_in_j_per_token", 0.2),
        ("b_out_j_per_token", 1.0),
        ("c0_mj_per_cached_token", 0.2),
    ] {
        text.push_str(&factor(name, base));
    }
    text.push_str("[parameter_set.idle]\nlower_bound = 1\nlow_case = 1\nbase_case = 1.5\nhigh_case = 1.5\nupper_bound = 2\nlabel = \"M\"\nsources = [\"test-meter\"]\nbasis = \"test\"\n");
    text.push_str(&provider(
        "anthropic",
        r#"["ember:US-IN", "ember:US-IA"]"#,
        "1.1 1.1 1.1 1.2 1.5",
    ));
    text.push_str(&provider(
        "openai",
        r#"["ember:US-IA"]"#,
        "1.2 1.2 1.2 1.2 1.2",
    ));
    text.push_str(&provider(
        "unknown",
        r#"["ember:US-IA"]"#,
        "1.5 1.5 1.5 1.5 1.5",
    ));
    text.push_str(&geography("ember:US-IN", 600, 2030, us_in_label));
    text.push_str(&geography("ember:US-IA", 300, 2024, "M"));
    text
}

/// The text with every run of whitespace as one space, so wrapped lines
/// read as one, ending in a space.
fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ") + " "
}

fn five(json: &serde_json::Value) -> Vec<f64> {
    [
        "lower_bound",
        "low_case",
        "base_case",
        "high_case",
        "upper_bound",
    ]
    .iter()
    .map(|field| json[*field].as_f64().unwrap_or(f64::NAN))
    .collect()
}

fn assert_five(actual: &[f64], expected: [f64; 5]) {
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "{actual:?} against {expected:?}"
        );
    }
}

// Node energy per call at the base values:
//
// - a1, Anthropic: L = 100,000, n_new = 1,000, n_out = 500. Prefill pairs
//   1,000 x (100,000 + 500) = 100,500,000 x 2 uJ = 201 J; decode pairs
//   500 x (100,000 + 1,000 + 250) = 50,625,000 x 20 uJ = 1,012.5 J; new
//   tokens 1,000 x 0.2 J = 200 J; output 500 x 1 J = 500 J; cached tokens
//   100,000 x 0.2 mJ = 20 J. Node 1,933.5 J.
// - o1, OpenAI: L = 0, n_new = 2,000, n_out = 1,000. 2,000 x 1,000 x 2 uJ =
//   4 J; 1,000 x (2,000 + 500) x 20 uJ = 50 J; 400 J; 1,000 J. Node 1,454 J.
// - x1, provider nowhere, model mystery, both unmatched, so the frontier
//   set and the unknown provider: n_new = 100, n_out = 10. 100 x 50 x 2 uJ
//   = 0.01 J; 10 x 105 x 20 uJ = 0.021 J; 20 J; 10 J. Node 30.031 J.
// - late is at the end of the range, which is exclusive.
//
// IT energy is node times 0.5 / 1 / 1.5 / 3 / 8, energy IT times PUE:
//
// - a1: IT 966.75 / 1,933.5 / 2,900.25 / 5,800.5 / 15,468 J; times 1.1 /
//   1.1 / 1.1 / 1.2 / 1.5 = 1,063.425 / 2,126.85 / 3,190.275 / 6,960.6 /
//   23,202 J; facility IT times 0.1 / 0.1 / 0.1 / 0.2 / 0.5 = 96.675 /
//   193.35 / 290.025 / 1,160.1 / 7,734 J.
// - o1: IT 727 / 1,454 / 2,181 / 4,362 / 11,632 J; times 1.2 = 872.4 /
//   1,744.8 / 2,617.2 / 5,234.4 / 13,958.4 J; facility times 0.2 = 145.4 /
//   290.8 / 436.2 / 872.4 / 2,326.4 J.
// - x1: IT 15.0155 / 30.031 / 45.0465 / 90.093 / 240.248 J; times 1.5 =
//   22.52325 / 45.0465 / 67.56975 / 135.1395 / 360.372 J; facility times
//   0.5 = 7.50775 / 15.0155 / 22.52325 / 45.0465 / 120.124 J.
//
// Energy in Wh is the sum over 3,600: lower bound (1,063.425 + 872.4 +
// 22.52325) / 3,600 = 0.543985625, and so on. Grid: Anthropic's sites are
// Indiana 600 and Iowa 300 g/kWh, so 300 / 300 / 450 / 600 / 600; OpenAI
// and unknown are Iowa alone, 300. Emissions in g are Wh / 1,000 times
// g/kWh: base case (3,190.275 x 450 + (2,617.2 + 67.56975) x 300) / 3.6 x
// 10^6 = 2,241,054.675 / 3.6 x 10^6 = 0.62251519.
#[test]
fn report_computes_hand_worked_values_with_overridden_data() -> TestResult {
    let (state, _) = acceptance_state()?;
    let output = run(
        state.path(),
        &[&["report", "--json"][..], &ACCEPTANCE_RANGE].concat(),
    )?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let estimates = &json["estimates"];
    assert_eq!(estimates[0]["quantity"], "energy");
    assert_five(
        &five(&estimates[0]),
        [
            0.543_985_625,
            1.087_971_25,
            1.631_956_875,
            3.425_038_75,
            37_520.772 / 3600.0,
        ],
    );
    assert_eq!(estimates[1]["quantity"], "energy_facility");
    assert_five(
        &five(&estimates[1]),
        [
            249.582_75 / 3600.0,
            499.165_5 / 3600.0,
            748.748_25 / 3600.0,
            2_077.546_5 / 3600.0,
            10_180.524 / 3600.0,
        ],
    );
    assert_eq!(estimates[2]["quantity"], "emissions");
    assert_five(
        &five(&estimates[2]),
        [
            1_958.348_25 * 300.0 / 3.6e6,
            3_916.696_5 * 300.0 / 3.6e6,
            2_241_054.675 / 3.6e6,
            (6_960.6 * 600.0 + 5_369.539_5 * 300.0) / 3.6e6,
            (23_202.0 * 600.0 + 14_318.772 * 300.0) / 3.6e6,
        ],
    );
    assert_eq!(json["observed_totals"]["calls"], 3);
    assert_eq!(
        json["unmatched"],
        serde_json::json!({
            "models": [{"name": "mystery", "calls": 1}],
            "providers": [{"name": "nowhere", "calls": 1}]
        })
    );
    let records = &json["data_version"]["overrides"][0]["records"];
    assert!(
        records.as_array().is_some_and(|records| records
            .iter()
            .any(|record| record["id"] == "ember:US-IN" && record["action"] == "replaced")),
        "{records}"
    );
    let basis = estimates[2]["basis"].as_str().unwrap_or_default();
    assert!(
        basis.contains("grid: Test meter readings, metered, data years 2024 to 2030"),
        "{basis}"
    );
    assert_eq!(estimates[0]["factor_quality"], "M");
    assert_eq!(estimates[2]["factor_quality"], "C");

    Ok(())
}

/// pi records Moonshot AI's API as `moonshotai` (api.moonshot.ai) and
/// `moonshotai-cn` (api.moonshot.cn); each matches its own provider record.
#[test]
fn pi_moonshot_calls_match_their_provider_records() -> TestResult {
    let state = TempDir::new()?;
    let logs = state.path().join("logs");
    let sessions = logs.join("pi/sessions/--work-alpha--");
    std::fs::create_dir_all(&sessions)?;
    let call = |id: &str, provider: &str| {
        serde_json::json!({
            "type": "message",
            "id": id,
            "parentId": null,
            "timestamp": "2026-09-30T09:00:05.000Z",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Synthetic reply."}],
                "api": "openai-completions",
                "provider": provider,
                "model": "kimi-k3",
                "usage": {"input": 1000, "output": 100, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 1100},
                "stopReason": "stop",
                "timestamp": 1_790_758_805_000_u64,
                "responseId": format!("resp-{id}"),
            },
        })
        .to_string()
    };
    let header = serde_json::json!({
        "type": "session",
        "version": 3,
        "id": "0b7e0000-0000-4000-8000-0000000000d1",
        "timestamp": "2026-09-30T09:00:00.000Z",
        "cwd": "/work/alpha",
    });
    std::fs::write(
        sessions.join("2026-09-30T09-00-00-000Z_0b7e0000-0000-4000-8000-0000000000d1.jsonl"),
        [
            header.to_string(),
            call("m1", "moonshotai"),
            call("m2", "moonshotai-cn"),
        ]
        .join("\n")
            + "\n",
    )?;
    let output = tokendraw_with_state(
        state.path(),
        &[
            "report",
            "--json",
            "--since",
            "2026-09-30",
            "--until",
            "2026-10-01",
        ],
    )
    .env("CLAUDE_CONFIG_DIR", logs.join("claude"))
    .env("CODEX_HOME", logs.join("codex"))
    .env("PI_CODING_AGENT_DIR", logs.join("pi"))
    .output()?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["observed_totals"]["calls"], 2);
    assert_eq!(
        json["unmatched"]["providers"],
        serde_json::json!([]),
        "{json}"
    );
    let groups: Vec<(&str, &str, f64)> = json["l1_groups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| {
                    (
                        group["provider"].as_str().unwrap_or_default(),
                        group["record"].as_str().unwrap_or_default(),
                        group["grid_g_per_kwh"]["base_case"]
                            .as_f64()
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        groups,
        [
            ("moonshotai", "moonshotai", 497.047),
            ("moonshotai-cn", "moonshotai-cn", 528.278)
        ]
    );
    Ok(())
}

fn acceptance_state() -> Result<(TempDir, PathBuf), std::io::Error> {
    let state = TempDir::new()?;
    let adapters = adapters_dir(state.path());
    std::fs::create_dir_all(&adapters)?;
    std::fs::write(adapters.join("example-command.toml"), command_adapter())?;
    std::fs::write(adapters.join("records.jsonl"), ACCEPTANCE_RECORDS)?;
    let data = data_dir(state.path());
    std::fs::create_dir_all(&data)?;
    std::fs::write(data.join("acceptance.toml"), acceptance_overrides("C"))?;
    Ok((state, data))
}

/// The range of [`ACCEPTANCE_RECORDS`] without the last.
const ACCEPTANCE_RANGE: [&str; 4] = ["--since", "2026-08-01", "--until", "2026-08-02"];

#[test]
fn report_text_names_the_data_and_its_quality() -> TestResult {
    let (state, data) = acceptance_state()?;
    let text = stdout(&run(
        state.path(),
        &[&["report"][..], &ACCEPTANCE_RANGE].concat(),
    )?);
    assert!(!text.contains("Electricity"), "{text}");
    assert!(!text.contains("Data sources"), "{text}");
    assert!(
        words(&text).contains("No data record for model mystery, provider nowhere; "),
        "{text}"
    );
    assert!(
        words(&text).contains("Data overrides in effect: "),
        "{text}"
    );
    let text = stdout(&run(
        state.path(),
        &[&["report", "--verbose"][..], &ACCEPTANCE_RANGE].concat(),
    )?);
    let prose = words(&text);
    assert!(
        prose.contains("grid intensity Test meter readings; metered; data years 2024 to 2030: disclosed by the provider, published by a company "),
        "{text}"
    );
    assert!(
        prose.contains("energy per call parameter set frontier: disclosed by the provider "),
        "{text}"
    );
    assert!(
        prose.contains("models without data mystery (1 call), so the frontier parameter set "),
        "{text}"
    );
    let verbose = stdout(&run(
        state.path(),
        &[&["report", "--verbose"][..], &ACCEPTANCE_RANGE].concat(),
    )?);
    assert!(verbose.contains("\n  nowhere (unknown) "), "{verbose}");
    assert!(verbose.contains("\nElectricity, Wh "), "{verbose}");

    std::fs::write(data.join("acceptance.toml"), acceptance_overrides("M"))?;
    let output = run(
        state.path(),
        &[&["report", "--json"][..], &ACCEPTANCE_RANGE].concat(),
    )?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["estimates"][2]["factor_quality"], "M");
    let text = stdout(&run(
        state.path(),
        &[&["report", "--verbose"][..], &ACCEPTANCE_RANGE].concat(),
    )?);
    assert!(
        words(&text).contains("data years 2024 to 2030: disclosed by the provider "),
        "{text}"
    );
    Ok(())
}

/// `path` with symbolic links resolved, as tokendraw names checkouts; on
/// Windows without the `\\?\` prefix.
fn real(path: &Path) -> Result<PathBuf, std::io::Error> {
    let path = std::fs::canonicalize(path)?;
    let text = path.to_string_lossy();
    Ok(match text.strip_prefix(r"\\?\") {
        Some(drive) if !drive.starts_with(r"UNC\") => PathBuf::from(drive),
        _ => path,
    })
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

struct Projects {
    _state: TempDir,
    root: PathBuf,
    repo: PathBuf,
    worktree: PathBuf,
}

impl Projects {
    fn new() -> TestResult<Self> {
        let state = TempDir::new()?;
        let root = real(state.path())?;
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git/worktrees/topic"))?;
        std::fs::write(repo.join(".git/worktrees/topic/commondir"), "../..\n")?;
        let worktree = root.join("repo-worktrees").join("topic");
        std::fs::create_dir_all(&worktree)?;
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", text(&repo.join(".git/worktrees/topic"))),
        )?;
        let record = |id: &str, session: &str, cwd: Option<&Path>| {
            serde_json::json!({
                "id": id, "timestamp": "2026-09-29T12:00:00Z", "provider": "example-proxy",
                "model": "example-medium", "session_id": session, "cwd": cwd.map(text),
                "tokens": {"uncached_input": 300, "cache_read": 200, "output": 50},
            })
            .to_string()
        };
        let gone = root.join("repo-worktrees").join("gone");
        let records = [
            record("r1", "main", Some(&repo)),
            record("r2", "topic", Some(&worktree.join("src"))),
            record("r3", "topic", Some(&worktree)),
            record("r4", "earlier", Some(&gone)),
            record("r5", "nowhere", None),
        ];
        let adapters = adapters_dir(&root);
        std::fs::create_dir_all(&adapters)?;
        std::fs::write(adapters.join("example-command.toml"), command_adapter())?;
        std::fs::write(adapters.join("records.jsonl"), records.join("\n") + "\n")?;
        std::fs::create_dir_all(worktree.join("src"))?;
        Ok(Self {
            _state: state,
            root,
            repo,
            worktree,
        })
    }

    fn exit(&self, args: &[&str], code: i32) -> TestResult<Output> {
        let empty = self.root.join("no-logs");
        let output = tokendraw_with_state(&self.root, args)
            .env("CLAUDE_CONFIG_DIR", &empty)
            .env("CODEX_HOME", &empty)
            .env("PI_CODING_AGENT_DIR", &empty)
            .env("USERPROFILE", &self.root)
            .output()?;
        assert_eq!(output.status.code(), Some(code), "{args:?}: {output:?}");
        Ok(output)
    }

    fn run(&self, args: &[&str]) -> TestResult<Output> {
        self.exit(args, 0)
    }

    /// The error of a run that fails, with `/` as the separator.
    fn refused(&self, args: &[&str]) -> TestResult<String> {
        Ok(stderr(&self.exit(args, 1)?).replace('\\', "/"))
    }

    fn json(&self, args: &[&str]) -> TestResult<serde_json::Value> {
        Ok(serde_json::from_slice(&self.run(args)?.stdout)?)
    }

    fn shown(&self, args: &[&str]) -> TestResult<String> {
        Ok(stdout(&self.run(args)?)
            .replace(&text(&self.root), "[root]")
            .replace('\\', "/"))
    }
}

#[test]
fn a_removed_worktree_counts_toward_its_project_once_labelled() -> TestResult {
    let projects = Projects::new()?;
    let repo = text(&projects.repo);
    let inventory = projects.json(&["projects", "--json"])?;
    assert_eq!(inventory["projects"][0]["project"], repo.as_str());
    assert_eq!(inventory["projects"][0]["calls"], 3);
    assert_eq!(inventory["projects"][0]["resolved_by"]["checkout"], 3);
    assert_eq!(inventory["unresolved"]["calls"], 2);
    let group = &inventory["unresolved"]["groups"][0];
    assert_eq!(group["directory"], text(&projects.root.join("repo-*")));
    assert_eq!(group["suggestion"]["project"], repo.as_str());
    assert_eq!(inventory["unresolved"]["without_directory"]["calls"], 1);
    insta::assert_snapshot!("projects_before_labels", projects.shown(&["projects"])?);

    std::fs::remove_dir_all(&projects.worktree)?;
    let report = projects.json(&["report", "--json", "--project", &repo])?;
    assert_eq!(report["project"], repo.as_str());
    assert_eq!(
        report["observed_totals"]["calls"], 3,
        "recorded while it existed"
    );
    assert!(report.get("projects").is_none());

    let pattern = text(&projects.root.join("repo-*"));
    let labelled = projects.shown(&["label", "dir", &pattern, &format!("project={repo}/src")])?;
    insta::assert_snapshot!("label_dir", labelled);
    let labelled = projects.shown(&[
        "label",
        "session",
        "example-command",
        "nowhere",
        "project=~/repo",
        "topic=t",
    ])?;
    insta::assert_snapshot!("label_session", labelled);
    insta::assert_snapshot!("label_list", projects.shown(&["label", "list"])?);

    let report = projects.json(&["report", "--json"])?;
    assert_eq!(report["projects"][0]["calls"], 5);
    assert_eq!(report["projects"][0]["resolved_by"]["label"], 2);
    assert_eq!(report["projects"][0]["resolved_by"]["recorded"], 2);
    assert_eq!(report["projects"][0]["resolved_by"]["checkout"], 1);
    assert_eq!(report["unresolved"]["calls"], 0);
    insta::assert_snapshot!("report_projects", projects.shown(&["report"])?);

    let removed = projects.shown(&[
        "label",
        "session",
        "example-command",
        "nowhere",
        "project=",
        "topic=",
    ])?;
    insta::assert_snapshot!("label_removed", removed);
    let report = projects.json(&["report", "--json"])?;
    assert_eq!(report["unresolved"]["calls"], 1);
    assert_eq!(report["unresolved"]["calls_without_directory"], 1);
    Ok(())
}

#[test]
fn a_project_path_that_selects_nothing_is_refused_with_where_its_calls_go() -> TestResult {
    let projects = Projects::new()?;
    let container = text(&projects.root.join("repo-worktrees"));
    let gone = text(&projects.root.join("repo-worktrees").join("gone"));
    let typo = text(&projects.root.join("repp"));
    assert_eq!(
        projects.refused(&["report", "--project", &typo])?,
        "tokendraw: no calls were made in ~/repp or attributed to it. Run `tokendraw projects` to list the projects.\n"
    );
    assert_eq!(
        projects.refused(&["report", "--project", &container])?,
        "tokendraw: ~/repo-worktrees is no project: of the calls made there, 2 are attributed to ~/repo and 1 belongs to no project. Run `tokendraw report --project ~/repo`, or `tokendraw projects` to assign the rest.\n"
    );

    let report = projects.json(&["report", "--json", "--project", &gone])?;
    assert_eq!(report["project"], serde_json::Value::Null);
    assert_eq!(report["directory"], gone.as_str());
    assert_eq!(report["observed_totals"]["calls"], 1);
    let redacted = projects.json(&["report", "--json", "--redact", "--project", &gone])?;
    assert_eq!(redacted["directory"], "redacted");
    let shown = projects.shown(&["report", "--project", &gone])?;
    let header = format!(
        "tokendraw report, unresolved calls in {}, ",
        gone.replace(&text(&projects.root), "~").replace('\\', "/")
    );
    assert!(shown.starts_with(&header), "{shown}");
    assert!(
        shown.contains("These calls belong to no project. Run `tokendraw projects` to list them"),
        "{shown}"
    );

    let pattern = text(&projects.root.join("repo-*"));
    projects.run(&["label", "dir", &pattern, "project=~/repo"])?;
    assert_eq!(
        projects.refused(&["report", "--project", &container])?,
        "tokendraw: ~/repo-worktrees is no project: the calls made there are attributed to ~/repo. Run `tokendraw report --project ~/repo`.\n",
        "a pattern label assigns the unresolved call"
    );
    Ok(())
}

#[test]
fn projects_and_unresolved_calls_add_up_to_the_total() -> TestResult {
    let projects = Projects::new()?;
    let report = projects.json(&["report", "--json"])?;
    let rows: Vec<&serde_json::Value> = report["projects"]
        .as_array()
        .ok_or("no projects")?
        .iter()
        .chain([&report["unresolved"]])
        .collect();
    let calls: u64 = rows.iter().filter_map(|row| row["calls"].as_u64()).sum();
    assert_eq!(Some(calls), report["observed_totals"]["calls"].as_u64());
    let emissions = report["estimates"]
        .as_array()
        .ok_or("no estimates")?
        .iter()
        .find(|estimate| estimate["quantity"] == "emissions")
        .ok_or("no emissions")?;
    for value in [
        "lower_bound",
        "low_case",
        "base_case",
        "high_case",
        "upper_bound",
    ] {
        let sum: f64 = rows
            .iter()
            .filter_map(|row| row["emissions_g"][value].as_f64())
            .sum();
        let total = emissions[value].as_f64().ok_or("no value")?;
        assert!(
            (sum - total).abs() <= total * 1e-9,
            "{value}: {sum} != {total}"
        );
    }
    Ok(())
}

/// One subagent line of the call `id`; `stop_reason` is `None` on a line
/// without the final usage.
fn subagent_line(id: &str, output: u64, stop_reason: Option<&str>) -> String {
    let line = serde_json::json!({
        "type": "assistant",
        "sessionId": "0b6d2c1e-0000-4000-8000-000000000009",
        "timestamp": "2026-09-30T12:00:00Z",
        "cwd": work("gamma"),
        "entrypoint": "cli",
        "message": {
            "id": id,
            "model": "claude-opus-5-5",
            "content": [{"type": "tool_use", "name": "Read", "input": {}}],
            "stop_reason": stop_reason,
            "usage": {
                "input_tokens": 1,
                "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 1000,
                "output_tokens": output,
            }
        }
    });
    format!("{line}\n")
}

/// Claude Code logs under `dir` with 30 complete subagent calls that grow
/// by 100 output tokens from first to final line, and one call whose final
/// line is missing.
fn logs_with_a_missing_final_line(dir: &Path) -> std::io::Result<()> {
    let subagents = dir.join("projects/-work-gamma/0b6d2c1e-0000-4000-8000-000000000009/subagents");
    std::fs::create_dir_all(&subagents)?;
    let mut text = String::new();
    for index in 0..30 {
        let id = format!("msg_done{index}");
        text.push_str(&subagent_line(&id, 10, None));
        text.push_str(&subagent_line(&id, 110, Some("tool_use")));
    }
    text.push_str(&subagent_line("msg_cut", 5, None));
    std::fs::write(subagents.join("agent-g1.jsonl"), text)
}

#[test]
fn report_adds_estimated_output_for_calls_without_their_final_usage() -> TestResult {
    let state = TempDir::new()?;
    let logs = state.path().join("logs");
    logs_with_a_missing_final_line(&logs.join("claude"))?;
    let report = |args: &[&str]| {
        tokendraw_with_state(state.path(), args)
            .env("CLAUDE_CONFIG_DIR", logs.join("claude"))
            .env("CODEX_HOME", logs.join("codex"))
            .env("PI_CODING_AGENT_DIR", logs.join("pi"))
            .output()
    };
    let output = report(&["report", "--json"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let claude = &json["sources"][0];
    assert_eq!(claude["client"], "claude-code");
    assert_eq!(claude["tokens"]["output"], 30 * 110 + 5);
    assert_eq!(
        claude["final_usage"],
        serde_json::json!({
            "missing": 1, "missing_logged_output": 5, "estimated_missing_output": 100,
            "missing_without_estimate": 0, "unknown": 0
        })
    );
    for estimate in json["estimates"].as_array().into_iter().flatten() {
        assert_eq!(estimate["activity_quality"], "E", "{estimate}");
    }
    let output = report(&["report"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(stdout(&output));
    Ok(())
}

/// Codex rollouts with 30 recorded `gpt-6-astra` compactions, a quarter of
/// their input uncached and 60 output tokens each, and two compactions
/// without a record, each after a 100,000-token call: one `gpt-6-astra`,
/// one `gpt-5.5`.
fn logs_with_unrecorded_compactions(dir: &Path) -> std::io::Result<()> {
    let sessions = dir.join("sessions/2026/07/01");
    std::fs::create_dir_all(&sessions)?;
    let usage = |input: u64, cached: u64, output: u64| {
        serde_json::json!({
            "input_tokens": input, "cached_input_tokens": cached,
            "output_tokens": output, "total_tokens": input + output,
        })
    };
    let rollout = |session: &str, model: &str, lines: Vec<serde_json::Value>| {
        let mut all = vec![
            serde_json::json!({"timestamp": "2026-07-01T10:00:00Z", "type": "session_meta",
                "payload": {"id": session, "timestamp": "2026-07-01T10:00:00Z",
                            "model_provider": "openai", "source": "exec", "cwd": work("delta")}}),
            serde_json::json!({"timestamp": "2026-07-01T10:00:00Z", "type": "turn_context",
                "payload": {"model": model, "cwd": work("delta")}}),
        ];
        all.extend(lines);
        let text: String = all.iter().map(|line| line.to_string() + "\n").collect();
        std::fs::write(
            sessions.join(format!("rollout-2026-07-01T10-00-00-{session}.jsonl")),
            text,
        )
    };
    let recorded = (0..30)
        .flat_map(|index| {
            let id = format!("resp_{index}");
            [
                serde_json::json!({"timestamp": "2026-07-01T10:00:01Z", "type": "token_usage_record",
                    "payload": {"response_id": id, "usage": usage(1000, 750, 60)}}),
                serde_json::json!({"timestamp": "2026-07-01T10:00:01Z", "type": "compacted",
                    "payload": {"message": "", "compaction_response_id": id}}),
            ]
        })
        .collect();
    rollout("recorded", "gpt-6-astra", recorded)?;
    for (session, model) in [("astra", "gpt-6-astra"), ("old", "gpt-5.5")] {
        rollout(
            session,
            model,
            vec![
                serde_json::json!({"timestamp": "2026-07-01T10:00:02Z", "type": "event_msg",
                    "payload": {"type": "token_count",
                                "info": {"total_token_usage": usage(100_000, 0, 5)}}}),
                serde_json::json!({"timestamp": "2026-07-01T10:00:03Z", "type": "compacted",
                    "payload": {"message": ""}}),
            ],
        )?;
    }
    Ok(())
}

#[test]
fn report_adds_estimated_tokens_for_compactions_without_a_usage_record() -> TestResult {
    let state = TempDir::new()?;
    let logs = state.path().join("logs");
    logs_with_unrecorded_compactions(&logs.join("codex"))?;
    let run = |args: &[&str]| {
        tokendraw_with_state(state.path(), args)
            .env("CLAUDE_CONFIG_DIR", logs.join("claude"))
            .env("CODEX_HOME", logs.join("codex"))
            .env("PI_CODING_AGENT_DIR", logs.join("pi"))
            .output()
    };
    let output = run(&["report", "--json"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let codex = json["sources"]
        .as_array()
        .and_then(|sources| sources.iter().find(|source| source["client"] == "codex"))
        .ok_or("no codex source")?;
    assert_eq!(codex["calls"], 34);
    assert_eq!(codex["tokens"]["output"], 30 * 60 + 2 * 5);
    assert_eq!(
        codex["unrecorded"],
        serde_json::json!({
            "calls": 2, "estimated_uncached_input": 50_000, "estimated_cache_read": 150_000,
            "estimated_output": 60, "without_input_estimate": 0, "without_output_estimate": 1
        })
    );
    for estimate in json["estimates"].as_array().into_iter().flatten() {
        assert_eq!(estimate["activity_quality"], "E", "{estimate}");
    }
    let output = run(&["report"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!("report_compactions", stdout(&output));
    let output = run(&["tokens"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!("tokens_compactions", stdout(&output));
    Ok(())
}

const FIRST_RUN: &str = "reading the agents' logs for the first time";

fn source<'a>(report: &'a serde_json::Value, client: &str) -> TestResult<&'a serde_json::Value> {
    Ok(report["sources"]
        .as_array()
        .and_then(|sources| sources.iter().find(|source| source["client"] == client))
        .ok_or_else(|| format!("no source {client} in {report}"))?)
}

#[test]
fn the_first_read_of_the_logs_says_so_once() -> TestResult {
    let state = TempDir::new()?;
    let first = run(state.path(), &["tokens"])?;
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    assert!(stderr(&first).contains(FIRST_RUN), "{first:?}");
    let second = run(state.path(), &["report"])?;
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    assert_eq!(stderr(&second), "");
    Ok(())
}

#[test]
fn a_ledger_that_a_label_created_still_gets_the_first_read_line() -> TestResult {
    let state = TempDir::new()?;
    let dir = state.path().to_string_lossy().into_owned();
    let label = run(state.path(), &["label", "dir", &dir, "project=example"])?;
    assert_eq!(label.status.code(), Some(0), "{label:?}");
    let report = run(state.path(), &["report"])?;
    assert!(stderr(&report).contains(FIRST_RUN), "{report:?}");
    Ok(())
}

#[test]
fn an_empty_home_reports_where_it_looked_without_a_first_read_line() -> TestResult {
    let state = TempDir::new()?;
    let empty = state.path().join("nothing");
    for _ in 0..2 {
        let output = tokendraw_with_state(state.path(), &["report", "--json"])
            .env("CLAUDE_CONFIG_DIR", empty.join("claude"))
            .env("CODEX_HOME", empty.join("codex"))
            .env("PI_CODING_AGENT_DIR", empty.join("pi"))
            .output()?;
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(stderr(&output), "");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let codex = source(&report, "codex")?;
        assert_eq!(codex["status"], "not_found");
        assert_eq!(codex["location"], "~/nothing/codex/sessions");
        assert_eq!(codex["calls"], 0);
    }
    Ok(())
}

#[test]
fn a_known_client_without_an_adapter_is_found_but_not_read() -> TestResult {
    let state = TempDir::new()?;
    std::fs::create_dir_all(state.path().join("opencode"))?;
    std::fs::write(state.path().join("opencode/opencode.db"), "")?;
    let output = run(state.path(), &["report", "--json"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let opencode = source(&report, "opencode")?;
    assert_eq!(opencode["status"], "found_not_read");
    assert_eq!(opencode["location"], "~/opencode/opencode.db");
    assert_eq!(opencode["reason"], "no adapter");
    for field in [
        "calls",
        "sessions",
        "tokens",
        "activity_quality",
        "final_usage",
        "client_versions",
    ] {
        assert!(opencode[field].is_null(), "{field}: {opencode}");
    }
    let text = stdout(&run(state.path(), &["report"])?);
    assert!(
        text.contains("opencode      found, not read   ~/opencode/opencode.db (no adapter)"),
        "{text}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_unreadable_log_directory_is_found_not_read_and_the_others_are_read() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let state = TempDir::new()?;
    let pi = state.path().join("pi");
    std::fs::create_dir_all(pi.join("sessions"))?;
    std::fs::set_permissions(pi.join("sessions"), std::fs::Permissions::from_mode(0o000))?;
    let readable = std::fs::read_dir(pi.join("sessions")).is_ok();
    let output = tokendraw_with_state(state.path(), &["report", "--json"])
        .env("PI_CODING_AGENT_DIR", &pi)
        .output();
    std::fs::set_permissions(pi.join("sessions"), std::fs::Permissions::from_mode(0o755))?;
    let output = output?;
    // A superuser reads it regardless, so there is nothing to test.
    if readable {
        return Ok(());
    }
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let pi = source(&report, "pi")?;
    assert_eq!(pi["status"], "found_not_read");
    assert_eq!(pi["reason"], "permission denied");
    assert_eq!(source(&report, "codex")?["status"], "read");
    assert!(source(&report, "codex")?["calls"].as_u64() > Some(0));
    Ok(())
}

#[test]
fn a_removed_user_adapter_keeps_its_calls_without_an_adapter() -> TestResult {
    let state = TempDir::new()?;
    let dir = install_adapters(state.path())?;
    let before: serde_json::Value =
        serde_json::from_slice(&run(state.path(), &["report", "--json"])?.stdout)?;
    let calls = source(&before, "example-agent")?["calls"].clone();
    assert_eq!(source(&before, "example-agent")?["status"], "read");
    std::fs::remove_file(dir.join("example-agent.toml"))?;
    let output = run(state.path(), &["report", "--json"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let after: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let agent = source(&after, "example-agent")?;
    assert_eq!(agent["status"], "no_adapter");
    assert!(agent["location"].is_null());
    assert_eq!(agent["calls"], calls);
    let entries = after["sources"].as_array().map_or(0, |sources| {
        sources
            .iter()
            .filter(|s| s["client"] == "example-agent")
            .count()
    });
    assert_eq!(entries, 1);
    let text = stdout(&run(state.path(), &["report"])?);
    assert!(
        text.contains("example-agent     no adapter   calls read earlier stay counted"),
        "{text}"
    );
    Ok(())
}

/// What `report --redact` must leave out: [`Marked`] puts it in every value
/// of the user's that is not an id.
const MARKER: &str = "acme-secret";

const MARKED_OVERRIDES: &str = r#"[sources.own-meter]
title = "Readings of the acme-secret meter"
publisher = "acme-secret studio"
accessed = 2026-10-01

[[provider]]
id = "anthropic"
sites = ["ember:US-IN"]
label = "M"
sources = ["own-meter"]
basis = "Served from the acme-secret office"
reviewed = 2026-10-01

[provider.pue]
lower_bound = 1
low_case = 1
base_case = 1
high_case = 1
upper_bound = 1
label = "M"
sources = ["own-meter"]
basis = "No data centre at acme-secret"

[[geography]]
id = "ember:US-IN"
name = "The acme-secret office"
year = 2026
g_co2e_per_kwh = 400
basis = "metered at acme-secret"
label = "M"
sources = ["own-meter"]
reviewed = 2026-10-01

[[location]]
id = "own-agent"
paths = [{ os = ["linux", "macos", "windows"], path = "~/acme-secret/logs" }]
label = "C"
sources = ["own-meter"]
basis = "Where the acme-secret agent logs"
reviewed = 2026-10-01
"#;

/// A home directory named with [`MARKER`], holding Claude Code's logs at
/// their default location, a project, a user adapter, data overrides with
/// [`MARKED_OVERRIDES`], a shipped known location and the overrides' known
/// location; and beside it a project, Codex's logs and the user adapter's
/// logs under a directory named with the marker.
struct Marked {
    _dir: TempDir,
    home: PathBuf,
    codex: PathBuf,
    /// The project outside the home directory.
    app: PathBuf,
}

impl Marked {
    fn new() -> TestResult<Self> {
        let dir = TempDir::new()?;
        let root = real(dir.path())?;
        let home = root.join(format!("home-{MARKER}"));
        let srv = root.join("srv").join(MARKER);
        let app = srv.join("app");
        let quest = home.join("games").join(format!("{MARKER}-quest"));
        for project in [&app, &quest] {
            std::fs::create_dir_all(project.join(".git"))?;
        }
        let assistant = |id: &str, cwd: &Path, output: u64| {
            serde_json::json!({
                "type": "assistant", "sessionId": format!("session-{id}"),
                "timestamp": "2026-09-30T12:00:00Z", "cwd": text(cwd), "version": "2.1.0",
                "message": {"id": id, "model": "claude-opus-5-5", "stop_reason": "end_turn",
                    "usage": {"input_tokens": 100, "cache_creation_input_tokens": 0,
                              "cache_read_input_tokens": 1000, "output_tokens": output}},
            })
            .to_string()
        };
        let logs = home.join(".claude/projects/-p");
        std::fs::create_dir_all(&logs)?;
        std::fs::write(
            logs.join("s.jsonl"),
            [
                assistant("m1", &app, 400),
                assistant("m2", &app.join("src"), 400),
                assistant("m3", &quest, 100),
            ]
            .join("\n")
                + "\n",
        )?;
        let codex = srv.join("codex");
        std::fs::create_dir_all(codex.join("sessions"))?;
        let agent_logs = srv.join("agent-logs");
        for day in ["2026-09-30/ea-1.jsonl", "2026-10-01/ea-2.jsonl"] {
            let to = agent_logs.join(day);
            std::fs::create_dir_all(to.parent().ok_or("no parent")?)?;
            std::fs::copy(fixtures().join("example-agent/logs").join(day), to)?;
        }
        let adapters = adapters_dir(&home);
        std::fs::create_dir_all(&adapters)?;
        std::fs::write(
            adapters.join("example-agent.toml"),
            agent_adapter("/usage/completion_tokens")
                .replace(
                    &toml_path(&fixtures().join("example-agent/logs")),
                    &toml_path(&agent_logs),
                )
                .replace("\"example-cloud\"", "\"anthropic\""),
        )?;
        let data = data_dir(&home);
        std::fs::create_dir_all(&data)?;
        std::fs::write(data.join(format!("{MARKER}.toml")), MARKED_OVERRIDES)?;
        std::fs::create_dir_all(home.join(MARKER).join("logs"))?;
        std::fs::create_dir_all(home.join("opencode"))?;
        std::fs::write(home.join("opencode/opencode.db"), "")?;
        Ok(Self {
            _dir: dir,
            home,
            codex,
            app,
        })
    }

    fn report(&self, args: &[&str]) -> TestResult<String> {
        let output = tokendraw_with_state(&self.home, &[&["report"][..], args].concat())
            .env_remove("CLAUDE_CONFIG_DIR")
            .env("CODEX_HOME", &self.codex)
            .env_remove("PI_CODING_AGENT_DIR")
            .env_remove("OPENCODE_DB")
            .output()?;
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        Ok(without_tomorrow(&String::from_utf8(output.stdout)?))
    }
}

#[test]
fn report_redact_replaces_projects_locations_overrides_and_their_text() -> TestResult {
    let marked = Marked::new()?;
    let app = text(&marked.app);
    let json = marked.report(&["--json", "--redact"])?;
    let project: serde_json::Value =
        serde_json::from_str(&marked.report(&["--json", "--redact", "--project", &app])?)?;
    assert_eq!(project["project"], "redacted");
    assert_eq!(project["observed_totals"]["calls"], 2);
    insta::assert_snapshot!("report_redact_json", without_platform(&json)?);
    insta::assert_snapshot!("report_redact", marked.report(&["--redact"])?);
    let header = marked.report(&["--redact", "--project", &app])?;
    assert!(
        header.starts_with("tokendraw report, project redacted, "),
        "{header}"
    );
    Ok(())
}

#[test]
fn a_redacted_report_holds_no_value_of_the_users_that_is_not_an_id() -> TestResult {
    let marked = Marked::new()?;
    let app = text(&marked.app);
    let unredacted: serde_json::Value = serde_json::from_str(&marked.report(&["--json"])?)?;
    let location = |client: &str| -> TestResult<String> {
        Ok(source(&unredacted, client)?["location"]
            .as_str()
            .unwrap_or_default()
            .to_owned())
    };
    for client in ["codex", "example-agent", "own-agent"] {
        assert!(location(client)?.contains(MARKER), "{client}: {unredacted}");
    }
    assert!(
        unredacted["data_version"]["overrides"][0]["path"]
            .as_str()
            .is_some_and(|path| path.contains(MARKER))
    );
    assert_eq!(unredacted["projects"][0]["project"], app.as_str());
    assert!(
        unredacted["projects"][1]["project"]
            .as_str()
            .is_some_and(|path| path.contains(MARKER))
    );
    assert_eq!(
        unredacted["grid"]["sources"][0],
        "Readings of the acme-secret meter"
    );
    assert_eq!(unredacted["grid"]["basis"], "metered at acme-secret");
    for args in [
        &["--json"][..],
        &[],
        &["--verbose"],
        &["--json", "--project", &app],
        &["--project", &app],
        &["--verbose", "--project", &app],
    ] {
        let shown = marked.report(args)?;
        assert!(shown.contains(MARKER), "{args:?}: {shown}");
        let redacted = marked.report(&[args, &["--redact"]].concat())?;
        assert!(!redacted.contains(MARKER), "{args:?}: {redacted}");
    }
    Ok(())
}

/// A published figure: the provider whose calls in `docs/validation/calls.jsonl`
/// stand for its workloads, the quantity of the report's L1 group it is
/// compared with, and its value.
struct Disclosure {
    provider: &'static str,
    quantity: &'static str,
    figure: f64,
    unit: &'static str,
}

const DISCLOSURES: [Disclosure; 3] = [
    Disclosure {
        provider: "google",
        quantity: "energy_wh",
        figure: 0.24,
        unit: "Wh",
    },
    Disclosure {
        provider: "openai",
        quantity: "energy_wh",
        figure: 0.34,
        unit: "Wh",
    },
    Disclosure {
        provider: "mistral",
        quantity: "emissions_g",
        figure: 1.14,
        unit: "g CO2e",
    },
];

const BANDS: [&str; 5] = [
    "lower_bound",
    "low_case",
    "base_case",
    "high_case",
    "upper_bound",
];

/// Three significant figures, as the report prints a base case.
fn significant(value: f64) -> String {
    if value >= 100.0 {
        return format!("{value:.0}");
    }
    let magnitude = value.log10().floor();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // the magnitude of a value below 100 that a call can produce is small
    let decimals = (2.0 - magnitude).max(0.0) as usize;
    format!("{value:.decimals$}")
}

fn position(figure: f64, values: &[f64; 5]) -> &'static str {
    let [lower_bound, low_case, base_case, high_case, upper_bound] = *values;
    if figure < lower_bound {
        "below the lower bound"
    } else if figure < low_case {
        "between lower bound and low case"
    } else if figure <= base_case {
        "low case to base case"
    } else if figure <= high_case {
        "base case to high case"
    } else if figure <= upper_bound {
        "between high case and upper bound"
    } else {
        "above the upper bound"
    }
}

/// The tables of `docs/validation.md`: every block of lines that starts
/// with the header `| Input tokens`.
fn validation_tables(note: &str) -> Vec<String> {
    let mut tables = Vec::new();
    let mut table: Option<Vec<&str>> = None;
    for line in note.lines() {
        match &mut table {
            Some(lines) if line.starts_with('|') => lines.push(line),
            Some(lines) => {
                tables.push(lines.join("\n"));
                table = None;
            }
            None if line.starts_with("| Input tokens") => table = Some(vec![line]),
            None => {}
        }
    }
    if let Some(lines) = table {
        tables.push(lines.join("\n"));
    }
    tables
}

#[test]
fn validation_tables_match_the_tool() -> TestResult {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
    let inputs = docs.join("validation");
    let state = TempDir::new()?;
    let data = data_dir(state.path());
    std::fs::create_dir_all(&data)?;
    std::fs::copy(inputs.join("data.toml"), data.join("validation.toml"))?;
    let adapters = adapters_dir(state.path());
    std::fs::create_dir_all(&adapters)?;
    std::fs::write(
        adapters.join("validation.toml"),
        format!(
            "client = \"validation\"\n\n[files]\nroot = {}\n\n[fields]\nid = \"/id\"\ntimestamp = \"/timestamp\"\nsession = \"/session_id\"\nmodel = \"/model\"\nprovider = \"/provider\"\n\n[tokens]\ninput = \"/tokens/uncached_input\"\noutput = \"/tokens/output\"\n",
            toml_path(&inputs)
        ),
    )?;
    let calls = std::fs::read_to_string(inputs.join("calls.jsonl"))?;
    let mut tables = Vec::new();
    for disclosure in &DISCLOSURES {
        let mut table = vec![
            format!(
                "| Input tokens | Parameter set | Lower bound | Low case | Base case | High case | Upper bound | {} {} is |",
                disclosure.figure, disclosure.unit
            ),
            "| --- | --- | --- | --- | --- | --- | --- | --- |".to_owned(),
        ];
        for line in calls.lines() {
            let call: serde_json::Value = serde_json::from_str(line)?;
            if call["provider"] != disclosure.provider {
                continue;
            }
            let since = call["timestamp"]
                .as_str()
                .ok_or("call without a timestamp")?;
            let until = since
                .parse::<jiff::Timestamp>()?
                .checked_add(jiff::SignedDuration::from_mins(1))?
                .to_string();
            let output = run(
                state.path(),
                &["report", "--json", "--since", since, "--until", &until],
            )?;
            assert_eq!(output.status.code(), Some(0), "{output:?}");
            let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            assert_eq!(report["l1_groups"].as_array().map(Vec::len), Some(1));
            let group = &report["l1_groups"][0][disclosure.quantity];
            let mut values = [0.0; 5];
            for (value, band) in values.iter_mut().zip(BANDS) {
                *value = group[band].as_f64().ok_or("band without a value")?;
            }
            let set = report["parameter_sets"][0]["id"]
                .as_str()
                .ok_or("report without a parameter set")?;
            table.push(format!(
                "| {} | `{set}` | {} | {} | {} | {} | {} | {} |",
                call["tokens"]["uncached_input"],
                significant(values[0]),
                significant(values[1]),
                significant(values[2]),
                significant(values[3]),
                significant(values[4]),
                position(disclosure.figure, &values),
            ));
        }
        tables.push(table.join("\n"));
    }
    let note = std::fs::read_to_string(docs.join("validation.md"))?;
    assert_eq!(
        validation_tables(&note),
        tables,
        "the tables the tool gives now:\n\n{}\n",
        tables.join("\n\n")
    );
    Ok(())
}

/// Makes calls with more than `tokens` cache-read tokens long-context, so
/// the synthetic logs, whose calls are short, have some.
fn long_context_above(state: &Path, tokens: u64) -> std::io::Result<()> {
    let dir = data_dir(state);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("long-context.toml"),
        format!(
            "[[finding]]\nid = \"long-context\"\ncached_tokens = {tokens}\nlabel = \"D\"\nsources = [\"anthropic-context-windows\"]\nbasis = \"A threshold for the synthetic logs\"\nreviewed = 2026-10-15\n"
        ),
    )
}

fn write_config(state: &Path, text: &str) -> std::io::Result<PathBuf> {
    let file = adapters_dir(state).with_file_name("config.toml");
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&file, text)?;
    Ok(file)
}

/// The text with the UTC date after today, which the measurement step
/// counts from, written `<tomorrow>` after `--since`.
fn without_tomorrow(text: &str) -> String {
    match jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
        .tomorrow()
    {
        Ok(tomorrow) => text.replace(&format!("--since {tomorrow}"), "--since <tomorrow>"),
        Err(_) => text.to_owned(),
    }
}

/// The lines from the one that is exactly `heading` to the next empty line.
fn section(text: &str, heading: &str) -> String {
    text.lines()
        .skip_while(|line| *line != heading)
        .take_while(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The finding with `code` in the `findings` of a report or of `tokendraw
/// findings`, `null` when there is none.
fn finding<'a>(json: &'a serde_json::Value, code: &str) -> &'a serde_json::Value {
    json["findings"]
        .as_array()
        .and_then(|findings| findings.iter().find(|finding| finding["code"] == code))
        .unwrap_or(&serde_json::Value::Null)
}

fn json_of(output: &Output) -> TestResult<serde_json::Value> {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn report_shows_a_finding_above_the_threshold_with_the_steps_of_its_clients() -> TestResult {
    let state = TempDir::new()?;
    long_context_above(state.path(), 1000)?;
    let output = run(state.path(), &["report"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = stdout(&output);
    insta::assert_snapshot!(section(&text, "long-context"));
    assert!(!text.contains("below 10% of the energy"), "{text}");
    let json = json_of(&run(state.path(), &["report", "--json"])?)?;
    let finding = finding(&json, "long-context")["facts"]["share_of_base_case"].as_f64();
    let breakdown = json["breakdown"][1]["share_of_base_case"].as_f64();
    assert!(
        finding
            .zip(breakdown)
            .is_some_and(|(finding, breakdown)| (finding - breakdown).abs() < 1e-12),
        "every client has steps, so the finding counts the calls the breakdown does: {json}"
    );

    assert_eq!(json["hidden_findings"]["count"], 0);
    Ok(())
}

#[test]
fn the_hidden_findings_command_shows_the_calls_of_the_report() -> TestResult {
    let state = TempDir::new()?;
    long_context_above(state.path(), 1000)?;
    write_config(state.path(), "[findings]\nthreshold_percent = 100\n")?;
    let alpha = work("alpha");
    let selection = [
        "--since",
        "2026-09-30",
        "--until",
        "2026-10-01",
        "--project",
        alpha.as_str(),
    ];
    let text = run(state.path(), &[&["report"][..], &selection].concat())?;
    let text = stdout(&text);
    let expected = format!(
        "3 findings below 100% of the energy:\n`tokendraw findings --since 2026-09-30 --until 2026-10-01 --project {alpha}`"
    );
    assert!(text.contains(&expected), "{text}");
    let report = json_of(&run(
        state.path(),
        &[&["report", "--json"][..], &selection].concat(),
    )?)?;
    assert_eq!(report["findings"], serde_json::json!([]));
    let command = report["hidden_findings"]["command"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let words: Vec<&str> = command.split(' ').collect();
    assert_eq!(words.first(), Some(&"tokendraw"), "{command}");
    let shown = json_of(&run(
        state.path(),
        &[&words[1..], &["--json"][..]].concat(),
    )?)?;
    let share = |json: &serde_json::Value| json["facts"]["share_of_base_case"].as_f64();
    write_config(state.path(), "[findings]\nthreshold_percent = 0\n")?;
    let at_zero = json_of(&run(
        state.path(),
        &[&["report", "--json"][..], &selection].concat(),
    )?)?;
    let long = |json| finding(json, "long-context");
    assert_eq!(share(long(&shown)), share(long(&at_zero)));
    let every_call = json_of(&run(state.path(), &["findings", "--json"])?)?;
    assert_ne!(
        share(long(&every_call)),
        share(long(&shown)),
        "the selection must change the share for this test to show anything"
    );
    Ok(())
}

#[test]
fn findings_lists_a_finding_without_calls_with_its_zero_share() -> TestResult {
    let state = TempDir::new()?;
    let output = run(state.path(), &["findings"])?;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    insta::assert_snapshot!(stdout(&output));
    let json = json_of(&run(state.path(), &["findings", "--json"])?)?;
    let long = finding(&json, "long-context");
    assert_eq!(long["facts"]["calls"], 0);
    assert_eq!(long["facts"]["share_of_base_case"], 0.0);
    assert!(long.get("measure").is_none(), "{long}");
    assert_eq!(json["reason"], serde_json::Value::Null);
    let report = json_of(&run(state.path(), &["report", "--json"])?)?;
    assert_eq!(finding(&report, "long-context"), &serde_json::Value::Null);
    assert_eq!(report["hidden_findings"]["count"], 0);
    Ok(())
}

#[test]
fn findings_without_energy_say_that_no_share_can_be_computed() -> TestResult {
    let state = TempDir::new()?;
    let range = ["--since", "2020-01-01", "--until", "2020-01-02"];
    let json = json_of(&run(
        state.path(),
        &[&["findings", "--json"][..], &range].concat(),
    )?)?;
    assert_eq!(json["findings"], serde_json::Value::Null);
    assert!(json["reason"].is_string(), "{json}");
    let text = run(state.path(), &[&["findings"][..], &range].concat())?;
    assert!(stdout(&text).contains("No findings: no energy was estimated"));
    let report = json_of(&run(
        state.path(),
        &[&["report", "--json"][..], &range].concat(),
    )?)?;
    assert!(report.get("findings").is_none(), "{report}");
    assert!(report.get("hidden_findings").is_none(), "{report}");
    Ok(())
}

#[test]
fn a_client_without_steps_is_left_out_until_an_override_adds_them() -> TestResult {
    let state = TempDir::new()?;
    install_adapters(state.path())?;
    long_context_above(state.path(), 500)?;
    let long = |json| finding(json, "long-context");
    let calls = |json| long(json)["facts"]["calls"].as_u64();
    let without = json_of(&run(state.path(), &["findings", "--json"])?)?;
    assert!(
        long(&without)["actions"].get("example-agent").is_none(),
        "{without}"
    );
    std::fs::write(
        data_dir(state.path()).join("steps.toml"),
        "[sources.agent-docs]\ntitle = \"Commands\"\npublisher = \"Example\"\naccessed = 2026-10-15\n\n[[action]]\nfinding = \"long-context\"\nclient = \"example-agent\"\nsteps = [\"/reset between tasks\"]\nlabel = \"C\"\nsources = [\"agent-docs\"]\nbasis = \"The agent's commands\"\nreviewed = 2026-10-15\n",
    )?;
    let with = json_of(&run(state.path(), &["findings", "--json"])?)?;
    assert_eq!(
        long(&with)["actions"]["example-agent"],
        serde_json::json!(["/reset between tasks"])
    );
    assert!(calls(&with) > calls(&without), "{with}");
    let redacted = json_of(&run(state.path(), &["report", "--json", "--redact"])?)?;
    let actions = &long(&redacted)["actions"];
    assert_eq!(actions["example-agent"], serde_json::json!(["redacted"]));
    assert_eq!(
        actions["codex"],
        serde_json::json!(["/compact during a task", "/new between tasks"])
    );
    Ok(())
}

#[test]
fn the_configuration_file_named_by_the_variable_sets_the_threshold() -> TestResult {
    let state = TempDir::new()?;
    long_context_above(state.path(), 1000)?;
    let elsewhere = state.path().join("elsewhere.toml");
    std::fs::write(&elsewhere, "[findings]\nthreshold_percent = 99\n")?;
    let output = tokendraw_with_state(state.path(), &["report", "--json"])
        .env("TOKENDRAW_CONFIG", &elsewhere)
        .output()?;
    let json = json_of(&output)?;
    assert_eq!(
        json["hidden_findings"]["threshold_share_of_base_case"],
        0.99
    );
    assert_eq!(json["hidden_findings"]["count"], 3);
    let missing = tokendraw_with_state(state.path(), &["report"])
        .env("TOKENDRAW_CONFIG", state.path().join("missing.toml"))
        .output()?;
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    assert!(stderr(&missing).contains("set TOKENDRAW_CONFIG to a readable file or unset it"));
    write_config(state.path(), "[findings]\nthreshold_percent = 120\n")?;
    let invalid = run(state.path(), &["findings"])?;
    assert_eq!(invalid.status.code(), Some(1), "{invalid:?}");
    assert!(
        stderr(&invalid).contains("it must be from 0 to 100; fix or remove the file"),
        "{invalid:?}"
    );
    Ok(())
}
