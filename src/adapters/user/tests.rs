use std::fs;

use super::*;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A log adapter file with `ROOT` for its root.
const LOGS: &str = r#"client = "example"

[files]
root = ROOT

[when]
"/type" = "response"

[fields]
timestamp = "/time"
model = "/model"

[values]
provider = "example-cloud"

[tokens]
input = "/usage/input"
output = "/usage/output"
"#;

/// The root `/logs` made absolute on the system running the test, as a
/// TOML literal string: on Windows a path without a drive is not
/// absolute, and its separators would need escaping in a basic string.
fn logs_root() -> String {
    let root = std::path::absolute("/logs")
        .unwrap_or_else(|error| panic!("cannot make /logs absolute: {error}"));
    format!("'{}'", root.display())
}

fn logs() -> String {
    LOGS.replace("ROOT", &logs_root())
}

fn problem(text: &str) -> String {
    match parse(Path::new("/adapters/example.toml"), text, &["claude-code"]) {
        Ok(_) => String::new(),
        Err(problem) => problem,
    }
}

#[test]
fn a_log_adapter_file_declares_a_declared_adapter() {
    assert_eq!(problem(&logs()), "");
    assert!(matches!(
        parse(Path::new("/adapters/example.toml"), &logs(), &[]),
        Ok(UserAdapter::Declared(_))
    ));
}

#[test]
fn a_command_file_declares_an_external_adapter() {
    let text = "client = \"tool\"\ncommand = [\"tool\", \"--records\"]\n";
    assert!(matches!(
        parse(Path::new("/adapters/tool.toml"), text, &[]),
        Ok(UserAdapter::External(_))
    ));
}

#[test]
fn invalid_files_name_what_is_wrong() {
    let cases = [
        (
            logs().replace("[files]", "command = [\"tool\"]\n[files]"),
            "both command and [files]",
        ),
        (
            "client = \"tool\"\n".to_owned(),
            "neither command nor [files]",
        ),
        (
            "client = \"tool\"\ncommand = []\n".to_owned(),
            "command is empty",
        ),
        (
            "client = \"tool\"\ncommand = [\"t\"]\nusage = \"cumulative\"\n".to_owned(),
            "a command adapter takes no",
        ),
        (
            logs().replace("client = \"example\"", "client = \"claude-code\""),
            "shipped adapter's",
        ),
        (
            logs().replace("client = \"example\"", "client = \" \""),
            "client is empty",
        ),
        (LOGS.replace("ROOT", "\"logs\""), "is relative"),
        (
            logs().replace("timestamp = \"/time\"", "timestamp = \"time\""),
            "not a JSON Pointer",
        ),
        (
            logs().replace("\"/type\"", "\"type\""),
            "not a JSON Pointer",
        ),
        (
            logs().replace("timestamp = \"/time\"\n", ""),
            "timestamp is missing",
        ),
        (
            logs().replace("model = \"/model\"\n", ""),
            "model is missing",
        ),
        (
            logs().replace("[values]", "[values]\nmodel = \"m\""),
            "model is set in both",
        ),
        (
            logs()
                .replace("input = \"/usage/input\"\noutput = \"/usage/output\"\n", "")
                .replace("[tokens]\n", ""),
            "[tokens] is missing",
        ),
        (
            logs().replace("[tokens]", "[tokens]\ncache = \"/c\""),
            "line 17: unknown field `cache`",
        ),
        (
            logs().replace("[when]", "colour = 1\n[when]"),
            "unknown field `colour`",
        ),
        (
            format!("usage = \"sometimes\"\n{}", logs()),
            "line 1: unknown variant `sometimes`",
        ),
        ("client = ".to_owned(), "line 1"),
    ];
    for (text, expected) in cases {
        let found = problem(&text);
        assert!(
            found.contains(expected),
            "expected {expected:?} in {found:?} for\n{text}"
        );
    }
}

#[test]
fn comments_formatting_and_order_do_not_change_the_definition() {
    let reordered = r#"
# Example agent's logs.
client = "example"
[tokens]
output = "/usage/output"
input  = "/usage/input"
[values]
provider = "example-cloud"
[fields]
model = "/model"
timestamp = "/time"
[when]
"/type" = "response"
[files]
root = ROOT
"#
    .replace("ROOT", &logs_root());
    let definition = |text: &str| -> Option<String> {
        let generic: Value = toml::from_str(text).ok()?;
        Some(canonical(&generic))
    };
    assert_eq!(definition(&logs()), definition(&reordered));
    assert_ne!(
        definition(&logs()),
        definition(&logs().replace("/usage/output", "/usage/total"))
    );
}

#[test]
fn a_root_in_the_home_directory_is_expanded() -> TestResult {
    let Some(home) = env::home_dir() else {
        return Ok(());
    };
    assert_eq!(root("~/.example/logs")?, home.join(".example/logs"));
    assert_eq!(root("~")?, home);
    Ok(())
}

#[test]
fn a_directory_loads_its_toml_files_in_name_order_and_ignores_the_rest() -> TestResult {
    let dir = tempfile::tempdir()?;
    fs::write(
        dir.path().join("b.toml"),
        "client = \"b\"\ncommand = [\"b\"]\n",
    )?;
    fs::write(
        dir.path().join("a.toml"),
        "client = \"a\"\ncommand = [\"a\"]\n",
    )?;
    fs::write(dir.path().join("a.py"), "print()\n")?;
    fs::create_dir(dir.path().join("nested.toml"))?;
    let clients: Vec<String> = load_dir(dir.path(), &[])?
        .iter()
        .map(|adapter| adapter.client().to_owned())
        .collect();
    assert_eq!(clients, ["a", "b"]);
    Ok(())
}

#[test]
fn a_missing_directory_declares_no_adapters() -> TestResult {
    let dir = tempfile::tempdir()?;
    assert!(load_dir(&dir.path().join("absent"), &[])?.is_empty());
    Ok(())
}

#[test]
fn two_files_declaring_the_same_client_are_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    fs::write(
        dir.path().join("a.toml"),
        "client = \"same\"\ncommand = [\"a\"]\n",
    )?;
    fs::write(
        dir.path().join("b.toml"),
        "client = \"same\"\ncommand = [\"b\"]\n",
    )?;
    let error = load_dir(dir.path(), &[])
        .err()
        .map(|error| error.to_string());
    let error = error.unwrap_or_default();
    assert!(error.contains("client same is also declared by"), "{error}");
    assert!(error.ends_with("fix or remove the file"), "{error}");
    Ok(())
}
