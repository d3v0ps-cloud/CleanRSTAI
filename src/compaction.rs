//! Context compaction: when a chat's next request would fill most of the model's context, the
//! older messages are summarised by a model and only the summary and the recent messages are
//! sent from then on. The chat keeps every message; a marker message holds the summary.

use anyhow::{Result, bail};
use serde_json::json;

use crate::config::{CompactionSettings, ProviderConfig};
use crate::llm::{self, ToolSpec};
use crate::types::{ChatMessage, Role};

/// Tokens an image is counted as (its base64 data says little about what the model sees).
const IMAGE_TOKENS: u64 = 1000;
/// Most characters of one message, and of one tool result, put into the text to summarise.
const MAX_MESSAGE_CHARS: usize = 8000;
const MAX_TOOL_CHARS: usize = 2000;

/// What compaction needs for one request.
#[derive(Clone, Debug)]
pub struct Compaction {
    pub settings: CompactionSettings,
    /// The provider and model that write the summary.
    pub provider: ProviderConfig,
    /// Compact on its own when the context fills up (the chat's switch is on).
    pub auto: bool,
}

/// A rough token count for a request: from the server's own count for the latest reply when
/// `anchor` allows it (no summary has replaced older messages since), plus about four
/// characters a token for everything after it.
pub fn estimate_tokens(
    system: &str,
    history: &[ChatMessage],
    tools: &[ToolSpec],
    anchor: bool,
) -> u64 {
    let chars = |m: &ChatMessage| -> (u64, u64) {
        let mut text = m.content.len()
            + m.tool_calls
                .iter()
                .map(|c| c.name.len() + c.arguments.len())
                .sum::<usize>();
        let mut images = 0;
        for a in &m.attachments {
            if a.is_image() {
                images += 1;
            } else {
                text += a.text.as_ref().map_or(0, String::len) + a.name.len() + 20;
            }
        }
        (text as u64, images)
    };
    let from_chars = |ms: &[ChatMessage]| -> u64 {
        ms.iter()
            .map(|m| {
                let (text, images) = chars(m);
                text / 4 + images * IMAGE_TOKENS + 4
            })
            .sum()
    };
    let tool_chars: usize = tools
        .iter()
        .map(|t| t.name.len() + t.description.len() + t.parameters.to_string().len())
        .sum();
    let fixed = (system.len() + tool_chars) as u64 / 4;
    let reported = anchor
        .then(|| {
            history.iter().enumerate().rev().find_map(|(i, m)| {
                let s = m.stats.as_ref()?;
                Some((i, s.prompt_tokens? + s.completion_tokens.unwrap_or(0)))
            })
        })
        .flatten();
    match reported {
        Some((i, tokens)) => tokens + from_chars(&history[i + 1..]),
        None => fixed + from_chars(history),
    }
}

/// Where to split `history` so the last `keep_recent` messages (moved back so tool results
/// stay with the call that asked for them) are kept. `None` when nothing would be summarised.
pub fn split_point(history: &[ChatMessage], keep_recent: usize) -> Option<usize> {
    let mut at = history.len().checked_sub(keep_recent.max(1))?;
    while at > 0 && history[at].role == Role::Tool {
        at -= 1;
    }
    (at > 0).then_some(at)
}

/// The messages before the split as plain text for the summarising model, after the previous
/// summary if there is one. Long messages and tool results are shortened.
pub fn transcript(previous: Option<&str>, old: &[ChatMessage]) -> String {
    let mut out = String::new();
    if let Some(p) = previous {
        out.push_str("<earlier_summary>\n");
        out.push_str(p.trim());
        out.push_str("\n</earlier_summary>\n\n");
    }
    for m in old {
        match m.role {
            Role::System => continue,
            Role::User => {
                out.push_str("User: ");
                out.push_str(&clip(
                    &crate::attach::text_with_files(&m.content, &m.attachments),
                    MAX_MESSAGE_CHARS,
                ));
            }
            Role::Assistant => {
                out.push_str("Assistant: ");
                out.push_str(&clip(&m.content, MAX_MESSAGE_CHARS));
                for c in &m.tool_calls {
                    out.push_str(&format!(
                        "\n[called {} with {}]",
                        c.name,
                        clip(&c.arguments, 1000)
                    ));
                }
            }
            Role::Tool => {
                out.push_str(&format!(
                    "[result of {}{}]: ",
                    m.tool_name.as_deref().unwrap_or("a tool"),
                    if m.is_error { ", an error" } else { "" }
                ));
                out.push_str(&clip(&m.content, MAX_TOOL_CHARS));
            }
        }
        out.push_str("\n\n");
    }
    out
}

