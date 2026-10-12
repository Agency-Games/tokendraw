use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;

fn spec(usage: Usage) -> Spec {
    Spec {
        when: BTreeMap::from([("/type".to_owned(), json!("response"))]),
        id: Some("/response/id".to_owned()),
        timestamp: "/time".to_owned(),
        session: Some("/session".to_owned()),
        cwd: Some("/cwd".to_owned()),
        provider: Text::Value("example-cloud".to_owned()),
        model: Text::Field("/response/model".to_owned()),
        role: Role::Headless,
        tokens: TokenFields {
            input: "/usage/input".to_owned(),
            output: "/usage/output".to_owned(),
            cache_read: Some("/usage/cache_read".to_owned()),
            cache_write: Some("/usage/cache_write".to_owned()),
            reasoning: Some("/usage/reasoning".to_owned()),
            input_includes_cache: false,
            output_includes_reasoning: true,
        },
        usage,
    }
}

/// The root `/logs` made absolute on the system running the test: on
/// Windows a path without a drive is not absolute.
fn root() -> PathBuf {
    std::path::absolute("/logs")
        .unwrap_or_else(|error| panic!("cannot make /logs absolute: {error}"))
}

fn adapter(spec: Spec) -> Declared {
    Declared::new(
        "example".to_owned(),
        root(),
        "jsonl".to_owned(),
        spec,
        Definition {
            file: PathBuf::from("/config/adapters/example.toml"),
            text: String::new(),
        },
    )
}

fn file() -> PathBuf {
    root().join("day").join("one.jsonl")
}

fn response(id: Option<&str>, time: &str, input: u64, output: u64) -> Value {
    let mut line = json!({
        "type": "response",
        "time": time,
        "session": "s1",
        "cwd": "/work",
        "response": {"model": "example-large"},
        "usage": {"input": input, "output": output, "cache_read": 100, "cache_write": 10, "reasoning": 2},
        "text": "not stored",
    });
    if let Some(id) = id {
        line["response"]["id"] = json!(id);
    }
    line
}

fn read_with(adapter: &Declared, lines: &[Value]) -> Vec<Line> {
    let mut reader = adapter.start(&file());
    lines
        .iter()
        .map(|line| reader.parse_line(line.to_string().as_bytes()))
        .collect()
}

fn calls_with(adapter: &Declared, lines: &[Value]) -> Vec<Call> {
    read_with(adapter, lines)
        .into_iter()
        .filter_map(|line| match line {
            Line::Call(call) => Some(call),
            Line::Other | Line::Unreadable => None,
        })
        .collect()
}

#[test]
fn a_call_line_becomes_a_call_with_its_fields() -> Result<(), jiff::Error> {
    let lines = read_with(
        &adapter(spec(Usage::PerCall)),
        &[response(Some("r1"), "2026-09-30T10:00:00Z", 7, 40)],
    );
    assert_eq!(
        lines,
        [Line::Call(Call {
            id: "r1".to_owned(),
            provider: "example-cloud".to_owned(),
            model: "example-large".to_owned(),
            timestamp: "2026-09-30T10:00:00Z".parse()?,
            session_id: "s1".to_owned(),
            cwd: Some("/work".to_owned()),
            role: Role::Headless,
            client_version: None,
            tokens: TokenClasses {
                uncached_input: Tokens(7),
                cache_write: Tokens(10),
                cache_read: Tokens(100),
                output: Tokens(40),
                reasoning: Some(Tokens(2)),
            },
            completion: Completion::FINAL,
            compaction: false,
        })]
    );
    Ok(())
}

#[test]
fn lines_failing_a_condition_are_no_call() {
    let mut request = response(Some("r1"), "2026-09-30T10:00:00Z", 7, 40);
    request["type"] = json!("request");
    let mut untyped = response(Some("r2"), "2026-09-30T10:00:00Z", 7, 40);
    untyped.as_object_mut().map(|line| line.remove("type"));
    assert_eq!(
        read_with(&adapter(spec(Usage::PerCall)), &[request, untyped]),
        [Line::Other, Line::Other]
    );
}

