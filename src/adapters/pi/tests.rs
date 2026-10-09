use std::fs;

use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;

const SESSION: &str = "0b7e0000-0000-4000-8000-000000000001";
const FORK: &str = "0b7e0000-0000-4000-8000-000000000002";

fn adapter() -> Pi {
    Pi::new(PathBuf::from("/logs/sessions"))
}

fn session_file() -> PathBuf {
    PathBuf::from(format!(
        "/logs/sessions/--work-alpha--/2026-09-30T12-00-00-000Z_{SESSION}.jsonl"
    ))
}

fn header(id: &str) -> Value {
    json!({
        "type": "session",
        "version": 3,
        "id": id,
        "timestamp": "2026-09-30T12:00:00.000Z",
        "cwd": "/work/alpha",
    })
}

/// Usage as pi writes it; the total is the sum of the classes.
fn usage(input: u64, cache_read: u64, cache_write: u64, output: u64) -> Value {
    json!({
        "input": input,
        "output": output,
        "cacheRead": cache_read,
        "cacheWrite": cache_write,
        "totalTokens": input + output + cache_read + cache_write,
        "cost": {"input": 0.01, "output": 0.02, "cacheRead": 0, "cacheWrite": 0, "total": 0.03},
    })
}

fn assistant(id: &str, timestamp: &str, response_id: Option<&str>, usage: Value) -> Value {
    let mut message = json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "not stored"}],
        "api": "openai-completions",
        "provider": "nebius",
        "model": "moonshotai/Kimi-K3",
        "stopReason": "toolUse",
        "timestamp": 1_790_769_600_000_u64,
    });
    message["usage"] = usage;
    if let Some(response_id) = response_id {
        message["responseId"] = json!(response_id);
    }
    entry(id, timestamp, message)
}

fn entry(id: &str, timestamp: &str, message: Value) -> Value {
    let mut entry = json!({"type": "message", "id": id, "parentId": null, "timestamp": timestamp});
    entry["message"] = message;
    entry
}

fn user(id: &str) -> Value {
    entry(
        id,
        "2026-09-30T12:00:01.000Z",
        json!({"role": "user", "content": [{"type": "text", "text": "usage"}], "timestamp": 1}),
    )
}

fn read(lines: &[Value]) -> Vec<Line> {
    let mut reader = adapter().start(&session_file());
    lines
        .iter()
        .map(|line| reader.parse_line(line.to_string().as_bytes()))
        .collect()
}

fn calls(lines: &[Value]) -> Vec<Call> {
    read(lines)
        .into_iter()
        .filter_map(|line| match line {
            Line::Call(call) => Some(call),
            _ => None,
        })
        .collect()
}

#[test]
fn an_assistant_message_becomes_a_call_with_its_token_classes() -> Result<(), jiff::Error> {
    let found = calls(&[
        header(SESSION),
        user("a1"),
        assistant(
            "a2",
            "2026-09-30T12:00:05.250Z",
            Some("resp-1"),
            usage(300, 5000, 40, 120),
        ),
    ]);
    assert_eq!(
        found,
        [Call {
            id: "resp-1".to_owned(),
            provider: "nebius".to_owned(),
            model: "moonshotai/Kimi-K3".to_owned(),
            timestamp: "2026-09-30T12:00:05.250Z".parse()?,
            session_id: SESSION.to_owned(),
            cwd: Some("/work/alpha".to_owned()),
            role: Role::Interactive,
            client_version: None,
            tokens: TokenClasses {
                uncached_input: Tokens(300),
                cache_write: Tokens(40),
                cache_read: Tokens(5000),
                output: Tokens(120),
                reasoning: None,
            },
            completion: Completion::FINAL,
            compaction: false,
        }]
    );
    Ok(())
}

#[test]
fn a_message_without_usage_is_no_call() {
    let mut failed = assistant("a2", "2026-09-30T12:00:05Z", None, usage(0, 0, 0, 0));
    failed["message"]["stopReason"] = json!("error");
    failed["message"]["errorMessage"] = json!("not stored");
    assert_eq!(read(&[header(SESSION), failed]), [Line::Other, Line::Other]);
}

#[test]
fn a_message_without_a_response_id_is_identified_by_its_session_and_entry() {
    let found = calls(&[
        header(SESSION),
        assistant("a2", "2026-09-30T12:00:05Z", None, usage(1, 0, 0, 1)),
    ]);
    assert_eq!(ids(&found), [format!("{SESSION}#a2")]);
}

fn ids(calls: &[Call]) -> Vec<String> {
    calls.iter().map(|call| call.id.clone()).collect()
}

