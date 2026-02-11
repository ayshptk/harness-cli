use harness::event::*;

/// Verify that events round-trip through JSON correctly.
#[test]
fn session_start_round_trip() {
    let event = Event::SessionStart(SessionStartEvent {
        session_id: "s-1".into(),
        agent: "claude".into(),
        model: Some("opus".into()),
        cwd: Some("/tmp".into()),
        timestamp_ms: 0,
    });
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn text_delta_round_trip() {
    let event = Event::TextDelta(TextDeltaEvent {
        text: "Hello, world!".into(),
        timestamp_ms: 0,
    });
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn message_round_trip() {
    let event = Event::Message(MessageEvent {
        role: Role::Assistant,
        text: "I found the bug in line 42.".into(),
        usage: None,
        timestamp_ms: 0,
    });
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn tool_start_round_trip() {
    let event = Event::ToolStart(ToolStartEvent {
        call_id: "c-1".into(),
        tool_name: "bash".into(),
        input: Some(serde_json::json!({"command": "ls -la"})),
        timestamp_ms: 0,
    });
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn tool_end_round_trip() {
    let event = Event::ToolEnd(ToolEndEvent {
        call_id: "c-1".into(),
        tool_name: "bash".into(),
        success: true,
        output: Some("file.txt\nREADME.md".into()),
        usage: None,
        timestamp_ms: 0,
    });
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn result_success_round_trip() {
    let event = Event::Result(ResultEvent {
        success: true,
        text: "Done!".into(),
        session_id: "s-1".into(),
        duration_ms: Some(1234),
        total_cost_usd: Some(0.03),
        usage: None,
        timestamp_ms: 0,
    });
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn result_error_round_trip() {
    let event = Event::Result(ResultEvent {
        success: false,
        text: String::new(),
        session_id: "s-1".into(),
        duration_ms: None,
        total_cost_usd: None,
        usage: None,
        timestamp_ms: 0,
    });
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn error_event_round_trip() {
    let event = Event::Error(ErrorEvent {
        message: "rate limit".into(),
        code: Some("429".into()),
        timestamp_ms: 0,
    });
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, parsed);
}

#[test]
fn optional_fields_omitted_in_json() {
    let event = Event::SessionStart(SessionStartEvent {
        session_id: "s-1".into(),
        agent: "codex".into(),
        model: None,
        cwd: None,
        timestamp_ms: 0,
    });
    let json = serde_json::to_string(&event).unwrap();
    // model and cwd should not appear in the JSON.
    assert!(!json.contains("model"));
    assert!(!json.contains("cwd"));
}

#[test]
fn event_display_trait() {
    let events = vec![
        Event::SessionStart(SessionStartEvent {
            session_id: "s-1".into(),
            agent: "claude".into(),
            model: None,
            cwd: None,
            timestamp_ms: 0,
        }),
        Event::TextDelta(TextDeltaEvent {
            text: "hi".into(),
            timestamp_ms: 0,
        }),
        Event::Message(MessageEvent {
            role: Role::Assistant,
            text: "hello".into(),
            usage: None,
            timestamp_ms: 0,
        }),
        Event::ToolStart(ToolStartEvent {
            call_id: "c-1".into(),
            tool_name: "bash".into(),
            input: None,
            timestamp_ms: 0,
        }),
        Event::ToolEnd(ToolEndEvent {
            call_id: "c-1".into(),
            tool_name: "bash".into(),
            success: true,
            output: None,
            usage: None,
            timestamp_ms: 0,
        }),
        Event::Result(ResultEvent {
            success: true,
            text: "ok".into(),
            session_id: "s-1".into(),
            duration_ms: None,
            total_cost_usd: None,
            usage: None,
            timestamp_ms: 0,
        }),
        Event::Error(ErrorEvent {
            message: "oops".into(),
            code: None,
            timestamp_ms: 0,
        }),
    ];

    for event in &events {
        let display = format!("{event}");
        assert!(!display.is_empty(), "Display output was empty for {event:?}");
    }
}

/// Test that the JSON tag serialization uses snake_case.
#[test]
fn json_type_tag_is_snake_case() {
    let cases: Vec<(Event, &str)> = vec![
        (
            Event::SessionStart(SessionStartEvent {
                session_id: "s".into(),
                agent: "a".into(),
                model: None,
                cwd: None,
                timestamp_ms: 0,
            }),
            "session_start",
        ),
        (
            Event::TextDelta(TextDeltaEvent {
                text: "x".into(),
                timestamp_ms: 0,
            }),
            "text_delta",
        ),
        (
            Event::Message(MessageEvent {
                role: Role::Assistant,
                text: "x".into(),
                usage: None,
                timestamp_ms: 0,
            }),
            "message",
        ),
        (
            Event::ToolStart(ToolStartEvent {
                call_id: "c".into(),
                tool_name: "t".into(),
                input: None,
                timestamp_ms: 0,
            }),
            "tool_start",
        ),
        (
            Event::ToolEnd(ToolEndEvent {
                call_id: "c".into(),
                tool_name: "t".into(),
                success: true,
                output: None,
                usage: None,
                timestamp_ms: 0,
            }),
            "tool_end",
        ),
        (
            Event::Result(ResultEvent {
                success: true,
                text: "x".into(),
                session_id: "s".into(),
                duration_ms: None,
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 0,
            }),
            "result",
        ),
        (
            Event::Error(ErrorEvent {
                message: "x".into(),
                code: None,
                timestamp_ms: 0,
            }),
            "error",
        ),
    ];

    for (event, expected_type) in cases {
        let json = serde_json::to_string(&event).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let actual_type = value.get("type").unwrap().as_str().unwrap();
        assert_eq!(
            actual_type, expected_type,
            "Event {event:?} serialized type tag as `{actual_type}`, expected `{expected_type}`"
        );
    }
}

/// Test backward compatibility: JSON without timestamp_ms deserializes correctly.
#[test]
fn deserialize_without_timestamp_ms() {
    let json = r#"{"type":"text_delta","text":"hello"}"#;
    let event: Event = serde_json::from_str(json).unwrap();
    match event {
        Event::TextDelta(d) => {
            assert_eq!(d.text, "hello");
            assert_eq!(d.timestamp_ms, 0);
        }
        other => panic!("expected TextDelta, got {other:?}"),
    }
}

/// Test that timestamp_ms round-trips correctly.
#[test]
fn timestamp_ms_round_trip() {
    let event = Event::TextDelta(TextDeltaEvent {
        text: "hi".into(),
        timestamp_ms: 1234567890123,
    });
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(event, parsed);
}
