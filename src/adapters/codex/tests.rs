use std::fs;

use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;

const SESSION: &str = "019f0000-0000-7000-8000-000000000001";

fn adapter() -> Codex {
    Codex::new(PathBuf::from("/logs/sessions"))
}

fn rollout_file() -> PathBuf {
    PathBuf::from(format!(
        "/logs/sessions/2026/09/30/rollout-2026-09-30T12-00-00-{SESSION}.jsonl"
    ))
}

fn line(timestamp: &str, kind: &str, payload: Value) -> Value {
    let mut entry = json!({"timestamp": timestamp, "type": kind});
    entry["payload"] = payload;
    entry
}

fn header(source: Value, thread_source: &str) -> Value {
    let mut meta = json!({
        "id": SESSION,
        "timestamp": "2026-09-30T12:00:00.000Z",
        "cwd": "/work/alpha",
        "originator": "codex_exec",
        "cli_version": "0.200.0",
        "thread_source": thread_source,
        "model_provider": "openai",
        "base_instructions": {"text": "not stored"},
    });
    meta["source"] = source;
    line("2026-09-30T12:00:00.000Z", "session_meta", meta)
}

fn exec_header() -> Value {
    header(json!("exec"), "user")
}

fn turn(model: &str, cwd: &str) -> Value {
    line(
        "2026-09-30T12:00:01.000Z",
        "turn_context",
        json!({"turn_id": "t1", "model": model, "cwd": cwd, "effort": "high"}),
    )
}

/// Usage as Codex writes it; the total is input plus output.
fn usage(input: u64, cached: u64, output: u64, reasoning: u64) -> Value {
    json!({
        "input_tokens": input,
        "cached_input_tokens": cached,
        "cache_write_input_tokens": 0,
        "output_tokens": output,
        "reasoning_output_tokens": reasoning,
        "total_tokens": input + output,
    })
}

fn record(timestamp: &str, response_id: &str, usage: Value) -> Value {
    let mut payload = json!({
        "response_id": response_id,
        "thread_id": SESSION,
        "turn_id": "t1",
        "turn_token_usage": usage,
        "thread_token_usage": usage,
    });
    payload["usage"] = usage;
    line(timestamp, "token_usage_record", payload)
}

fn token_count(timestamp: &str, total: Value) -> Value {
    let mut payload = json!({
        "type": "token_count",
        "info": {"last_token_usage": total},
        "rate_limits": null,
    });
    payload["info"]["total_token_usage"] = total;
    line(timestamp, "event_msg", payload)
}

fn task_started(started_at: i64) -> Value {
    line(
        "2026-09-30T12:00:00.000Z",
        "event_msg",
        json!({"type": "task_started", "turn_id": "t1", "started_at": started_at}),
    )
}

fn message() -> Value {
    line(
        "2026-09-30T12:00:01.000Z",
        "response_item",
        json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "token_count"}]}),
    )
}