/// A fork created at `created`, with its parent's two messages copied and
/// one message of its own.
fn fork(id: &str, created: &str) -> Vec<Value> {
    let mut header = header(id);
    header["timestamp"] = json!(created);
    header["parentSession"] = json!(session_file());
    vec![
        header,
        assistant(
            "a2",
            "2026-09-30T12:00:05Z",
            Some("resp-1"),
            usage(1, 0, 0, 1),
        ),
        assistant("a3", "2026-09-30T12:00:06Z", None, usage(1, 0, 0, 1)),
        assistant(
            "b1",
            "2026-09-30T13:00:05Z",
            Some("resp-2"),
            usage(1, 0, 0, 1),
        ),
    ]
}

#[test]
fn a_forks_copied_messages_are_its_parents_and_only_its_own_are_calls() {
    let found = calls(&fork(FORK, "2026-09-30T13:00:00Z"));
    assert_eq!(ids(&found), ["resp-2"]);
    assert_eq!(found[0].session_id, FORK);
}

#[test]
fn a_copy_stamped_at_the_forks_creation_is_no_call() {
    let found = calls(&fork(FORK, "2026-09-30T12:00:06Z"));
    assert_eq!(ids(&found), ["resp-2"]);
}

#[test]
fn a_fork_of_a_fork_counts_only_its_own_messages() {
    let mut lines = fork(
        "0b7e0000-0000-4000-8000-000000000003",
        "2026-09-30T14:00:00Z",
    );
    lines.push(assistant(
        "c1",
        "2026-09-30T14:00:05Z",
        Some("resp-3"),
        usage(1, 0, 0, 1),
    ));
    assert_eq!(ids(&calls(&lines)), ["resp-3"]);
}

#[test]
fn a_new_session_that_names_a_parent_without_copying_counts_its_messages() {
    let mut header = header(SESSION);
    header["parentSession"] = json!("/logs/sessions/--work-alpha--/earlier.jsonl");
    let found = calls(&[
        header,
        assistant(
            "a2",
            "2026-09-30T12:00:05Z",
            Some("resp-1"),
            usage(1, 0, 0, 1),
        ),
    ]);
    assert_eq!(ids(&found), ["resp-1"]);
}

#[test]
fn each_message_names_its_own_provider_and_model() {
    let mut other = assistant(
        "a3",
        "2026-09-30T12:00:06Z",
        Some("resp-2"),
        usage(1, 0, 0, 1),
    );
    other["message"]["provider"] = json!("litellm");
    other["message"]["model"] = json!("glm-5.2");
    let found = calls(&[
        header(SESSION),
        assistant(
            "a2",
            "2026-09-30T12:00:05Z",
            Some("resp-1"),
            usage(1, 0, 0, 1),
        ),
        other,
    ]);
    let models: Vec<(&str, &str)> = found
        .iter()
        .map(|call| (call.provider.as_str(), call.model.as_str()))
        .collect();
    assert_eq!(
        models,
        [("nebius", "moonshotai/Kimi-K3"), ("litellm", "glm-5.2")]
    );
}

#[test]
fn a_message_before_the_header_is_unreadable() {
    let used = assistant(
        "a2",
        "2026-09-30T12:00:05Z",
        Some("resp-1"),
        usage(1, 0, 0, 1),
    );
    assert_eq!(read(&[used]), [Line::Unreadable]);
}

#[test]
fn other_entries_and_unknown_fields_record_no_usage() {
    let mut header = header(SESSION);
    header["future"] = json!({"nested": [1, 2]});
    let mut tool_result = user("a3");
    tool_result["message"]["role"] = json!("toolResult");
    let found = read(&[
        header,
        json!({"type": "model_change", "id": "a1", "parentId": null,
               "timestamp": "2026-09-30T12:00:00Z", "provider": "nebius", "modelId": "m"}),
        user("a2"),
        tool_result,
        json!({"type": "compaction", "id": "a4", "summary": "not stored"}),
        json!({"type": "world_state", "message": {"role": "assistant"}}),
    ]);
    assert_eq!(found, vec![Line::Other; 6]);
}

#[test]
fn lines_that_cannot_be_read_are_unreadable() {
    let mut fractional = assistant(
        "a2",
        "2026-09-30T12:00:05Z",
        Some("resp-1"),
        usage(1, 0, 0, 1),
    );
    fractional["message"]["usage"]["output"] = json!(1.5);
    let mut untimed = assistant(
        "a3",
        "2026-09-30T12:00:05Z",
        Some("resp-2"),
        usage(1, 0, 0, 1),
    );
    untimed["timestamp"] = json!(null);
    let mut reader = adapter().start(&session_file());
    assert_eq!(
        reader.parse_line(b"{\"type\":\"message\",\"message\":{\"role\""),
        Line::Unreadable
    );
    assert_eq!(
        reader.parse_line(b"{\"type\":\"message\",\"message\":\"assistant\"}"),
        Line::Unreadable
    );
    assert_eq!(
        read(&[header(SESSION), fractional, untimed])[1..],
        [Line::Unreadable, Line::Unreadable]
    );
}

