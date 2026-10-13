//! Context compaction: when a run's prompt grows past a share of the
//! model's context window, the older part of the conversation is replaced
//! by a summary the same model writes, and the run goes on with the
//! summary plus the most recent messages. The system prompt and the task
//! always stay.

use pulse_provider::{Message, Request, Role};

/// Percent of the context window a prompt may use before compaction
/// (the `harness.compact_at` setting; 0 turns compaction off).
pub const DEFAULT_COMPACT_AT: i32 = 70;

/// The most recent messages compaction never touches.
pub const COMPACT_KEEP_RECENT: usize = 6;

/// Model calls between two compactions (or after one that failed).
pub const COMPACT_MIN_GAP_CALLS: i32 = 3;

/// The first line of the message that stands in for the compacted part.
pub const COMPACT_MARKER: &str = "[Earlier conversation compacted]";

/// Characters of one message kept in the transcript the summarizer reads.
pub const COMPACT_MESSAGE_LIMIT: usize = 3000;

/// Characters of the whole transcript the summarizer reads.
pub const COMPACT_TRANSCRIPT_LIMIT: usize = 400_000;

/// Tokens the summary may run to.
pub const COMPACT_SUMMARY_TOKENS: i32 = 4000;

const SUMMARY_SYSTEM: &str = "You are compacting the context of a coding agent that is part-way through a task so it can go on with less history. Write a summary in plain text that lets the agent continue exactly where it left off. Cover, under these headings: Goal (the task and what done looks like); Done so far (files read or changed, commands run and what they showed, decisions and why); Facts learned (paths, names, versions, errors, numbers, anything the agent would otherwise have to look up again); Open problems; Next steps (what to do first). Keep exact file paths, identifiers, commands and error messages verbatim. Keep anything the user or the task asked to keep. Be concrete and complete; leave out pleasantries and reasoning that led nowhere.";

/// True when the prompt is large enough, relative to the window, that the
/// run should compact before the next call.
pub fn should_compact(compact_at: i32, context_window: i32, last_prompt_tokens: i32) -> bool {
    if compact_at <= 0 || context_window <= 0 || last_prompt_tokens <= 0 {
        return false;
    }
    let threshold = (f64::from(context_window) * f64::from(compact_at.min(100)) / 100.0) as i32;
    last_prompt_tokens > threshold
}

/// Where the kept tail starts: the latest assistant turn that leaves at
/// least `keep` messages after it and at least one full turn before it.
/// Cutting at an assistant message keeps every tool result next to the
/// call that asked for it. None when the conversation is too short.
pub fn compaction_cut(msgs: &[Message], keep: usize) -> Option<usize> {
    let keep = keep.max(1);
    let last = msgs.len().checked_sub(keep)?;
    (4..=last).rev().find(|&i| msgs[i].role == Role::Assistant)
}

/// The compacted part of the conversation as text for the summarizer:
/// `msgs[2..cut]`, one block per message, long ones cut in the middle.
pub fn render_transcript(msgs: &[Message], cut: usize) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for (i, m) in msgs.iter().enumerate().take(cut).skip(2) {
        let mut text = String::new();
        match m.role {
            Role::Assistant => {
                text.push_str("## assistant\n");
                if !m.content.trim().is_empty() {
                    text.push_str(&middle_cut(m.content.trim(), COMPACT_MESSAGE_LIMIT));
                    text.push('\n');
                }
                for tc in &m.tool_calls {
                    text.push_str(&format!(
                        "[tool call {}: {}]\n",
                        tc.name,
                        middle_cut(tc.arguments.trim(), 600)
                    ));
                }
            }
            Role::Tool => {
                let name = if m.name.is_empty() {
                    "result".to_string()
                } else {
                    m.name.clone()
                };
                text.push_str(&format!(
                    "## tool result ({name})\n{}\n",
                    middle_cut(m.content.trim(), COMPACT_MESSAGE_LIMIT)
                ));
            }
            Role::User => {
                let head = if m.content.starts_with(COMPACT_MARKER) {
                    "## earlier summary"
                } else if i >= 2 && m.content.starts_with(super::harness::RETURN_CODE_PREFIX) {
                    "## observation"
                } else {
                    "## user"
                };
                text.push_str(&format!(
                    "{head}\n{}\n",
                    middle_cut(m.content.trim(), COMPACT_MESSAGE_LIMIT)
                ));
            }
            Role::System => {
                text.push_str(&format!(
                    "## system\n{}\n",
                    middle_cut(m.content.trim(), COMPACT_MESSAGE_LIMIT)
                ));
            }
        }
        blocks.push(text);
    }
    let mut out = blocks.join("\n");
    if out.len() > COMPACT_TRANSCRIPT_LIMIT {
        let start = out.len() - COMPACT_TRANSCRIPT_LIMIT;
        let start = out.ceil_char_boundary(start);
        out = format!(
            "[earliest part of the transcript omitted]\n\n{}",
            &out[start..]
        );
    }
    out
}

