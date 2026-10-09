use proptest::prelude::*;
use serde_json::json;

use super::*;

fn parse(line: &serde_json::Value) -> Line {
    reader().parse_line(line.to_string().as_bytes())
}

#[test]
fn a_record_becomes_a_call() -> Result<(), jiff::Error> {
    let record = json!({
        "id": "r1",
        "timestamp": "2026-09-30T10:00:00Z",
        "provider": "nebius",
        "model": "moonshotai/Kimi-K3",
        "session_id": "s1",
        "cwd": "/work",
        "role": "subagent",
        "tokens": {"uncached_input": 7, "cache_write": 10, "cache_read": 100, "output": 40, "reasoning": 2},
        "added_later": true,
    });
    assert_eq!(
        parse(&record),
        Line::Call(Call {
            id: "r1".to_owned(),
            provider: "nebius".to_owned(),
            model: "moonshotai/Kimi-K3".to_owned(),
            timestamp: "2026-09-30T10:00:00Z".parse()?,
            session_id: "s1".to_owned(),
            cwd: Some("/work".to_owned()),
            role: Role::Subagent,
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
        })
    );
    Ok(())
}

#[test]
fn optional_fields_default_to_an_interactive_call_without_cache_or_directory() {
    let record = json!({
        "id": "r1", "timestamp": "2026-09-30T10:00:00Z", "provider": "p", "model": "m",
        "session_id": "s1", "tokens": {"uncached_input": 7, "output": 40},
    });
    let Line::Call(call) = parse(&record) else {
        panic!("expected a call");
    };
    assert_eq!(call.role, Role::Interactive);
    assert_eq!(call.cwd, None);
    assert_eq!(call.tokens.cache_read, Tokens(0));
    assert_eq!(call.tokens.reasoning, None);
}

#[test]
fn a_record_without_tokens_is_no_call() {
    let record = json!({
        "id": "r1", "timestamp": "2026-09-30T10:00:00Z", "provider": "p", "model": "m",
        "session_id": "s1", "tokens": {"uncached_input": 0, "output": 0},
    });
    assert_eq!(parse(&record), Line::Other);
}

#[test]
fn records_that_cannot_be_read_are_unreadable() {
    let complete = json!({
        "id": "r1", "timestamp": "2026-09-30T10:00:00Z", "provider": "p", "model": "m",
        "session_id": "s1", "tokens": {"uncached_input": 7, "output": 40},
    });
    let mut unreadable = Vec::new();
    for field in [
        "id",
        "timestamp",
        "provider",
        "model",
        "session_id",
        "tokens",
    ] {
        let mut record = complete.clone();
        record.as_object_mut().map(|record| record.remove(field));
        unreadable.push(record);
    }
    let mut review = complete.clone();
    review["role"] = json!("review");
    unreadable.push(review);
    let mut negative = complete;
    negative["tokens"]["output"] = json!(-1);
    unreadable.push(negative);
    for record in &unreadable {
        assert_eq!(parse(record), Line::Unreadable, "{record}");
    }
    assert_eq!(reader().parse_line(b"not json"), Line::Unreadable);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = reader().parse_line(&bytes);
    }
}