#[test]
fn saved_state_that_cannot_be_read_starts_nothing() {
    assert!(adapter().resume(&session_file(), "{\"session\":").is_none());
}

#[test]
fn session_files_in_every_directory_are_discovered() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let project = root.path().join("--work-alpha--");
    fs::create_dir_all(&project)?;
    fs::write(project.join("2026-09-30T12-00-00-000Z_a.jsonl"), "")?;
    fs::write(project.join("notes.txt"), "")?;
    let found = Pi::new(root.path().to_path_buf()).discover().files;
    assert_eq!(found, [project.join("2026-09-30T12-00-00-000Z_a.jsonl")]);
    Ok(())
}

/// When the `n`th message of [`session`] was written.
fn written(n: usize) -> String {
    format!("2026-09-30T12:{:02}:{:02}Z", n / 60, n % 60)
}

/// `steps` are (input, cache read, output). With `forked_after`, the
/// session is a fork created when that message was written.
fn session(steps: &[(u16, u16, u16)], forked_after: Option<usize>) -> Vec<Value> {
    let mut header = header(SESSION);
    if let Some(n) = forked_after {
        header["timestamp"] = json!(written(n));
        header["parentSession"] = json!("/logs/sessions/--work-alpha--/parent.jsonl");
    }
    let mut lines = vec![header];
    for (n, &(input, cache_read, output)) in steps.iter().enumerate() {
        lines.push(user(&format!("u{n}")));
        lines.push(assistant(
            &format!("a{n}"),
            &written(n),
            (n % 2 == 0).then_some(format!("resp-{n}")).as_deref(),
            usage(
                u64::from(input),
                u64::from(cache_read),
                0,
                u64::from(output),
            ),
        ));
    }
    lines
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let mut reader = adapter().start(&session_file());
        let _ = reader.parse_line(&bytes);
    }

    #[test]
    fn json_values_of_any_shape_never_panic(value in arbitrary_json()) {
        let mut reader = adapter().start(&session_file());
        let _ = reader.parse_line(value.to_string().as_bytes());
        for kind in ["session", "message"] {
            let mut line = value.clone();
            if let Value::Object(map) = &mut line {
                map.insert("type".to_owned(), json!(kind));
            }
            let _ = reader.parse_line(line.to_string().as_bytes());
        }
        let mut used = assistant("a2", "2026-09-30T12:00:05Z", Some("resp-1"), usage(1, 0, 0, 1));
        used["message"]["usage"] = value;
        let _ = reader.parse_line(used.to_string().as_bytes());
    }

    #[test]
    fn the_calls_add_up_to_the_usage_of_every_message(
        steps in proptest::collection::vec((0..500_u16, 0..500_u16, 0..50_u16), 0..20),
    ) {
        let found = calls(&session(&steps, None));
        let sum = |class: fn(&TokenClasses) -> Tokens| -> u64 {
            found.iter().map(|call| class(&call.tokens).0).sum()
        };
        let total = |step: fn(&(u16, u16, u16)) -> u16| -> u64 {
            steps.iter().map(|s| u64::from(step(s))).sum()
        };
        prop_assert_eq!(sum(|tokens| tokens.uncached_input), total(|s| s.0));
        prop_assert_eq!(sum(|tokens| tokens.cache_read), total(|s| s.1));
        prop_assert_eq!(sum(|tokens| tokens.output), total(|s| s.2));
        let nonzero = steps.iter().filter(|(a, b, c)| a + b + c > 0).count();
        prop_assert_eq!(found.len(), nonzero);
    }

    #[test]
    fn reading_in_two_parts_with_saved_state_equals_reading_at_once(
        steps in proptest::collection::vec((0..500_u16, 0..500_u16, 0..50_u16), 1..12),
        split in any::<prop::sample::Index>(),
        forked_after in any::<Option<prop::sample::Index>>(),
    ) {
        let lines = session(&steps, forked_after.map(|n| n.index(steps.len())));
        let at = split.index(lines.len() + 1);
        let whole = read(&lines);
        let mut first = adapter().start(&session_file());
        let mut parts: Vec<Line> = lines[..at]
            .iter()
            .map(|line| first.parse_line(line.to_string().as_bytes()))
            .collect();
        let saved = first.save().unwrap_or_default();
        let Some(mut second) = adapter().resume(&session_file(), &saved) else {
            panic!("a reader's own saved state resumes");
        };
        parts.extend(lines[at..].iter().map(|line| second.parse_line(line.to_string().as_bytes())));
        prop_assert_eq!(parts, whole);
    }
}

fn arbitrary_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        ".{0,8}".prop_map(Value::from),
    ];
    leaf.prop_recursive(3, 32, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::from),
            proptest::collection::btree_map(
                prop_oneof![
                    Just("type".to_owned()),
                    Just("message".to_owned()),
                    Just("role".to_owned()),
                    Just("usage".to_owned()),
                    ".{0,6}"
                ],
                inner,
                0..4
            )
            .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}
