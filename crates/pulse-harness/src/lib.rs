//! PULSE: the built-in agent harness: bash-tool loop, limits, deny rules, trajectories, sessions.

pub mod agents;
pub mod agents_command;
pub mod background;
pub(crate) mod buildenv;
pub mod chat;
pub mod chat_command;
pub mod commands;
pub mod compact;
pub mod conversation;
pub mod fetch;
pub mod harness;
pub mod history;
pub mod hooks;
pub mod instructions;
pub mod localenv;
pub mod memory;
pub mod permissions;
pub mod readonly;
pub mod rewind;
pub mod search;
pub mod skills;
#[cfg(test)]
pub(crate) mod test_helpers;

pub use commands::*;
pub use compact::*;
pub use fetch::*;
pub use harness::*;
pub use hooks::*;
pub use localenv::*;
pub use readonly::{TreeSnapshot, read_only_refusal};
pub use skills::*;

pub fn register() {
    commands::register();
    chat_command::register();
    *pulse_board::AGENT_SPEC_CHECK
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(agent_spec_check);
}

/// The board's check for `--run-as NAME` on a job, a schedule or a hook:
/// the agent must load for the project the way `pulse harness run --agent`
/// resolves it, by exact name first, then ignoring case.
fn agent_spec_check(
    s: &pulse_store::Store,
    project: &str,
    checkout: &str,
    name: &str,
) -> std::result::Result<(), String> {
    let specs = agents::load_agent_specs(checkout, &s.home, Some(s), project);
    let found = specs
        .iter()
        .find(|a| a.name == name)
        .or_else(|| specs.iter().find(|a| a.name.eq_ignore_ascii_case(name)));
    match found {
        Some(a) if a.loads() => Ok(()),
        Some(a) => Err(format!(
            "{} does not load: {}",
            a.source,
            a.rejected.as_deref().unwrap_or("rejected")
        )),
        None => {
            let known: Vec<&str> = specs
                .iter()
                .filter(|a| a.loads())
                .map(|a| a.name.as_str())
                .collect();
            Err(if known.is_empty() {
                format!(
                    "there is no custom agent {name:?}; add .pulse/agents/{name}.md or ~/.pulse/agents/{name}.md"
                )
            } else {
                format!(
                    "there is no custom agent {name:?}; the agents are {}",
                    known.join(", ")
                )
            })
        }
    }
}