fn read(lines: &[Value]) -> Vec<Line> {
    let mut reader = adapter().start(&rollout_file());
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

fn ids(calls: &[Call]) -> Vec<&str> {
    calls.iter().map(|call| call.id.as_str()).collect()
}

fn outputs(calls: &[Call]) -> Vec<u64> {
    calls.iter().map(|call| call.tokens.output.0).collect()
}

#[test]
fn a_token_usage_record_becomes_a_call_with_its_token_classes() -> Result<(), jiff::Error> {
    let mut used = usage(30_000, 25_000, 400, 150);
    used["cache_write_input_tokens"] = json!(1000);
    let found = calls(&[
        exec_header(),
        turn("gpt-6-astra", "/work/alpha/sub"),
        record("2026-09-30T12:00:05.250Z", "resp_1", used),
    ]);
    assert_eq!(
        found,
        [Call {
            id: "resp_1".to_owned(),
            provider: "openai".to_owned(),
            model: "gpt-6-astra".to_owned(),
            timestamp: "2026-09-30T12:00:05.250Z".parse()?,
            session_id: SESSION.to_owned(),
            cwd: Some("/work/alpha/sub".to_owned()),
            role: Role::Headless,
            client_version: Some("0.200.0".to_owned()),
            tokens: TokenClasses {
                uncached_input: Tokens(4000),
                cache_write: Tokens(1000),
                cache_read: Tokens(25_000),
                output: Tokens(400),
                reasoning: Some(Tokens(150)),
            },
            completion: Completion::FINAL,
            compaction: false,
        }]
    );
    Ok(())
}

#[test]
fn the_token_count_after_a_record_adds_no_second_call() {
    let found = calls(&[
        exec_header(),
        turn("gpt-6-astra", "/work"),
        record("2026-09-30T12:00:05Z", "resp_1", usage(100, 0, 10, 0)),
        token_count("2026-09-30T12:00:05Z", usage(100, 0, 10, 0)),
        record("2026-09-30T12:00:09Z", "resp_2", usage(150, 100, 20, 0)),
        token_count("2026-09-30T12:00:09Z", usage(250, 100, 30, 0)),
    ]);
    assert_eq!(ids(&found), ["resp_1", "resp_2"]);
    assert_eq!(outputs(&found), [10, 20]);
}

#[test]
fn without_records_each_increase_of_the_totals_is_a_call() {
    let found = calls(&[
        exec_header(),
        turn("gpt-6-astra", "/work"),
        token_count("2026-09-30T12:00:05Z", usage(100, 0, 10, 4)),
        token_count("2026-09-30T12:00:09Z", usage(250, 80, 30, 9)),
    ]);
    assert_eq!(
        ids(&found),
        [format!("{SESSION}#1"), format!("{SESSION}#2")]
    );
    assert_eq!(
        found[1].tokens,
        TokenClasses {
            uncached_input: Tokens(70),
            cache_write: Tokens(0),
            cache_read: Tokens(80),
            output: Tokens(20),
            reasoning: Some(Tokens(5)),
        }
    );
}

#[test]
fn a_repeated_total_is_no_call() {
    let found = calls(&[
        exec_header(),
        turn("gpt-6-astra", "/work"),
        token_count("2026-09-30T12:00:05Z", usage(100, 0, 10, 0)),
        token_count("2026-09-30T12:00:06Z", usage(100, 0, 10, 0)),
        token_count("2026-09-30T12:00:09Z", usage(150, 0, 15, 0)),
    ]);
    assert_eq!(
        ids(&found),
        [format!("{SESSION}#1"), format!("{SESSION}#2")]
    );
    assert_eq!(outputs(&found), [10, 5]);
}

#[test]
fn a_falling_total_is_a_restart_whose_new_totals_are_the_call() {
    let found = calls(&[
        exec_header(),
        turn("gpt-6-astra", "/work"),
        token_count("2026-09-30T12:00:05Z", usage(1000, 0, 100, 0)),
        token_count("2026-09-30T12:00:09Z", usage(300, 0, 7, 0)),
        token_count("2026-09-30T12:00:12Z", usage(400, 0, 9, 0)),
    ]);
    assert_eq!(outputs(&found), [100, 7, 2]);
    assert_eq!(found[1].tokens.uncached_input, Tokens(300));
}

#[test]
fn a_record_whose_usage_never_reaches_the_totals_is_still_one_call() {
    let found = calls(&[
        exec_header(),
        turn("gpt-6-astra", "/work"),
        record("2026-09-30T12:00:05Z", "resp_1", usage(100, 0, 10, 0)),
        token_count("2026-09-30T12:00:05Z", usage(100, 0, 10, 0)),
        record("2026-09-30T12:00:07Z", "resp_2", usage(500, 400, 50, 0)),
        token_count("2026-09-30T12:00:07Z", usage(100, 0, 10, 0)),
        record("2026-09-30T12:00:09Z", "resp_3", usage(120, 100, 5, 0)),
        token_count("2026-09-30T12:00:09Z", usage(220, 100, 15, 0)),
    ]);
    assert_eq!(ids(&found), ["resp_1", "resp_2", "resp_3"]);
    assert_eq!(outputs(&found), [10, 50, 5]);
}

#[test]
fn a_file_that_starts_recording_midway_counts_each_response_once() {
    let found = calls(&[
        exec_header(),
        turn("gpt-6-astra", "/work"),
        token_count("2026-09-03T12:00:05Z", usage(100, 0, 10, 0)),
        token_count("2026-09-03T12:00:09Z", usage(250, 100, 30, 0)),
        record("2026-09-04T12:00:05Z", "resp_1", usage(300, 250, 5, 0)),
        token_count("2026-09-04T12:00:05Z", usage(550, 350, 35, 0)),
    ]);
    assert_eq!(
        ids(&found),
        [
            format!("{SESSION}#1"),
            format!("{SESSION}#2"),
            "resp_1".to_owned()
        ]
    );
    assert_eq!(outputs(&found), [10, 20, 5]);
}

#[test]
fn a_token_count_without_info_records_no_usage() {
    let mut empty = token_count("2026-09-30T12:00:05Z", usage(0, 0, 0, 0));
    empty["payload"]["info"] = Value::Null;
    let found = read(&[exec_header(), turn("gpt-6-astra", "/work"), empty]);
    assert_eq!(found, [Line::Other, Line::Other, Line::Other]);
}

#[test]
fn a_record_before_an_event_without_info_still_covers_the_next_increase() {
    let mut empty = token_count("2026-09-30T12:00:05Z", usage(0, 0, 0, 0));
    empty["payload"]["info"] = Value::Null;
    let found = calls(&[
        exec_header(),
        turn("gpt-6-astra", "/work"),
        record("2026-09-30T12:00:05Z", "resp_1", usage(100, 0, 10, 0)),
        empty,
        token_count("2026-09-30T12:00:05Z", usage(100, 0, 10, 0)),
    ]);
    assert_eq!(ids(&found), ["resp_1"]);
}

/// An older fork: its own header, its parent's header, then the parent's
/// history with the parent's token events, all stamped when the fork was
/// created.
fn fork_with_copied_history(own_turn_started_at: i64) -> Vec<Value> {
    let mut own = header(json!({"subagent": {"thread_spawn": {}}}), "subagent");
    own["payload"]["forked_from_id"] = json!("019f0000-0000-7000-8000-0000000000aa");
    let mut parent = exec_header();
    parent["payload"]["id"] = json!("019f0000-0000-7000-8000-0000000000aa");
    parent["payload"]["timestamp"] = json!("2026-09-30T11:00:00.000Z");
    // 2026-09-30T11:00:00Z and 12:00:00Z, the fork's creation.
    let parent_turn = 1_790_766_000;
    vec![
        own,
        parent,
        task_started(parent_turn),
        turn("gpt-6-astra", "/work"),
        record("2026-09-30T12:00:00Z", "resp_parent", usage(100, 0, 10, 0)),
        token_count("2026-09-30T12:00:00Z", usage(100, 0, 10, 0)),
        token_count("2026-09-30T12:00:00Z", usage(300, 0, 30, 0)),
        task_started(own_turn_started_at),
        turn("gpt-6-sol", "/work/fork"),
        token_count("2026-09-30T12:00:20Z", usage(600, 250, 37, 0)),
    ]
}

#[test]
fn a_forks_copied_history_is_its_parents_and_yields_no_calls() {
    let created = 1_790_769_600;
    let found = calls(&fork_with_copied_history(created + 5));
    assert_eq!(ids(&found), [format!("{SESSION}#3")]);
    assert_eq!(found[0].role, Role::Subagent);
    assert_eq!(found[0].model, "gpt-6-sol");
    assert_eq!(found[0].tokens.uncached_input, Tokens(50));
    assert_eq!(found[0].tokens.cache_read, Tokens(250));
    assert_eq!(found[0].tokens.output, Tokens(7));
}

#[test]
fn a_fork_whose_own_turn_never_starts_yields_no_calls() {
    let created = 1_790_769_600;
    assert_eq!(calls(&fork_with_copied_history(created - 1)), []);
}

fn compacted(timestamp: &str, response_id: Option<&str>) -> Value {
    let mut payload = json!({"message": "not stored", "replacement_history": []});
    if let Some(id) = response_id {
        payload["compaction_response_id"] = json!(id);
    }
    line(timestamp, "compacted", payload)
}

fn unrecorded(context_input: u64) -> Completion {
    Completion::Unrecorded {
        context_input: Tokens(context_input),
    }
}

#[test]
fn a_compaction_without_a_record_is_a_call_without_tokens_after_the_previous_calls_input() {
    let found = calls(&[
        exec_header(),
        turn("gpt-5.6-sol", "/work"),
        token_count("2026-09-03T12:00:05Z", usage(1000, 800, 10, 0)),
        compacted("2026-09-03T12:00:06Z", None),
        token_count("2026-09-03T12:00:09Z", usage(1300, 800, 30, 0)),
    ]);
    assert_eq!(
        ids(&found),
        [
            format!("{SESSION}#1"),
            format!("{SESSION}#compaction-1@2026-09-03T12:00:06Z"),
            format!("{SESSION}#2"),
        ]
    );
    let compaction = &found[1];
    assert_eq!(compaction.tokens, TokenClasses::default());
    assert_eq!(compaction.completion, unrecorded(1000));
    assert!(compaction.compaction);
    assert_eq!(compaction.model, "gpt-5.6-sol");
    assert!(!found[0].compaction && !found[2].compaction);
}

#[test]
fn a_compaction_naming_its_record_marks_that_records_call() {
    let found = calls(&[
        exec_header(),
        turn("gpt-6-astra", "/work"),
        record("2026-09-30T12:00:05Z", "resp_1", usage(500, 400, 50, 0)),
        compacted("2026-09-30T12:00:05Z", Some("resp_1")),
        compacted("2026-09-30T12:00:06Z", Some("resp_unknown")),
    ]);
    assert_eq!(ids(&found), ["resp_1", "resp_1"]);
    assert!(!found[0].compaction);
    assert!(found[1].compaction);
    assert_eq!(found[1].tokens, found[0].tokens);
    assert_eq!(found[1].timestamp, found[0].timestamp);
    assert_eq!(found[1].completion, Completion::FINAL);
}

#[test]
fn compactions_without_a_record_at_the_same_time_are_distinct_calls() {
    let found = calls(&[
        exec_header(),
        turn("gpt-5.5", "/work"),
        compacted("2026-07-01T12:00:00Z", None),
        compacted("2026-07-01T12:00:00Z", None),
    ]);
    assert_eq!(
        ids(&found),
        [
            format!("{SESSION}#compaction-1@2026-07-01T12:00:00Z"),
            format!("{SESSION}#compaction-2@2026-07-01T12:00:00Z"),
        ]
    );
    assert_eq!(found[0].completion, unrecorded(0));
}

#[test]
fn a_forks_copied_compaction_is_its_parents_and_its_own_follows_the_copied_input() {
    let created = 1_790_769_600;
    let mut lines = fork_with_copied_history(created + 5);
    lines.insert(7, compacted("2026-09-30T12:00:00Z", None));
    lines.insert(10, compacted("2026-09-30T12:00:10Z", None));
    // A copied compaction before the parent's first turn context.
    lines.insert(2, compacted("2026-09-30T12:00:00Z", None));
    assert!(!read(&lines).contains(&Line::Unreadable));
    let found = calls(&lines);
    assert_eq!(
        ids(&found),
        [
            format!("{SESSION}#compaction-3@2026-09-30T12:00:10Z"),
            format!("{SESSION}#3"),
        ]
    );
    assert_eq!(found[0].completion, unrecorded(200));
    assert_eq!(found[0].model, "gpt-6-sol");
}

#[test]
fn saved_state_from_before_compactions_were_read_resumes() -> Result<(), serde_json::Error> {
    let mut reader = adapter().start(&rollout_file());
    for line in [
        exec_header(),
        turn("gpt-5.5", "/work"),
        token_count("2026-07-01T12:00:00Z", usage(100, 0, 10, 0)),
    ] {
        reader.parse_line(line.to_string().as_bytes());
    }
    let mut saved: Value = serde_json::from_str(&reader.save().unwrap_or_default())?;
    for key in ["latest_record", "latest_input", "unrecorded_compactions"] {
        saved.as_object_mut().map(|state| state.remove(key));
    }
    let resumed = adapter().resume(&rollout_file(), &saved.to_string());
    let line = resumed.map(|mut reader| {
        reader.parse_line(
            compacted("2026-07-01T12:00:01Z", None)
                .to_string()
                .as_bytes(),
        )
    });
    let Some(Line::Call(call)) = line else {
        panic!("expected a call, got {line:?}");
    };
    assert_eq!(
        call.id,
        format!("{SESSION}#compaction-1@2026-07-01T12:00:01Z")
    );
    assert_eq!(call.completion, unrecorded(0));
    Ok(())
}

fn saved_after(lines: &[Value]) -> String {
    let mut reader = adapter().start(&rollout_file());
    for line in lines {
        reader.parse_line(line.to_string().as_bytes());
    }
    reader.save().unwrap_or_default()
}

/// The version of the call a reader resumed from `saved` reads from a
/// record.
fn version_after(saved: &str) -> Option<String> {
    let mut reader = adapter().resume(&rollout_file(), saved)?;
    let line = reader.parse_line(
        record("2026-09-30T12:00:05.000Z", "resp_1", usage(10, 0, 1, 0))
            .to_string()
            .as_bytes(),
    );
    match line {
        Line::Call(call) => call.client_version,
        other => panic!("expected a call, got {other:?}"),
    }
}

#[test]
fn a_file_continued_after_its_header_keeps_the_version_of_codex() {
    let saved = saved_after(&[exec_header(), turn("gpt-6-astra", "/work")]);
    assert_eq!(version_after(&saved).as_deref(), Some("0.200.0"));
}

#[test]
fn saved_state_from_before_versions_were_read_resumes_without_one() -> Result<(), serde_json::Error>
{
    let mut saved: Value =
        serde_json::from_str(&saved_after(&[exec_header(), turn("gpt-6-astra", "/work")]))?;
    saved["session"]
        .as_object_mut()
        .map(|session| session.remove("version"));
    assert_eq!(version_after(&saved.to_string()), None);
    Ok(())
}

#[test]
fn roles_come_from_the_session_header() {
    let cases = [
        (header(json!("exec"), "user"), Role::Headless),
        (header(json!("cli"), "user"), Role::Interactive),
        (header(json!("vscode"), "user"), Role::Interactive),
        (
            header(json!({"subagent": {"other": "guardian"}}), "subagent"),
            Role::Subagent,
        ),
    ];
    for (header, role) in cases {
        let found = calls(&[
            header,
            turn("gpt-6-astra", "/work"),
            record("2026-09-30T12:00:05Z", "resp_1", usage(100, 0, 10, 0)),
        ]);
        assert_eq!(found[0].role, role);
    }
}

#[test]
fn model_and_directory_come_from_the_latest_turn_context() {
    let mut no_cwd = turn("gpt-6-luna", "/unused");
    if let Some(payload) = no_cwd["payload"].as_object_mut() {
        payload.remove("cwd");
    }
    let found = calls(&[
        exec_header(),
        record("2026-09-30T12:00:01Z", "resp_0", usage(1, 0, 1, 0)),
        turn("gpt-6-astra", "/work/one"),
        record("2026-09-30T12:00:05Z", "resp_1", usage(100, 0, 10, 0)),
        no_cwd,
        record("2026-09-30T12:00:09Z", "resp_2", usage(100, 0, 10, 0)),
    ]);
    let seen: Vec<_> = found
        .iter()
        .map(|call| (call.model.as_str(), call.cwd.as_deref()))
        .collect();
    assert_eq!(
        seen,
        [
            ("gpt-6-astra", Some("/work/one")),
            ("gpt-6-luna", Some("/work/one"))
        ]
    );
}

#[test]
fn usage_before_the_header_or_a_turn_context_is_unreadable() {
    let used = record("2026-09-30T12:00:05Z", "resp_1", usage(100, 0, 10, 0));
    assert_eq!(read(std::slice::from_ref(&used)), [Line::Unreadable]);
    assert_eq!(
        read(&[exec_header(), used]),
        [Line::Other, Line::Unreadable]
    );
}

#[test]
fn conversation_lines_and_unknown_types_record_no_usage() {
    let found = read(&[
        exec_header(),
        message(),
        line("2026-09-30T12:00:01Z", "world_state", json!("anything")),
        line(
            "2026-09-30T12:00:01Z",
            "event_msg",
            json!({"type": "item_completed", "info": [1, 2]}),
        ),
    ]);
    assert_eq!(found, vec![Line::Other; 4]);
}

#[test]
fn lines_that_cannot_be_read_are_unreadable() {
    let mut fractional = token_count("2026-09-30T12:00:05Z", usage(100, 0, 10, 0));
    fractional["payload"]["info"]["total_token_usage"]["input_tokens"] = json!(1.5);
    let mut reader = adapter().start(&rollout_file());
    assert_eq!(
        reader.parse_line(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\""),
        Line::Unreadable
    );
    assert_eq!(
        reader.parse_line(b"{\"type\":\"event_msg\",\"payload\":\"token_count\"}"),
        Line::Unreadable
    );
    assert_eq!(
        read(&[exec_header(), turn("gpt-6-astra", "/w"), fractional])[2],
        Line::Unreadable
    );
}

#[test]
fn saved_state_that_cannot_be_read_starts_nothing() {
    assert!(adapter().resume(&rollout_file(), "{\"session\":").is_none());
}

#[test]
fn only_rollout_files_are_discovered() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let day = root.path().join("2026/09/30");
    fs::create_dir_all(&day)?;
    fs::write(day.join("rollout-2026-09-30T12-00-00-a.jsonl"), "")?;
    fs::write(day.join("history.jsonl"), "")?;
    let found = Codex::new(root.path().to_path_buf()).discover().files;
    assert_eq!(found, [day.join("rollout-2026-09-30T12-00-00-a.jsonl")]);
    Ok(())
}