#[test]
fn a_call_line_missing_a_required_field_is_unreadable() {
    let base = response(Some("r1"), "2026-09-30T10:00:00Z", 7, 40);
    let without = |parent: &str, key: &str| {
        let mut line = base.clone();
        if let Some(Value::Object(map)) = line.pointer_mut(parent) {
            map.remove(key);
        }
        line
    };
    let with = |pointer: &str, wrong: Value| {
        let mut line = base.clone();
        if let Some(value) = line.pointer_mut(pointer) {
            *value = wrong;
        }
        line
    };
    let lines = [
        without("", "time"),
        without("", "session"),
        without("/response", "model"),
        without("/response", "id"),
        without("/usage", "output"),
        with("/time", json!("yesterday")),
        with("/usage/output", json!(-1)),
        with("/usage/input", json!("7")),
        with("/cwd", json!(3)),
    ];
    let read = read_with(&adapter(spec(Usage::PerCall)), &lines);
    assert!(
        read.iter().all(|line| *line == Line::Unreadable),
        "{read:?}"
    );
}

#[test]
fn optional_counts_that_are_missing_are_zero_and_a_wrong_one_is_unreadable() {
    let mut missing = response(Some("r1"), "2026-09-30T10:00:00Z", 7, 40);
    missing["usage"] = json!({"input": 7, "output": 40, "cache_read": null});
    let mut wrong = response(Some("r2"), "2026-09-30T10:00:00Z", 7, 40);
    wrong["usage"]["cache_read"] = json!("many");
    let read = read_with(&adapter(spec(Usage::PerCall)), &[missing, wrong]);
    let Some(Line::Call(call)) = read.first() else {
        panic!("expected a call: {read:?}");
    };
    assert_eq!(call.tokens.cache_read, Tokens(0));
    assert_eq!(call.tokens.cache_write, Tokens(0));
    assert_eq!(call.tokens.reasoning, None);
    assert_eq!(read.get(1), Some(&Line::Unreadable));
}

#[test]
fn input_that_includes_the_cache_and_output_without_reasoning_are_converted() {
    let mut spec = spec(Usage::PerCall);
    spec.tokens.input_includes_cache = true;
    spec.tokens.output_includes_reasoning = false;
    let calls = calls_with(
        &adapter(spec),
        &[response(Some("r1"), "2026-09-30T10:00:00Z", 117, 40)],
    );
    let tokens = calls.first().map(|call| call.tokens);
    assert_eq!(
        tokens,
        Some(TokenClasses {
            uncached_input: Tokens(7),
            cache_write: Tokens(10),
            cache_read: Tokens(100),
            output: Tokens(42),
            reasoning: Some(Tokens(2)),
        })
    );
}

#[test]
fn a_call_line_without_tokens_is_no_call() {
    let mut empty = response(Some("r1"), "2026-09-30T10:00:00Z", 0, 0);
    empty["usage"] = json!({"input": 0, "output": 0, "reasoning": 0});
    assert_eq!(
        read_with(&adapter(spec(Usage::PerCall)), &[empty]),
        [Line::Other]
    );
}

#[test]
fn lines_that_are_not_json_are_unreadable() {
    let mut reader = adapter(spec(Usage::PerCall)).start(&file());
    assert_eq!(reader.parse_line(b"{\"type\": "), Line::Unreadable);
    assert_eq!(reader.parse_line(b""), Line::Unreadable);
}

fn prefix(file: &str, position: u64) -> String {
    let start = json!([file, position]).to_string();
    format!("{},", start.trim_end_matches(']'))
}

fn generated(usage: Usage) -> Spec {
    let mut spec = spec(usage);
    spec.id = None;
    spec.session = None;
    spec
}

#[test]
fn without_an_id_field_the_session_is_the_file_and_ids_name_file_and_position() {
    let calls = calls_with(
        &adapter(generated(Usage::PerCall)),
        &[
            response(None, "2026-09-30T10:00:00Z", 7, 40),
            json!({"type": "note"}),
            response(None, "2026-09-30T10:00:00Z", 7, 40),
        ],
    );
    let session = Path::new("day")
        .join("one.jsonl")
        .to_string_lossy()
        .into_owned();
    assert!(
        calls.iter().all(|call| call.session_id == session),
        "{calls:?}"
    );
    let ids: Vec<&str> = calls.iter().map(|call| call.id.as_str()).collect();
    assert_eq!(ids.len(), 2);
    assert!(
        ids.first()
            .is_some_and(|id| id.starts_with(&prefix(&session, 1))),
        "{ids:?}"
    );
    assert!(
        ids.get(1)
            .is_some_and(|id| id.starts_with(&prefix(&session, 3))),
        "{ids:?}"
    );
}

