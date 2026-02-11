use futures::StreamExt;

use crate::event::{Event, MessageEvent, Role, UsageData, UsageDeltaEvent};
use crate::runner::EventStream;

/// Configuration for the normalization layer — fallback values from the task config.
pub struct NormalizeConfig {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
}

/// Wraps a raw `EventStream` with stateful enrichment so that all consumers
/// (headless, TUI, tests, library users) get uniform events regardless of which
/// agent backend produced them.
pub fn normalize_stream(stream: EventStream, config: NormalizeConfig) -> EventStream {
    let state = NormalizeState {
        session_id: String::new(),
        start_timestamp_ms: 0,
        last_assistant_text: String::new(),
        accumulated_usage: UsageData::default(),
        has_usage: false,
        cwd: config.cwd,
        model: config.model,
        seen_user_message: false,
        seen_usage_delta: false,
        prompt: config.prompt,
    };

    let normalized = stream
        .scan(state, |state, item| {
            let results: Vec<crate::Result<Event>> = match item {
                Ok(event) => state.enrich(event).into_iter().map(Ok).collect(),
                err => vec![err],
            };
            std::future::ready(Some(futures::stream::iter(results)))
        })
        .flatten();

    Box::pin(normalized)
}

struct NormalizeState {
    session_id: String,
    start_timestamp_ms: u64,
    last_assistant_text: String,
    accumulated_usage: UsageData,
    has_usage: bool,
    cwd: Option<String>,
    model: Option<String>,
    seen_user_message: bool,
    seen_usage_delta: bool,
    prompt: Option<String>,
}

impl NormalizeState {
    fn accumulate_usage(&mut self, usage: &UsageData) {
        self.has_usage = true;
        if let Some(v) = usage.input_tokens {
            *self.accumulated_usage.input_tokens.get_or_insert(0) += v;
        }
        if let Some(v) = usage.output_tokens {
            *self.accumulated_usage.output_tokens.get_or_insert(0) += v;
        }
        if let Some(v) = usage.cache_read_tokens {
            *self.accumulated_usage.cache_read_tokens.get_or_insert(0) += v;
        }
        if let Some(v) = usage.cache_creation_tokens {
            *self.accumulated_usage.cache_creation_tokens.get_or_insert(0) += v;
        }
        if let Some(v) = usage.cost_usd {
            *self.accumulated_usage.cost_usd.get_or_insert(0.0) += v;
        }
    }

    /// Synthesize a user message event with the stored prompt.
    fn make_user_message(&self, timestamp_ms: u64) -> Event {
        Event::Message(MessageEvent {
            role: Role::User,
            text: self.prompt.clone().unwrap_or_default(),
            usage: None,
            timestamp_ms,
        })
    }

    /// Maybe prepend a synthetic user message before the given event.
    /// Returns the event(s) to emit.
    fn maybe_prepend_user_message(&mut self, event: Event, timestamp_ms: u64) -> Vec<Event> {
        if !self.seen_user_message && self.prompt.is_some() {
            self.seen_user_message = true;
            vec![self.make_user_message(timestamp_ms), event]
        } else {
            vec![event]
        }
    }

