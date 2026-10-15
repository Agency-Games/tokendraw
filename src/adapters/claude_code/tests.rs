use proptest::prelude::*;
use serde_json::json;

use super::*;

fn adapter() -> ClaudeCode {
    ClaudeCode::new(PathBuf::from("/logs/projects"))
}

fn session_file() -> PathBuf {
    PathBuf::from("/logs/projects/-work-alpha/session.jsonl")
}

fn assistant(model: &str, entrypoint: &str) -> serde_json::Value {
    json!({
        "type": "assistant",
        "sessionId": "session-a",
        "timestamp": "2026-09-30T12:00:00.250Z",
        "cwd": "/work/alpha",
        "entrypoint": entrypoint,
        "version": "2.1.0",
        "someNewField": {"nested": [1, 2, 3]},
        "message": {
            "id": "msg_1",
            "model": model,
            "content": [{"type": "text", "text": "not stored"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 3,
                "cache_creation_input_tokens": 1000,
                "cache_read_input_tokens": 20000,
                "output_tokens": 40,
                "output_tokens_details": {"thinking_tokens": 10}
            }
        }
    })
}

fn parse_with(adapter: &ClaudeCode, file: &Path, line: &[u8]) -> Line {
    adapter.start(file).parse_line(line)
}

fn parse(value: &serde_json::Value, file: &Path) -> Line {
    parse_with(&adapter(), file, value.to_string().as_bytes())
}

#[test]
fn an_assistant_message_becomes_a_call_with_its_token_classes() -> Result<(), jiff::Error> {
    let Line::Call(call) = parse(&assistant("claude-opus-5-5", "cli"), &session_file()) else {
        panic!("expected a call");
    };
    assert_eq!(
        call,
        Call {
            id: "msg_1".to_owned(),
            provider: "anthropic".to_owned(),
            model: "claude-opus-5-5".to_owned(),
            timestamp: "2026-09-30T12:00:00.250Z".parse()?,
            session_id: "session-a".to_owned(),
            agent_id: None,
            cwd: Some("/work/alpha".to_owned()),
            role: Role::Interactive,
            client_version: Some("2.1.0".to_owned()),
            tokens: TokenClasses {
                uncached_input: Tokens(3),
                cache_write: Tokens(1000),
                cache_read: Tokens(20000),
                output: Tokens(40),
                reasoning: Some(Tokens(10)),
            },
            completion: Completion::Final {
                stop_reason: Some("end_turn".to_owned()),
            },
            compaction: false,
        }
    );
    Ok(())
}

#[test]
fn a_line_without_a_version_has_no_client_version() {
    let mut line = assistant("claude-opus-5-5", "cli");
    line.as_object_mut().map(|line| line.remove("version"));
    let Line::Call(call) = parse(&line, &session_file()) else {
        panic!("expected a call");
    };
    assert_eq!(call.client_version, None);
}

#[test]
fn a_line_without_a_stop_reason_carries_partial_usage() {
    for stop_reason in [None, Some(serde_json::Value::Null)] {
        let mut line = assistant("claude-opus-5-5", "cli");
        let Some(message) = line["message"].as_object_mut() else {
            panic!("expected an object");
        };
        message.remove("stop_reason");
        if let Some(value) = stop_reason {
            message.insert("stop_reason".to_owned(), value);
        }
        let Line::Call(call) = parse(&line, &session_file()) else {
            panic!("expected a call");
        };
        assert_eq!(call.completion, Completion::Partial);
    }
}

#[test]
fn reasoning_is_unknown_when_output_details_are_null() {
    let mut line = assistant("claude-opus-5-5", "cli");
    line["message"]["usage"]["output_tokens_details"] = serde_json::Value::Null;
    let Line::Call(call) = parse(&line, &session_file()) else {
        panic!("expected a call");
    };
    assert_eq!(call.tokens.reasoning, None);
}

#[test]
fn print_mode_and_sdk_sessions_are_headless() {
    for entrypoint in ["sdk-cli", "sdk-py", "sdk-ts"] {
        let Line::Call(call) = parse(&assistant("claude-opus-5-5", entrypoint), &session_file())
        else {
            panic!("expected a call");
        };
        assert_eq!(call.role, Role::Headless, "{entrypoint}");
    }
}

#[test]
fn transcripts_in_a_subagents_directory_are_subagent_calls_with_their_agent() {
    let file = PathBuf::from("/logs/projects/-work-alpha/session/subagents/agent-1.jsonl");
    let mut line = assistant("claude-opus-5-5", "cli");
    line["agentId"] = "a1".into();
    let Line::Call(call) = parse(&line, &file) else {
        panic!("expected a call");
    };
    assert_eq!(call.role, Role::Subagent);
    assert_eq!(call.agent_id.as_deref(), Some("a1"));
}

#[test]
fn an_agent_id_outside_a_subagent_transcript_is_not_kept() {
    let mut line = assistant("claude-opus-5-5", "cli");
    line["agentId"] = "a1".into();
    let Line::Call(call) = parse(&line, &session_file()) else {
        panic!("expected a call");
    };
    assert_eq!(call.agent_id, None);
}

#[test]
fn a_subagents_directory_above_the_projects_directory_does_not_make_subagents() {
    let adapter = ClaudeCode::new(PathBuf::from("/subagents/projects"));
    let file = PathBuf::from("/subagents/projects/-work/session.jsonl");
    let line = assistant("claude-opus-5-5", "cli").to_string();
    let Line::Call(call) = parse_with(&adapter, &file, line.as_bytes()) else {
        panic!("expected a call");
    };
    assert_eq!(call.role, Role::Interactive);
}

#[test]
fn synthetic_messages_are_not_calls() {
    assert_eq!(
        parse(&assistant(SYNTHETIC_MODEL, "cli"), &session_file()),
        Line::Other
    );
}

#[test]
fn other_line_types_record_no_usage() {
    let line = json!({"type": "user", "message": {"role": "user", "content": "hello"}});
    assert_eq!(parse(&line, &session_file()), Line::Other);
}

#[test]
fn a_line_that_is_not_json_is_unreadable() {
    assert_eq!(
        parse_with(
            &adapter(),
            &session_file(),
            b"{\"type\":\"assistant\",\"mess"
        ),
        Line::Unreadable
    );
}

#[test]
fn an_assistant_message_without_usage_is_unreadable() {
    let mut line = assistant("claude-opus-5-5", "cli");
    if let Some(message) = line["message"].as_object_mut() {
        message.remove("usage");
    }
    assert_eq!(parse(&line, &session_file()), Line::Unreadable);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = parse_with(&adapter(), &session_file(), &bytes);
    }

    #[test]
    fn any_usage_values_are_read_exactly(
        input in any::<u64>(),
        write in any::<u64>(),
        read in any::<u64>(),
        output in any::<u64>(),
        thinking in proptest::option::of(any::<u64>()),
    ) {
        let mut line = assistant("claude-opus-5-5", "cli");
        line["message"]["usage"] = json!({
            "input_tokens": input,
            "cache_creation_input_tokens": write,
            "cache_read_input_tokens": read,
            "output_tokens": output,
            "output_tokens_details": {"thinking_tokens": thinking},
        });
        let Line::Call(call) = parse(&line, &session_file()) else {
            panic!("expected a call");
        };
        prop_assert_eq!(call.tokens, TokenClasses {
            uncached_input: Tokens(input),
            cache_write: Tokens(write),
            cache_read: Tokens(read),
            output: Tokens(output),
            reasoning: thinking.map(Tokens),
        });
    }

    #[test]
    fn json_values_of_any_shape_never_panic(value in arbitrary_json()) {
        let _ = parse(&value, &session_file());
        let mut line = assistant("claude-opus-5-5", "cli");
        line["message"]["usage"]["output_tokens_details"] = value;
        let _ = parse(&line, &session_file());
    }
}

fn arbitrary_json() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::from),
        any::<i64>().prop_map(serde_json::Value::from),
        ".{0,8}".prop_map(serde_json::Value::from),
    ];
    leaf.prop_recursive(3, 32, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(serde_json::Value::from),
            proptest::collection::btree_map(
                prop_oneof![
                    Just("type".to_owned()),
                    Just("message".to_owned()),
                    ".{0,6}"
                ],
                inner,
                0..4
            )
            .prop_map(|map| serde_json::Value::Object(map.into_iter().collect())),
        ]
    })
}
