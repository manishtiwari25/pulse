use std::sync::Once;

static REGISTER: Once = Once::new();
pub fn register() {
    REGISTER.call_once(|| {
        pulse_cli::register(pulse_cli::Command {
            name: "evolve".into(),
            group: "agents".into(),
            raw: true,
            no_store: true,
            read_only: false,
            summary: crate::commands::EVOLVE_SUMMARY.into(),
            usage: crate::commands::EVOLVE_USAGE.into(),
            aliases: Vec::new(),
            run: crate::commands::run_evolve,
            flags: None,
        });
        pulse_cli::register(pulse_cli::Command {
            name: "reflect".into(),
            group: "agents".into(),
            raw: true,
            no_store: true,
            read_only: true,
            summary: crate::commands::REFLECT_SUMMARY.into(),
            usage: crate::commands::REFLECT_USAGE.into(),
            aliases: Vec::new(),
            run: crate::commands::run_reflect,
            flags: None,
        });
        pulse_cli::register(pulse_cli::Command {
            name: "fleet".into(),
            group: "agents".into(),
            raw: true,
            no_store: true,
            read_only: false,
            summary: crate::commands::FLEET_SUMMARY.into(),
            usage: crate::commands::FLEET_USAGE.into(),
            aliases: Vec::new(),
            run: crate::commands::run_fleet,
            flags: None,
        });
        pulse_cli::register(pulse_cli::Command {
            name: "usage".into(),
            group: "inspect".into(),
            raw: true,
            no_store: true,
            read_only: true,
            summary: crate::commands::USAGE_SUMMARY.into(),
            usage: crate::commands::USAGE_USAGE.into(),
            aliases: Vec::new(),
            run: crate::commands::run_usage,
            flags: None,
        });
    });
}