/// The request that asks `model` for the summary.
pub fn summary_request(model: &str, task: &str, transcript: &str) -> Request {
    let user = format!(
        "The task the agent is working on:\n\n{}\n\nThe part of the conversation to compact, oldest first:\n\n{transcript}\n\nWrite the summary now.",
        middle_cut(task.trim(), 12_000)
    );
    Request {
        model: model.to_string(),
        messages: vec![
            Message {
                role: Role::System,
                content: SUMMARY_SYSTEM.to_string(),
                ..Message::new(Role::System)
            },
            Message {
                role: Role::User,
                content: user,
                ..Message::new(Role::User)
            },
        ],
        tools: Vec::new(),
        max_tokens: COMPACT_SUMMARY_TOKENS,
        temperature: None,
        initiator: "agent".to_string(),
    }
}

/// The message that replaces the compacted part.
pub fn compacted_message(summary: &str, replaced: usize) -> Message {
    Message {
        role: Role::User,
        content: format!(
            "{COMPACT_MARKER}\nThe {replaced} messages before this point were replaced by this summary, which you wrote. Continue the task from here; the messages after this one are the most recent ones, unchanged. Do not repeat work the summary records as done.\n\n{}",
            summary.trim()
        ),
        ..Message::new(Role::User)
    }
}

/// The conversation after compaction: system prompt, task, the summary
/// message, then `msgs[cut..]`.
pub fn apply_compaction(msgs: &[Message], cut: usize, summary: &str) -> Vec<Message> {
    let cut = cut.clamp(2, msgs.len());
    let mut out: Vec<Message> = Vec::with_capacity(msgs.len() - cut + 3);
    out.extend(msgs.iter().take(2).cloned());
    out.push(compacted_message(summary, cut - 2));
    out.extend(msgs[cut..].iter().cloned());
    out
}