/// Ask `c.provider` for a summary of `old` (and the `previous` summary).
pub async fn summarise(
    http: &reqwest::Client,
    c: &Compaction,
    previous: Option<&str>,
    old: &[ChatMessage],
) -> Result<String> {
    let messages = vec![
        json!({"role": "system", "content": c.settings.prompt}),
        json!({"role": "user", "content": format!(
            "<conversation>\n{}</conversation>\n\nWrite the summary now.",
            transcript(previous, old)
        )}),
    ];
    let turn = llm::chat_completion(http, &c.provider, messages, &[], |_| {}, |_| {}).await?;
    let summary = turn.content.trim();
    if summary.is_empty() {
        bail!("the model returned an empty summary");
    }
    Ok(summary.to_owned())
}

/// The latest task list among `old`, as a checklist, when `kept` has none of its own: the
/// summary carries it on so the model keeps working through it.
pub fn carried_tasks(old: &[ChatMessage], kept: &[ChatMessage]) -> Option<String> {
    let latest = |ms: &[ChatMessage]| {
        ms.iter()
            .flat_map(|m| &m.tool_calls)
            .rfind(|c| c.name == crate::tools::TASKS)
            .and_then(|c| crate::tools::parse_tasks(&c.arguments).ok())
    };
    if latest(kept).is_some() {
        return None;
    }
    let tasks = latest(old)?;
    let lines: Vec<String> = tasks
        .iter()
        .map(|t| {
            let mark = match t.status {
                crate::tools::TaskStatus::Done => "x",
                crate::tools::TaskStatus::InProgress => "~",
                crate::tools::TaskStatus::Pending => " ",
            };
            format!("- [{mark}] {}", t.title.trim())
        })
        .collect();
    Some(format!(
        "Task list at this point ([x] done, [~] in progress):\n{}",
        lines.join("\n")
    ))
}

/// The system prompt with the summary of the earlier conversation added.
pub fn system_with_summary(system: &str, summary: &str) -> String {
    format!(
        "{}\n\nThe earlier part of this conversation was summarised to save space:\n\
         <summary_of_earlier_conversation>\n{}\n</summary_of_earlier_conversation>",
        system.trim_end(),
        summary.trim()
    )
}

/// Put in front of the kept messages when they don't start with the user's (some chat
/// templates require the first message to be the user's).
pub const CONTINUE_NOTE: &str = "(The earlier part of this conversation is summarised in the \
system prompt. Continue from where it left off.)";