fn growing_rollout(steps: &[(u16, u16, u16)]) -> (Vec<Value>, (u64, u64, u64)) {
    let mut lines = vec![exec_header(), turn("gpt-6-astra", "/work")];
    let (mut input, mut cached, mut output) = (0_u64, 0_u64, 0_u64);
    for &(new_input, new_cached, new_output) in steps {
        input += u64::from(new_input) + u64::from(new_cached);
        cached += u64::from(new_cached);
        output += u64::from(new_output);
        lines.push(message());
        lines.push(token_count(
            "2026-09-30T12:00:05Z",
            usage(input, cached, output, 0),
        ));
    }
    (lines, (input, cached, output))
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let mut reader = adapter().start(&rollout_file());
        let _ = reader.parse_line(&bytes);
    }

    #[test]
    fn json_values_of_any_shape_never_panic(value in arbitrary_json()) {
        let mut reader = adapter().start(&rollout_file());
        let _ = reader.parse_line(value.to_string().as_bytes());
        for kind in ["session_meta", "turn_context", "token_usage_record", "event_msg"] {
            let _ = reader.parse_line(line("2026-09-30T12:00:00Z", kind, value.clone()).to_string().as_bytes());
        }
        let mut count = token_count("2026-09-30T12:00:05Z", usage(1, 0, 1, 0));
        count["payload"]["info"]["total_token_usage"] = value;
        let _ = reader.parse_line(count.to_string().as_bytes());
    }

    #[test]
    fn the_calls_of_growing_totals_add_up_to_the_final_totals(
        steps in proptest::collection::vec((0..500_u16, 0..500_u16, 0..50_u16), 0..20),
    ) {
        let (lines, (input, cached, output)) = growing_rollout(&steps);
        let found = calls(&lines);
        let sum = |class: fn(&TokenClasses) -> Tokens| -> u64 {
            found.iter().map(|call| class(&call.tokens).0).sum()
        };
        prop_assert_eq!(sum(|tokens| tokens.uncached_input), input - cached);
        prop_assert_eq!(sum(|tokens| tokens.cache_read), cached);
        prop_assert_eq!(sum(|tokens| tokens.output), output);
        let nonzero = steps.iter().filter(|(a, b, c)| a + b + c > 0).count();
        prop_assert_eq!(found.len(), nonzero);
    }

    #[test]
    fn reading_in_two_parts_with_saved_state_equals_reading_at_once(
        steps in proptest::collection::vec((0..500_u16, 0..500_u16, 0..50_u16), 1..12),
        split in any::<prop::sample::Index>(),
        fork in any::<bool>(),
    ) {
        let lines = if fork {
            let mut lines = fork_with_copied_history(1_790_769_605);
            lines.extend(growing_rollout(&steps).0.into_iter().skip(2));
            lines
        } else {
            growing_rollout(&steps).0
        };
        let at = split.index(lines.len() + 1);
        let whole = read(&lines);
        let mut first = adapter().start(&rollout_file());
        let mut parts: Vec<Line> = lines[..at]
            .iter()
            .map(|line| first.parse_line(line.to_string().as_bytes()))
            .collect();
        let saved = first.save().unwrap_or_default();
        let Some(mut second) = adapter().resume(&rollout_file(), &saved) else {
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
                    Just("payload".to_owned()),
                    Just("info".to_owned()),
                    Just("total_tokens".to_owned()),
                    ".{0,6}"
                ],
                inner,
                0..4
            )
            .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}
