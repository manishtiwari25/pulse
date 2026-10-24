//! `pulse chat`: the hub's conversations from the command line - list
//! them, read one, and `attach` a project from inside a conversation's own
//! agent (the brief tells it to).
use crate::conversation::{CONVERSATION_ENV, attach_workspace};
use anyhow::Result;
use pulse_cli::{Command, Context, FlagSet, errorf, register as cli_register, write_json};
use serde_json::json;
use std::io::Write;
use std::sync::Once;

static REGISTER: Once = Once::new();

const USAGE: &str = "pulse chat list [--all] [--json]\n       pulse chat show ID [--json]\n       pulse chat attach SLUG [--id ID]";

pub fn register() {
    REGISTER.call_once(|| {
        cli_register(Command {
            name: "chat".into(),
            group: "human".into(),
            summary:
                "the hub's conversations: list them, read one, attach a project from inside one"
                    .into(),
            usage: USAGE.into(),
            aliases: vec![],
            no_store: false,
            read_only: false,
            raw: false,
            run,
            flags: Some(flags),
        });
    });
}

fn flags(fs: &mut FlagSet) {
    fs.bool_var("all", false, "list archived conversations too");
    fs.string_var(
        "id",
        "",
        &format!("the conversation to attach (default: ${CONVERSATION_ENV}, set inside a conversation's agent)"),
    );
}

fn run(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let Some(s) = c.store.clone() else {
        return Err(errorf(2, "pulse chat: no store", "", "pulse init <project-path>").into());
    };
    let verb = args.first().map(String::as_str).unwrap_or("list");
    match (verb, args.len()) {
        ("list", 1) | ("list", 0) => {
            let rows = s.chat_list(fs.get_bool("all"), 200)?;
            if c.json {
                return write_json(c.out.as_mut(), &rows);
            }
            if rows.is_empty() {
                writeln!(c.out, "No conversations yet. Start one in the hub: Chat.")?;
                return Ok(());
            }
            for r in rows {
                writeln!(
                    c.out,
                    "{}  {:<8} {:<16} {}",
                    r.id,
                    r.status,
                    if r.project.is_empty() {
                        "(no project)"
                    } else {
                        r.project.as_str()
                    },
                    r.title
                )?;
            }
            Ok(())
        }
        ("show", 2) => {
            let id = &args[1];
            let Some(row) = s.chat_get(id)? else {
                return Err(errorf(
                    2,
                    &format!("pulse chat show: no conversation {id}"),
                    "",
                    "pulse chat list --all",
                )
                .into());
            };
            let messages = s.chat_messages(id, 0)?;
            if c.json {
                return write_json(
                    c.out.as_mut(),
                    &json!({"conversation": row, "messages": messages}),
                );
            }
            writeln!(c.out, "{}  {}", row.id, row.title)?;
            writeln!(
                c.out,
                "status {}{}  project {}  branch {}  turns {}  session {}",
                row.status,
                if row.note.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", row.note)
                },
                if row.project.is_empty() {
                    "-"
                } else {
                    &row.project
                },
                if row.branch.is_empty() {
                    "-"
                } else {
                    &row.branch
                },
                row.turns,
                if row.session_id.is_empty() {
                    "-"
                } else {
                    &row.session_id
                },
            )?;
            if !row.cwd.is_empty() {
                writeln!(c.out, "cwd {}", row.cwd)?;
            }
            for m in messages {
                writeln!(c.out)?;
                writeln!(c.out, "[{}] {}", m.role, m.at)?;
                writeln!(c.out, "{}", m.content.trim_end())?;
            }
            Ok(())
        }
        ("attach", 2) => {
            let slug = &args[1];
            let mut id = fs.get_string("id");
            if id.is_empty() {
                id = std::env::var(CONVERSATION_ENV).unwrap_or_default();
            }
            if id.is_empty() {
                return Err(errorf(
                    2,
                    "pulse chat attach: which conversation?",
                    &format!("Run it from inside a conversation's agent (${CONVERSATION_ENV} is set there) or pass --id."),
                    "pulse chat attach SLUG --id chat-...",
                )
                .into());
            }
            let home = s.home.clone();
            let attached = attach_workspace(&s, &home, &id, slug)?;
            if c.json {
                return write_json(c.out.as_mut(), &attached);
            }
            writeln!(
                c.out,
                "Conversation {} now works in {} (project {}, branch {} from {}).",
                attached.id, attached.cwd, attached.project, attached.branch, attached.base
            )?;
            writeln!(
                c.out,
                "The move takes effect after this turn: finish your reply now; the next turn starts there by itself."
            )?;
            Ok(())
        }
        _ => Err(errorf(
            2,
            &format!(
                "pulse chat: unknown or incomplete command {:?}",
                args.join(" ")
            ),
            "",
            USAGE,
        )
        .into()),
    }
}