/// Keeps the start and the end of a long text.
fn middle_cut(s: &str, limit: usize) -> String {
    if s.len() <= limit || limit < 40 {
        return s.to_string();
    }
    let head = s.floor_char_boundary(limit / 2);
    let tail = s.ceil_char_boundary(s.len() - limit / 2);
    format!(
        "{}\n[... {} characters cut ...]\n{}",
        &s[..head],
        tail - head,
        &s[tail..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_provider::ToolCall;

    fn m(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            ..Message::new(Role::User)
        }
    }

    fn conversation() -> Vec<Message> {
        // S, U(task), then three turns: A(tc) T, A(tc) T T, A(text) U(obs), A(tc) T
        let mut a1 = m(Role::Assistant, "");
        a1.tool_calls = vec![ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: r#"{"path":"a.rs"}"#.into(),
        }];
        let mut t1 = m(
            Role::Tool,
            "<returncode>0</returncode>\n<output>\nfn a() {}\n</output>",
        );
        t1.tool_call_id = "c1".into();
        t1.name = "read".into();
        let mut a2 = m(Role::Assistant, "Two more.");
        a2.tool_calls = vec![
            ToolCall {
                id: "c2".into(),
                name: "grep".into(),
                arguments: "{}".into(),
            },
            ToolCall {
                id: "c3".into(),
                name: "glob".into(),
                arguments: "{}".into(),
            },
        ];
        let mut t2 = m(Role::Tool, "g");
        t2.tool_call_id = "c2".into();
        let mut t3 = m(Role::Tool, "h");
        t3.tool_call_id = "c3".into();
        let a3 = m(Role::Assistant, "```bash\nls\n```");
        let o3 = m(
            Role::User,
            "<returncode>0</returncode>\n<output>\na.rs\n</output>",
        );
        let mut a4 = m(Role::Assistant, "");
        a4.tool_calls = vec![ToolCall {
            id: "c4".into(),
            name: "edit".into(),
            arguments: "{}".into(),
        }];
        let mut t4 = m(Role::Tool, "ok");
        t4.tool_call_id = "c4".into();
        vec![
            m(Role::System, "system"),
            m(Role::User, "the task"),
            a1,
            t1,
            a2,
            t2,
            t3,
            a3,
            o3,
            a4,
            t4,
        ]
    }

    #[test]
    fn threshold_is_a_share_of_the_window() {
        assert!(should_compact(70, 1000, 701));
        assert!(!should_compact(70, 1000, 700));
        assert!(!should_compact(0, 1000, 999), "0 is off");
        assert!(!should_compact(70, 0, 999), "no window known");
        assert!(!should_compact(70, 1000, 0), "no measurement yet");
        assert!(should_compact(150, 1000, 1001), "capped at 100 percent");
    }

    #[test]
    fn the_cut_lands_on_an_assistant_turn_leaving_the_tail() {
        let msgs = conversation();
        assert_eq!(msgs.len(), 11);
        // keep 6 -> last index 5; the latest assistant at or before 5 is a2 (index 4)
        assert_eq!(compaction_cut(&msgs, 6), Some(4));
        // keep 2 -> last index 9 = a4
        assert_eq!(compaction_cut(&msgs, 2), Some(9));
        // keep 3 -> last index 8 (observation) -> back to a3 at 7
        assert_eq!(compaction_cut(&msgs, 3), Some(7));
        // nothing before index 4 counts as a turn worth compacting
        assert_eq!(compaction_cut(&msgs[..6], 3), None);
        assert_eq!(compaction_cut(&msgs[..2], 1), None);
        assert_eq!(compaction_cut(&[], 1), None);
    }

    #[test]
    fn transcript_names_every_part_and_cuts_long_ones() {
        let mut msgs = conversation();
        msgs[3].content = format!(
            "<returncode>0</returncode>\n<output>\n{}\n</output>",
            "x".repeat(10_000)
        );
        let text = render_transcript(&msgs, 9);
        assert!(
            text.starts_with("## assistant\n[tool call read: {\"path\":\"a.rs\"}]\n"),
            "{text}"
        );
        assert!(
            text.contains("## tool result (read)\n<returncode>0</returncode>"),
            "{text}"
        );
        assert!(text.contains("characters cut ...]"), "{text}");
        assert!(
            text.contains("## assistant\nTwo more.\n[tool call grep: {}]\n[tool call glob: {}]\n"),
            "{text}"
        );
        assert!(text.contains("## tool result (result)\ng\n"), "{text}");
        assert!(text.contains("## assistant\n```bash\nls\n```\n"), "{text}");
        assert!(
            text.contains("## observation\n<returncode>0</returncode>"),
            "{text}"
        );
        assert!(
            !text.contains("[tool call edit"),
            "index 9 is where the tail starts"
        );
        assert!(
            !text.contains("the task"),
            "the task is not part of the transcript"
        );
        assert!(text.len() < 4000, "{}", text.len());
    }

    #[test]
    fn compaction_replaces_the_middle_and_keeps_the_ends() {
        let msgs = conversation();
        let out = apply_compaction(&msgs, 7, "Goal: finish.\nDone so far: read a.rs.");
        assert_eq!(out.len(), 2 + 1 + 4);
        assert_eq!(out[0].content, "system");
        assert_eq!(out[1].content, "the task");
        assert_eq!(out[2].role, Role::User);
        assert!(
            out[2].content.starts_with(COMPACT_MARKER),
            "{}",
            out[2].content
        );
        assert!(
            out[2].content.contains("The 5 messages before this point"),
            "{}",
            out[2].content
        );
        assert!(
            out[2]
                .content
                .ends_with("Goal: finish.\nDone so far: read a.rs.")
        );
        assert_eq!(out[3].content, "```bash\nls\n```");
        assert_eq!(out[6].tool_call_id, "c4");
        // a second compaction folds the earlier summary into the transcript
        let again = render_transcript(&out, 5);
        assert!(
            again.starts_with("## earlier summary\n[Earlier conversation compacted]"),
            "{again}"
        );
    }

    #[test]
    fn the_summary_request_has_no_tools_and_carries_the_task() {
        let req = summary_request("m-1", "Fix the bug", "## assistant\nhi\n");
        assert_eq!(req.model, "m-1");
        assert!(req.tools.is_empty());
        assert_eq!(req.initiator, "agent");
        assert_eq!(req.max_tokens, COMPACT_SUMMARY_TOKENS);
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, Role::System);
        assert!(req.messages[0].content.contains("Next steps"));
        assert!(req.messages[1].content.contains("Fix the bug"));
        assert!(req.messages[1].content.contains("## assistant\nhi"));
    }

    #[test]
    fn middle_cut_keeps_both_ends() {
        assert_eq!(middle_cut("short", 100), "short");
        let long = format!("{}{}", "a".repeat(100), "b".repeat(100));
        let cut = middle_cut(&long, 60);
        assert!(cut.starts_with(&"a".repeat(30)));
        assert!(cut.ends_with(&"b".repeat(30)));
        assert!(cut.contains("[... 140 characters cut ...]"));
        let uni = "é".repeat(100);
        let cut = middle_cut(&uni, 61);
        assert!(cut.contains("characters cut"));
    }
}