/// Shorten the results of tool calls older than the last `keep_recent` messages to `chars`
/// characters.
pub fn trim_tool_results(history: &mut [ChatMessage], keep_recent: usize, chars: usize) {
    let end = history.len().saturating_sub(keep_recent);
    for m in history[..end].iter_mut().filter(|m| m.role == Role::Tool) {
        let n = m.content.chars().count();
        if n > chars {
            m.content = format!(
                "{}\n[… {} more characters of this older tool result were left out]",
                m.content.chars().take(chars).collect::<String>(),
                n - chars
            );
        }
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [{} more characters]", &s[..end], s.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCall;

    fn turn() -> Vec<ChatMessage> {
        let call = ToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"a"}"#.into(),
        };
        let mut a = ChatMessage::new(Role::Assistant, "");
        a.tool_calls = vec![call.clone()];
        vec![
            ChatMessage::new(Role::User, "fix it"),
            a,
            ChatMessage::tool_result(&call, "x".repeat(5000), false),
            ChatMessage::new(Role::Assistant, "done"),
        ]
    }

    #[test]
    fn splits_before_the_call_that_asked_for_a_kept_result() {
        let h = turn();
        // Keeping 2 would start at the tool result; it moves back to its call.
        assert_eq!(split_point(&h, 2), Some(1));
        assert_eq!(split_point(&h, 1), Some(3));
        assert_eq!(split_point(&h, 4), None);
        assert_eq!(split_point(&h, 10), None);
    }

    #[test]
    fn estimates_from_the_server_count_or_characters() {
        let mut h = turn();
        let by_chars = estimate_tokens("sys", &h, &[], true);
        assert!((1250..1400).contains(&by_chars), "{by_chars}");
        h[1].stats = Some(crate::stats::RequestStats {
            prompt_tokens: Some(9000),
            completion_tokens: Some(100),
            ..Default::default()
        });
        let anchored = estimate_tokens("sys", &h, &[], true);
        assert!((10_300..10_400).contains(&anchored), "{anchored}");
        // After a compaction the server's count covered messages no longer sent.
        assert_eq!(estimate_tokens("sys", &h, &[], false), by_chars);
    }

    #[test]
    fn transcript_and_trimming_shorten_long_tool_output() {
        let h = turn();
        let t = transcript(Some("earlier"), &h);
        assert!(
            t.starts_with("<earlier_summary>\nearlier\n</earlier_summary>"),
            "{t}"
        );
        assert!(t.contains("User: fix it"), "{t}");
        assert!(
            t.contains("[called read_file with {\"path\":\"a\"}]"),
            "{t}"
        );
        assert!(t.contains("[3000 more characters]"), "{t}");

        let mut h = turn();
        trim_tool_results(&mut h, 1, 100);
        assert!(h[2].content.starts_with(&"x".repeat(100)));
        assert!(h[2].content.contains("4900 more characters"));
        let mut h = turn();
        trim_tool_results(&mut h, 2, 100);
        assert_eq!(h[2].content.len(), 5000, "recent results stay whole");
    }

    #[test]
    fn carries_the_latest_task_list_on() {
        let tasks = |list: &str| {
            let mut m = ChatMessage::new(Role::Assistant, "");
            m.tool_calls = vec![ToolCall {
                id: "t".into(),
                name: crate::tools::TASKS.into(),
                arguments: list.into(),
            }];
            m
        };
        let old = vec![
            tasks(r#"{"tasks": [{"title": "a", "status": "pending"}]}"#),
            tasks(
                r#"{"tasks": [{"title": "a", "status": "done"}, {"title": "b", "status": "in_progress"}]}"#,
            ),
        ];
        assert_eq!(
            carried_tasks(&old, &turn()).unwrap(),
            "Task list at this point ([x] done, [~] in progress):\n- [x] a\n- [~] b"
        );
        assert_eq!(
            carried_tasks(&old, &old[1..]),
            None,
            "the kept part has its own"
        );
        assert_eq!(carried_tasks(&turn(), &[]), None);
    }

    #[tokio::test]
    async fn summarises_with_the_given_provider() {
        let (base, log) = crate::testutil::serve(|_| {
            crate::testutil::Response::json(json!({
                "choices": [{"message": {"role": "assistant", "content": " - user wants a fix\n"}, "finish_reason": "stop"}]
            }))
        })
        .await;
        let c = Compaction {
            settings: CompactionSettings::default(),
            provider: ProviderConfig {
                base_url: format!("{base}/v1"),
                model: "summary-model".into(),
                stream: false,
                ..Default::default()
            },
            auto: true,
        };
        let out = summarise(&reqwest::Client::new(), &c, None, &turn())
            .await
            .unwrap();
        assert_eq!(out, "- user wants a fix");
        let body = log.lock().unwrap()[0].json();
        assert_eq!(body["model"], "summary-model");
        assert!(body.get("tools").is_none() || body["tools"].as_array().unwrap().is_empty());
        assert!(
            body["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("User: fix it")
        );
        assert!(
            system_with_summary("Be nice.", "notes")
                .contains("<summary_of_earlier_conversation>\nnotes\n")
        );
    }
}