#[test]
fn two_files_sharing_a_session_never_share_a_generated_id() {
    let mut spec = generated(Usage::PerCall);
    spec.session = Some("/session".to_owned());
    let adapter = adapter(spec);
    let line = response(None, "2026-09-30T10:00:00Z", 7, 40);
    let ids = |name: &str| -> Vec<String> {
        let mut reader = adapter.start(&root().join(name));
        match reader.parse_line(line.to_string().as_bytes()) {
            Line::Call(call) => vec![call.id],
            Line::Other | Line::Unreadable => Vec::new(),
        }
    };
    let first = ids("a.jsonl");
    let second = ids("b.jsonl");
    assert_eq!(first.len(), 1);
    assert_ne!(first, second);
}

#[test]
fn a_replaced_file_gets_new_ids_for_new_calls_and_keeps_those_of_the_same_lines() {
    let adapter = adapter(generated(Usage::PerCall));
    let original = calls_with(&adapter, &[response(None, "2026-09-30T10:00:00Z", 7, 40)]);
    let restored = calls_with(&adapter, &[response(None, "2026-09-30T10:00:00Z", 7, 40)]);
    let same_time_other_tokens =
        calls_with(&adapter, &[response(None, "2026-09-30T10:00:00Z", 7, 41)]);
    let mut other_model = response(None, "2026-09-30T10:00:00Z", 7, 40);
    other_model["response"]["model"] = json!("example-small");
    let same_time_other_model = calls_with(&adapter, &[other_model]);
    let id = |calls: &[Call]| calls.first().map(|call| call.id.clone());
    assert_eq!(id(&original), id(&restored));
    assert_ne!(id(&original), id(&same_time_other_tokens));
    assert_ne!(id(&original), id(&same_time_other_model));
}

#[test]
fn generated_ids_keep_facts_apart_whatever_they_contain() {
    let adapter = adapter(generated(Usage::PerCall));
    let mut first = response(None, "2026-09-30T10:00:00Z", 7, 40);
    first["response"]["model"] = json!("q|r");
    let mut second = first.clone();
    second["cwd"] = json!("/work|q");
    second["response"]["model"] = json!("r");
    let mut third = first.clone();
    third["cwd"] = json!("/work");
    third["response"]["model"] = json!("q\",\"r");
    let ids: Vec<Option<String>> = [first, second, third]
        .iter()
        .map(|line| {
            calls_with(&adapter, std::slice::from_ref(line))
                .first()
                .map(|call| call.id.clone())
        })
        .collect();
    assert!(ids.iter().all(Option::is_some), "{ids:?}");
    assert_ne!(ids.first(), ids.get(1));
    assert_ne!(ids.first(), ids.get(2));
    assert_ne!(ids.get(1), ids.get(2));
}

#[test]
fn reasoning_kept_apart_from_output_takes_part_in_a_restart() {
    let mut spec = generated(Usage::Cumulative);
    spec.tokens.output_includes_reasoning = false;
    let mut before = response(None, "2026-09-30T10:00:00Z", 10, 5);
    before["usage"]["reasoning"] = json!(100);
    let mut after = response(None, "2026-09-30T10:00:01Z", 10, 5);
    after["usage"]["reasoning"] = json!(3);
    let outputs: Vec<u64> = calls_with(&adapter(spec), &[before, after])
        .iter()
        .map(|call| call.tokens.output.0)
        .collect();
    assert_eq!(outputs, [105, 8]);
}