    fn enrich(&mut self, event: Event) -> Vec<Event> {
        match event {
            Event::SessionStart(mut e) => {
                self.session_id = e.session_id.clone();
                self.start_timestamp_ms = e.timestamp_ms;

                if e.model.is_none() {
                    e.model = self.model.clone();
                }
                if e.cwd.is_none() {
                    e.cwd = self.cwd.clone();
                }

                // SessionStart itself is never preceded by a user message —
                // the user message goes after it.
                vec![Event::SessionStart(e)]
            }
            Event::Message(ref e) if e.role == Role::User => {
                self.seen_user_message = true;
                vec![event]
            }
            Event::Message(ref e) if e.role == Role::Assistant && !e.text.is_empty() => {
                self.last_assistant_text = e.text.clone();
                let ts = e.timestamp_ms;
                self.maybe_prepend_user_message(event, ts)
            }
            Event::UsageDelta(ref e) => {
                self.seen_usage_delta = true;
                self.accumulate_usage(&e.usage);
                let ts = e.timestamp_ms;
                self.maybe_prepend_user_message(event, ts)
            }
            Event::Result(mut e) => {
                // Fill text from last assistant message if empty.
                if e.text.is_empty() && !self.last_assistant_text.is_empty() {
                    e.text = self.last_assistant_text.clone();
                }
                // Fill session_id if empty.
                if e.session_id.is_empty() && !self.session_id.is_empty() {
                    e.session_id = self.session_id.clone();
                }
                // Compute duration from timestamps if not set.
                if e.duration_ms.is_none() && self.start_timestamp_ms > 0 && e.timestamp_ms > 0 {
                    e.duration_ms = Some(e.timestamp_ms.saturating_sub(self.start_timestamp_ms));
                }
                // Fill usage from accumulated deltas if not set.
                if e.usage.is_none() && self.has_usage {
                    e.usage = Some(self.accumulated_usage.clone());
                }
                // Fill total_cost_usd from accumulated usage cost if not set.
                if e.total_cost_usd.is_none() {
                    if let Some(ref usage) = e.usage {
                        if let Some(cost) = usage.cost_usd {
                            e.total_cost_usd = Some(cost);
                        }
                    }
                }

                let ts = e.timestamp_ms;
                let result_event = Event::Result(e);

                // Maybe prepend user message.
                let mut events = self.maybe_prepend_user_message(result_event, ts);

                // Synthesize UsageDelta before Result if none was seen.
                if !self.seen_usage_delta {
                    // Extract usage from the Result event (it's the last in events).
                    if let Some(Event::Result(ref r)) = events.last() {
                        if let Some(ref usage) = r.usage {
                            let synthetic_usage = Event::UsageDelta(UsageDeltaEvent {
                                usage: usage.clone(),
                                timestamp_ms: ts,
                            });
                            // Insert before the last element (the Result).
                            let Some(result_ev) = events.pop() else {
                                return events;
                            };
                            events.push(synthetic_usage);
                            events.push(result_ev);
                        }
                    }
                }

                events
            }
            other => {
                let ts = match &other {
                    Event::TextDelta(e) => e.timestamp_ms,
                    Event::ToolStart(e) => e.timestamp_ms,
                    Event::ToolEnd(e) => e.timestamp_ms,
                    Event::Error(e) => e.timestamp_ms,
                    _ => 0,
                };
                self.maybe_prepend_user_message(other, ts)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::*;
    use futures::StreamExt;

    fn make_stream(events: Vec<Event>) -> EventStream {
        let iter = events.into_iter().map(Ok);
        Box::pin(futures::stream::iter(iter))
    }

    #[tokio::test]
    async fn session_start_fills_missing_model_and_cwd() {
        let events = vec![Event::SessionStart(SessionStartEvent {
            session_id: "s1".into(),
            agent: "codex".into(),
            model: None,
            cwd: None,
            timestamp_ms: 1000,
        })];

        let config = NormalizeConfig {
            cwd: Some("/home/user".into()),
            model: Some("gpt-5-codex".into()),
            prompt: None,
        };

        let mut stream = normalize_stream(make_stream(events), config);
        let event = stream.next().await.unwrap().unwrap();

        if let Event::SessionStart(e) = event {
            assert_eq!(e.model, Some("gpt-5-codex".into()));
            assert_eq!(e.cwd, Some("/home/user".into()));
        } else {
            panic!("expected SessionStart");
        }
    }

    #[tokio::test]
    async fn session_start_preserves_existing_model_and_cwd() {
        let events = vec![Event::SessionStart(SessionStartEvent {
            session_id: "s1".into(),
            agent: "claude".into(),
            model: Some("claude-opus-4-6".into()),
            cwd: Some("/original".into()),
            timestamp_ms: 1000,
        })];

        let config = NormalizeConfig {
            cwd: Some("/fallback".into()),
            model: Some("fallback-model".into()),
            prompt: None,
        };

        let mut stream = normalize_stream(make_stream(events), config);
        let event = stream.next().await.unwrap().unwrap();

        if let Event::SessionStart(e) = event {
            assert_eq!(e.model, Some("claude-opus-4-6".into()));
            assert_eq!(e.cwd, Some("/original".into()));
        } else {
            panic!("expected SessionStart");
        }
    }

    #[tokio::test]
    async fn result_text_filled_from_last_assistant_message() {
        let events = vec![
            Event::SessionStart(SessionStartEvent {
                session_id: "s1".into(),
                agent: "codex".into(),
                model: None,
                cwd: None,
                timestamp_ms: 1000,
            }),
            Event::Message(MessageEvent {
                role: Role::Assistant,
                text: "Hello from codex!".into(),
                usage: None,
                timestamp_ms: 1500,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: String::new(),
                session_id: String::new(),
                duration_ms: None,
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 2000,
            }),
        ];

        // No prompt → no synthetic user message, indices unchanged.
        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        if let Event::Result(ref r) = collected[2] {
            assert_eq!(r.text, "Hello from codex!");
            assert_eq!(r.session_id, "s1");
            assert_eq!(r.duration_ms, Some(1000));
        } else {
            panic!("expected Result");
        }
    }

    #[tokio::test]
    async fn result_duration_computed_from_timestamps() {
        let events = vec![
            Event::SessionStart(SessionStartEvent {
                session_id: "s1".into(),
                agent: "opencode".into(),
                model: None,
                cwd: None,
                timestamp_ms: 5000,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: "done".into(),
                session_id: "s1".into(),
                duration_ms: None,
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 8000,
            }),
        ];

        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        if let Event::Result(ref r) = collected[1] {
            assert_eq!(r.duration_ms, Some(3000));
        } else {
            panic!("expected Result");
        }
    }

    #[tokio::test]
    async fn result_preserves_existing_duration() {
        let events = vec![
            Event::SessionStart(SessionStartEvent {
                session_id: "s1".into(),
                agent: "claude".into(),
                model: None,
                cwd: None,
                timestamp_ms: 1000,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: "done".into(),
                session_id: "s1".into(),
                duration_ms: Some(999),
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 5000,
            }),
        ];

        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        if let Event::Result(ref r) = collected[1] {
            assert_eq!(r.duration_ms, Some(999));
        } else {
            panic!("expected Result");
        }
    }

    #[tokio::test]
    async fn result_usage_filled_from_accumulated_deltas() {
        let events = vec![
            Event::SessionStart(SessionStartEvent {
                session_id: "s1".into(),
                agent: "codex".into(),
                model: None,
                cwd: None,
                timestamp_ms: 1000,
            }),
            Event::UsageDelta(UsageDeltaEvent {
                usage: UsageData {
                    input_tokens: Some(100),
                    output_tokens: Some(50),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: Some(0.01),
                },
                timestamp_ms: 1500,
            }),
            Event::UsageDelta(UsageDeltaEvent {
                usage: UsageData {
                    input_tokens: Some(200),
                    output_tokens: Some(75),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: Some(0.02),
                },
                timestamp_ms: 1800,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: "done".into(),
                session_id: "s1".into(),
                duration_ms: None,
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 2000,
            }),
        ];

        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        if let Event::Result(ref r) = collected[3] {
            let usage = r.usage.as_ref().unwrap();
            assert_eq!(usage.input_tokens, Some(300));
            assert_eq!(usage.output_tokens, Some(125));
            assert!((usage.cost_usd.unwrap() - 0.03).abs() < 1e-10);
            // total_cost_usd should be filled from accumulated cost.
            assert!((r.total_cost_usd.unwrap() - 0.03).abs() < 1e-10);
        } else {
            panic!("expected Result");
        }
    }

    #[tokio::test]
    async fn result_preserves_existing_usage() {
        let existing_usage = UsageData {
            input_tokens: Some(999),
            output_tokens: Some(888),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: Some(0.99),
        };

        let events = vec![
            Event::UsageDelta(UsageDeltaEvent {
                usage: UsageData {
                    input_tokens: Some(100),
                    output_tokens: Some(50),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: None,
                },
                timestamp_ms: 1500,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: "done".into(),
                session_id: "s1".into(),
                duration_ms: Some(500),
                total_cost_usd: None,
                usage: Some(existing_usage.clone()),
                timestamp_ms: 2000,
            }),
        ];

        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        if let Event::Result(ref r) = collected[1] {
            assert_eq!(r.usage, Some(existing_usage));
        } else {
            panic!("expected Result");
        }
    }

    #[tokio::test]
    async fn passthrough_events_unchanged() {
        let events = vec![
            Event::TextDelta(TextDeltaEvent {
                text: "hello".into(),
                timestamp_ms: 1000,
            }),
            Event::ToolStart(ToolStartEvent {
                call_id: "c1".into(),
                tool_name: "read".into(),
                input: None,
                timestamp_ms: 1100,
            }),
            Event::ToolEnd(ToolEndEvent {
                call_id: "c1".into(),
                tool_name: "read".into(),
                success: true,
                output: Some("content".into()),
                usage: None,
                timestamp_ms: 1200,
            }),
            Event::Error(ErrorEvent {
                message: "oops".into(),
                code: None,
                timestamp_ms: 1300,
            }),
        ];

        let expected = events.clone();
        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        assert_eq!(collected, expected);
    }

    #[tokio::test]
    async fn errors_pass_through_stream() {
        let events: Vec<crate::Result<Event>> = vec![
            Ok(Event::TextDelta(TextDeltaEvent {
                text: "hi".into(),
                timestamp_ms: 1000,
            })),
            Err(crate::Error::Other("test error".into())),
        ];

        let raw: EventStream = Box::pin(futures::stream::iter(events));
        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let mut stream = normalize_stream(raw, config);

        let first = stream.next().await.unwrap();
        assert!(first.is_ok());

        let second = stream.next().await.unwrap();
        assert!(second.is_err());
    }

    // ─── New round-2 tests ────────────────────────────────────────

    #[tokio::test]
    async fn user_message_synthesized_after_session_start() {
        let events = vec![
            Event::SessionStart(SessionStartEvent {
                session_id: "s1".into(),
                agent: "codex".into(),
                model: None,
                cwd: None,
                timestamp_ms: 1000,
            }),
            Event::Message(MessageEvent {
                role: Role::Assistant,
                text: "Hello!".into(),
                usage: None,
                timestamp_ms: 1500,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: "Hello!".into(),
                session_id: "s1".into(),
                duration_ms: Some(500),
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 2000,
            }),
        ];

        let config = NormalizeConfig {
            cwd: None,
            model: None,
            prompt: Some("say hello".into()),
        };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        // SessionStart, Message(user), Message(assistant), Result
        assert_eq!(collected.len(), 4, "events: {collected:?}");
        assert!(matches!(&collected[0], Event::SessionStart(_)));
        if let Event::Message(ref m) = collected[1] {
            assert_eq!(m.role, Role::User);
            assert_eq!(m.text, "say hello");
            assert_eq!(m.timestamp_ms, 1500);
        } else {
            panic!("expected synthetic user Message at [1], got {:?}", collected[1]);
        }
        assert!(matches!(&collected[2], Event::Message(m) if m.role == Role::Assistant));
        assert!(matches!(&collected[3], Event::Result(_)));
    }

    #[tokio::test]
    async fn user_message_not_duplicated_when_adapter_sends_one() {
        let events = vec![
            Event::SessionStart(SessionStartEvent {
                session_id: "s1".into(),
                agent: "cursor".into(),
                model: None,
                cwd: None,
                timestamp_ms: 1000,
            }),
            Event::Message(MessageEvent {
                role: Role::User,
                text: "say hello".into(),
                usage: None,
                timestamp_ms: 1200,
            }),
            Event::Message(MessageEvent {
                role: Role::Assistant,
                text: "Hello!".into(),
                usage: None,
                timestamp_ms: 1500,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: "Hello!".into(),
                session_id: "s1".into(),
                duration_ms: Some(500),
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 2000,
            }),
        ];

        let config = NormalizeConfig {
            cwd: None,
            model: None,
            prompt: Some("say hello".into()),
        };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        // Should NOT inject a second user message.
        let user_messages: Vec<_> = collected
            .iter()
            .filter(|e| matches!(e, Event::Message(m) if m.role == Role::User))
            .collect();
        assert_eq!(user_messages.len(), 1, "expected exactly 1 user message, got {user_messages:?}");
    }

    #[tokio::test]
    async fn user_message_not_injected_without_prompt() {
        let events = vec![
            Event::SessionStart(SessionStartEvent {
                session_id: "s1".into(),
                agent: "codex".into(),
                model: None,
                cwd: None,
                timestamp_ms: 1000,
            }),
            Event::Message(MessageEvent {
                role: Role::Assistant,
                text: "Hello!".into(),
                usage: None,
                timestamp_ms: 1500,
            }),
        ];

        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        // No prompt → no user message injected.
        assert_eq!(collected.len(), 2);
        assert!(matches!(&collected[0], Event::SessionStart(_)));
        assert!(matches!(&collected[1], Event::Message(m) if m.role == Role::Assistant));
    }

    #[tokio::test]
    async fn total_cost_filled_from_accumulated_usage() {
        let events = vec![
            Event::UsageDelta(UsageDeltaEvent {
                usage: UsageData {
                    input_tokens: Some(100),
                    output_tokens: Some(50),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: Some(0.05),
                },
                timestamp_ms: 1000,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: "done".into(),
                session_id: "s1".into(),
                duration_ms: Some(500),
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 2000,
            }),
        ];

        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        if let Event::Result(ref r) = collected[1] {
            assert!((r.total_cost_usd.unwrap() - 0.05).abs() < 1e-10);
        } else {
            panic!("expected Result");
        }
    }

    #[tokio::test]
    async fn usage_delta_synthesized_before_result() {
        // Stream has no UsageDelta events, but Result has usage data.
        let events = vec![
            Event::SessionStart(SessionStartEvent {
                session_id: "s1".into(),
                agent: "claude".into(),
                model: None,
                cwd: None,
                timestamp_ms: 1000,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: "done".into(),
                session_id: "s1".into(),
                duration_ms: Some(1000),
                total_cost_usd: Some(0.01),
                usage: Some(UsageData {
                    input_tokens: Some(200),
                    output_tokens: Some(100),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: Some(0.01),
                }),
                timestamp_ms: 2000,
            }),
        ];

        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        // SessionStart, synthetic UsageDelta, Result
        assert_eq!(collected.len(), 3, "events: {collected:?}");
        assert!(matches!(&collected[0], Event::SessionStart(_)));
        if let Event::UsageDelta(ref u) = collected[1] {
            assert_eq!(u.usage.input_tokens, Some(200));
            assert_eq!(u.usage.output_tokens, Some(100));
        } else {
            panic!("expected synthetic UsageDelta at [1], got {:?}", collected[1]);
        }
        assert!(matches!(&collected[2], Event::Result(_)));
    }

    #[tokio::test]
    async fn no_synthetic_usage_delta_when_already_present() {
        let events = vec![
            Event::SessionStart(SessionStartEvent {
                session_id: "s1".into(),
                agent: "codex".into(),
                model: None,
                cwd: None,
                timestamp_ms: 1000,
            }),
            Event::UsageDelta(UsageDeltaEvent {
                usage: UsageData {
                    input_tokens: Some(100),
                    output_tokens: Some(50),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: None,
                },
                timestamp_ms: 1500,
            }),
            Event::Result(ResultEvent {
                success: true,
                text: "done".into(),
                session_id: "s1".into(),
                duration_ms: Some(1000),
                total_cost_usd: None,
                usage: None,
                timestamp_ms: 2000,
            }),
        ];

        let config = NormalizeConfig { cwd: None, model: None, prompt: None };
        let stream = normalize_stream(make_stream(events), config);
        let collected: Vec<Event> = stream.map(|r| r.unwrap()).collect().await;

        // Should be exactly: SessionStart, UsageDelta, Result — no extra UsageDelta.
        let usage_deltas: Vec<_> = collected
            .iter()
            .filter(|e| matches!(e, Event::UsageDelta(_)))
            .collect();
        assert_eq!(usage_deltas.len(), 1, "expected exactly 1 UsageDelta, got {usage_deltas:?}");
    }
}