#[test]
fn cumulative_totals_count_each_increase_and_restart_when_they_fall() {
    let adapter = adapter(generated(Usage::Cumulative));
    let calls = calls_with(
        &adapter,
        &[
            response(None, "2026-09-30T10:00:00Z", 10, 5),
            response(None, "2026-09-30T10:00:01Z", 10, 5),
            response(None, "2026-09-30T10:00:02Z", 25, 12),
            response(None, "2026-09-30T10:00:03Z", 3, 1),
        ],
    );
    let inputs: Vec<(u64, u64, u64)> = calls
        .iter()
        .map(|call| {
            (
                call.tokens.uncached_input.0,
                call.tokens.output.0,
                call.tokens.cache_read.0,
            )
        })
        .collect();
    // The second line repeats the totals; the fourth starts them again.
    assert_eq!(inputs, [(10, 5, 100), (15, 7, 0), (3, 1, 100)]);
    let session = Path::new("day")
        .join("one.jsonl")
        .to_string_lossy()
        .into_owned();
    let positions: Vec<bool> = calls
        .iter()
        .zip(1..)
        .map(|(call, position)| call.id.starts_with(&prefix(&session, position)))
        .collect();
    assert_eq!(positions, [true, true, true]);
}

#[test]
fn saved_state_that_cannot_be_read_starts_nothing() {
    let adapter = adapter(spec(Usage::Cumulative));
    assert!(adapter.resume(&file(), "not a state").is_none());
}

fn cumulative_lines(steps: &[(u16, u16)]) -> Vec<Value> {
    let mut input = 0;
    let mut output = 0;
    steps
        .iter()
        .zip(0..)
        .map(|(&(more_input, more_output), second)| {
            input += u64::from(more_input);
            output += u64::from(more_output);
            response(
                None,
                &format!("2026-09-30T10:00:{:02}Z", second % 60),
                input,
                output,
            )
        })
        .collect()
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let mut reader = adapter(spec(Usage::Cumulative)).start(&file());
        let _ = reader.parse_line(&bytes);
    }

    #[test]
    fn json_values_of_any_shape_never_panic(value in arbitrary_json()) {
        for usage in [Usage::PerCall, Usage::Cumulative] {
            let mut reader = adapter(spec(usage)).start(&file());
            let _ = reader.parse_line(value.to_string().as_bytes());
        }
    }

    #[test]
    fn cumulative_calls_add_up_to_the_last_totals(
        steps in proptest::collection::vec((0..500_u16, 0..500_u16), 0..20),
    ) {
        let lines = cumulative_lines(&steps);
        let calls = calls_with(&adapter(generated(Usage::Cumulative)), &lines);
        let input: u64 = calls.iter().map(|call| call.tokens.uncached_input.0).sum();
        let output: u64 = calls.iter().map(|call| call.tokens.output.0).sum();
        let expected_input: u64 = steps.iter().map(|step| u64::from(step.0)).sum();
        let expected_output: u64 = steps.iter().map(|step| u64::from(step.1)).sum();
        prop_assert_eq!((input, output), (expected_input, expected_output));
    }

    #[test]
    fn reading_in_two_parts_with_saved_state_equals_reading_at_once(
        steps in proptest::collection::vec((0..500_u16, 0..500_u16), 1..12),
        split in 0..12_usize,
    ) {
        let lines = cumulative_lines(&steps);
        let split = split.min(lines.len());
        let adapter = adapter(generated(Usage::Cumulative));
        let at_once = read_with(&adapter, &lines);
        let mut first = adapter.start(&file());
        let mut parts: Vec<Line> = lines[..split]
            .iter()
            .map(|line| first.parse_line(line.to_string().as_bytes()))
            .collect();
        let saved = first.save().unwrap_or_default();
        let resumed = adapter.resume(&file(), &saved);
        prop_assert!(resumed.is_some());
        if let Some(mut second) = resumed {
            parts.extend(
                lines[split..]
                    .iter()
                    .map(|line| second.parse_line(line.to_string().as_bytes())),
            );
        }
        prop_assert_eq!(parts, at_once);
    }
}

fn arbitrary_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        "[a-z]{0,8}".prop_map(Value::from),
        Just(json!("response")),
        Just(json!("2026-09-30T10:00:00Z")),
    ];
    leaf.prop_recursive(4, 32, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::from),
            proptest::collection::btree_map(
                prop_oneof![
                    Just("type".to_owned()),
                    Just("time".to_owned()),
                    Just("session".to_owned()),
                    Just("usage".to_owned()),
                    Just("input".to_owned()),
                    Just("output".to_owned()),
                    Just("response".to_owned()),
                    Just("model".to_owned()),
                    "[a-z]{1,6}",
                ],
                inner,
                0..6,
            )
            .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}
