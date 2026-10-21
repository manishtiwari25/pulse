use crate::register::{actor, input_error, json_output, no_args};
use anyhow::Result;
use pulse_cli::Context;
use pulse_cli::flags::{FlagError, FlagSet};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn settings_usage() -> String {
    format!(
        r#"pulse settings [get NAME | set NAME VALUE | list | help] [--json]

Runner and evolution caps (built-in defaults):
  minutes_per_job  default {} minutes
    Worker wall-clock cap; also the deadline for each measurement harness and
    scorer. Measurement requires a positive value.
  harness.max_steps  default {} model calls
    Model calls one worker run may make before the built-in harness stops it;
    harness.max_cost caps what a run may spend ({} = no cap).
  worker_model     default empty (the runner's default model)
    Model of the runs that change code: task, improve and experiment jobs; also
    every other run while light_model is empty.
  light_model      default empty (the same as worker_model)
    Model of the light runs: lead, answer, ballot, reflect, onboard and clean
    jobs, the sealed held-out call and the executor login check. A cheaper
    model here leaves the strong one to implementation.
  evolve.limits   default {}
    Optional loop budget: cycles=N,hours=H. none does not remove runner caps.

evolve_minutes and evolve_credits are not supported by this CLI; they are
not aliases for the settings above.

Use pulse settings get NAME for an effective value, or pulse settings list --json
for values, defaults, validation rules and descriptions. Only the owner changes caps."#,
        pulse_store::DEFAULT_SETTINGS["minutes_per_job"],
        pulse_store::HARNESS_SETTINGS["harness.max_steps"],
        pulse_store::HARNESS_SETTINGS["harness.max_cost"],
        pulse_store::EVOLVE_SETTINGS["evolve.limits"]
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingReport {
    pub name: String,
    pub value: String,
    pub default: String,
    pub kind: String,
    pub min: Option<i64>,
    #[serde(with = "pulse_store::gojson::nil_slice", default)]
    pub choices: Vec<String>,
    pub description: String,
    pub updated_at: String,
    pub updated_by: String,
}

pub fn settings_report(s: &pulse_store::Store) -> Result<Vec<SettingReport>> {
    let rows = s.setting_rows()?;
    let stored: BTreeMap<_, _> = rows.into_iter().map(|r| (r.name.clone(), r)).collect();
    let mut names = pulse_store::setting_names();
    names.extend(pulse_store::extra_setting_names());
    let mut report = Vec::with_capacity(names.len());
    for name in names {
        let (def, rule) = rule_for(&name);
        let mut r = SettingReport {
            name: name.clone(),
            value: def.clone(),
            default: def,
            kind: rule.kind,
            min: rule.has_min.then_some(rule.min),
            choices: rule.choices,
            description: rule.description,
            ..Default::default()
        };
        if let Some(row) = stored.get(&name) {
            r.value = row.value.clone();
            r.updated_at = row.updated_at.clone();
            r.updated_by = row.updated_by.clone();
        } else if !pulse_store::DEFAULT_SETTINGS.contains_key(&name) {
            r.updated_by = "default".into();
        }
        report.push(r);
    }
    Ok(report)
}

fn rule_for(name: &str) -> (String, pulse_store::SettingRule) {
    for (defs, rules) in [
        (
            &*pulse_store::DEFAULT_SETTINGS,
            &*pulse_store::SETTING_RULES,
        ),
        (&*pulse_store::MODEL_SETTINGS, &*pulse_store::MODEL_RULES),
        (
            &*pulse_store::HARNESS_SETTINGS,
            &*pulse_store::HARNESS_RULES,
        ),
        (
            &*pulse_store::COMPUTE_SETTINGS,
            &*pulse_store::COMPUTE_RULES,
        ),
        (&*pulse_store::EVOLVE_SETTINGS, &*pulse_store::EVOLVE_RULES),
        (&*pulse_store::DAEMON_SETTINGS, &*pulse_store::DAEMON_RULES),
        (&*pulse_store::LEAD_SETTINGS, &*pulse_store::LEAD_RULES),
        (&*pulse_store::OWNER_SETTINGS, &*pulse_store::OWNER_RULES),
    ] {
        if let Some(def) = defs.get(name) {
            return (def.clone(), rules.get(name).cloned().unwrap_or_default());
        }
    }
    (String::new(), pulse_store::SettingRule::default())
}

pub fn listed_settings(s: &pulse_store::Store) -> Result<Vec<pulse_store::SettingRow>> {
    let stored = s.setting_rows()?;
    let extra_names = pulse_store::extra_setting_names();
    let mut extras: BTreeMap<String, pulse_store::SettingRow> = extra_names
        .iter()
        .map(|name| {
            (
                name.clone(),
                pulse_store::SettingRow {
                    name: name.clone(),
                    value: s.setting(name, ""),
                    updated_by: "default".into(),
                    ..Default::default()
                },
            )
        })
        .collect();
    let mut rows = Vec::with_capacity(stored.len() + extras.len());
    for row in stored {
        if extras.contains_key(&row.name) {
            extras.insert(row.name.clone(), row);
        } else {
            rows.push(row);
        }
    }
    for name in extra_names {
        if let Some(row) = extras.remove(&name) {
            rows.push(row);
        }
    }
    Ok(rows)
}

pub fn agent_setting_value(name: &str, value: &str) -> String {
    if pulse_bus::PRIVATE_SETTINGS.contains(&name) && !value.is_empty() {
        pulse_bus::PRIVATE_VALUE.into()
    } else {
        value.into()
    }
}

pub fn run_settings(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    let action = args.first().map_or("list", String::as_str);
    if action == "help" {
        no_args(&args[1..])?;
        return Err(FlagError::Help.into());
    }
    if args.len() > 3 {
        no_args(&args[3..])?;
    }
    let opened;
    if c.store.is_none() {
        opened = pulse_store::Store::open(pulse_store::Options {
            home: c.home.clone(),
            now: c.now.clone(),
            ..Default::default()
        })?;
        c.store = Some(opened);
    }
    match action {
        "list" | "get" => {
            let s = c.store.as_ref().expect("settings store open").clone();
            if action == "get" && args.len() > 1 {
                let values = s.settings()?;
                let name = &args[1];
                let mut exists = true;
                let mut value = values.get(name).cloned().unwrap_or_else(|| {
                    let (def, _) = rule_for(name);
                    if def.is_empty() && !known_default_exists(name) {
                        exists = false;
                    }
                    def
                });
                value = agent_setting_value(name, &value);
                if c.json {
                    #[derive(Serialize)]
                    struct Out<'a> {
                        name: &'a str,
                        value: Option<String>,
                    }
                    return json_output(
                        c,
                        &Out {
                            name,
                            value: exists.then_some(value),
                        },
                    );
                }
                if !exists {
                    value = "None".into();
                }
                c.println(value)?;
                return Ok(());
            }
            if c.json {
                let mut report = settings_report(&s)?;
                for r in &mut report {
                    r.value = agent_setting_value(&r.name, &r.value);
                }
                return json_output(c, &report);
            }
            for r in listed_settings(&s)? {
                c.printf(format_args!(
                    "{:<18} {:<40} {} {}\n",
                    r.name,
                    agent_setting_value(&r.name, &r.value),
                    r.updated_at,
                    r.updated_by
                ))?;
            }
            Ok(())
        }
        "set" => {
            if args.len() < 3 {
                return Err(pulse_cli::errorf(2, "pulse: settings set NAME VALUE", "", "").into());
            }
            if args[1].starts_with("owner.")
                && !c.agent.is_empty()
                && c.agent != pulse_store::HUMAN_AGENT
            {
                return Err(pulse_cli::errorf(
                    2,
                    &format!("pulse: {} is the owner's setting", args[1]),
                    "it hands the owner's decisions to the leads, so only the owner sets it - in the console or here without PULSE_AGENT",
                    &format!("pulse settings get {}", args[1]),
                )
                .into());
            }
            let s = c.store.as_ref().expect("settings store open").clone();
            if args[1] == "push.credential"
                && ((!c.agent.is_empty() && c.agent != pulse_store::HUMAN_AGENT)
                    || pulse_store::worker_guard(&s.home))
            {
                return Err(pulse_cli::errorf(
                    2,
                    "pulse: push.credential is the owner's setting",
                    "it names the key the daemon pushes with (ADR-041), so only the owner sets it - with pulse cred set, which probes the remote first",
                    "pulse cred status",
                )
                .into());
            }
            s.set_setting(&actor(c), &args[1], &args[2])
                .map_err(input_error)?;
            if c.json {
                #[derive(Serialize)]
                struct Out<'a> {
                    name: &'a str,
                    value: String,
                }
                return json_output(
                    c,
                    &Out {
                        name: &args[1],
                        value: agent_setting_value(&args[1], &s.setting(&args[1], "")),
                    },
                );
            }
            c.printf(format_args!(
                "{}={}\n",
                args[1],
                agent_setting_value(&args[1], &args[2])
            ))?;
            Ok(())
        }
        _ => Err(pulse_cli::errorf(
            2,
            "pulse: settings action must be get, set, list, or help",
            "",
            "",
        )
        .into()),
    }
}

fn known_default_exists(name: &str) -> bool {
    pulse_store::DEFAULT_SETTINGS.contains_key(name)
        || pulse_store::MODEL_SETTINGS.contains_key(name)
        || pulse_store::HARNESS_SETTINGS.contains_key(name)
        || pulse_store::COMPUTE_SETTINGS.contains_key(name)
        || pulse_store::EVOLVE_SETTINGS.contains_key(name)
        || pulse_store::DAEMON_SETTINGS.contains_key(name)
        || pulse_store::LEAD_SETTINGS.contains_key(name)
        || pulse_store::OWNER_SETTINGS.contains_key(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use pulse_cli::flags::FlagSet;
    use std::fmt::Write as _;

    #[test]
    fn settings_set_refuses_bad_input() {
        let (_dir, s) = temporary_store();
        for args in [
            vec!["set"],
            vec!["set", "workers"],
            vec!["set", "not-a-setting", "1"],
            vec!["set", "workers", "many"],
            vec!["set", "workers", "-1"],
            vec!["set", "parallel_per_project", "0"],
            vec!["set", "jobs_per_day", "two"],
            vec!["set", "harness.max_steps", "0"],
            vec!["set", "amend_majority", "1/0"],
            vec!["set", "landing", "automatic"],
        ] {
            let before = event_count(&s);
            let got = invoke(Some(s.clone()), false, "", "settings", &args);
            assert_eq!(got.code, 2, "{args:?}: {got:?}");
            assert_eq!(event_count(&s), before);
        }
    }

    #[test]
    fn settings_successful_writes() {
        let (_dir, s) = temporary_store();
        for (name, input, normalized) in [
            ("workers", " 0002 ", "2"),
            ("jobs_per_day", "OFF", "off"),
            ("amend_majority", " 2 / 3 ", "2/3"),
            ("landing", "HUMAN", "human"),
            ("worker_model", "", ""),
        ] {
            let got = invoke(
                Some(s.clone()),
                false,
                "",
                "settings",
                &["set", name, input],
            );
            assert_eq!(got.code, 0, "{got:?}");
            assert_eq!(s.setting(name, ""), normalized);
        }
    }

    #[test]
    fn settings_report() {
        let (_dir, s) = temporary_store();
        let before = event_count(&s);
        let report = super::settings_report(&s).unwrap();
        let mut want = pulse_store::setting_names();
        want.extend(pulse_store::extra_setting_names());
        let got: Vec<_> = report.iter().map(|r| r.name.clone()).collect();
        assert_eq!(got, want);
        for row in &report {
            assert!(
                !row.kind.is_empty() && !row.description.is_empty(),
                "{row:?}"
            );
        }
        let json = invoke(Some(s.clone()), true, "", "settings", &[]);
        let decoded: Vec<SettingReport> = serde_json::from_str(&json.out).unwrap();
        assert_eq!(decoded, report, "{json:?}");
        assert_eq!(event_count(&s), before);
        assert_eq!(
            invoke(
                Some(s.clone()),
                false,
                "",
                "settings",
                &["get", "compute.provider"]
            )
            .out,
            "azure\n"
        );
        assert_eq!(
            invoke(
                Some(s.clone()),
                false,
                "",
                "settings",
                &["get", "owner.presence"]
            )
            .out,
            "present\n"
        );
        let get = invoke(
            Some(s.clone()),
            false,
            "",
            "settings",
            &["get", "light_model"],
        );
        assert_eq!((get.code, get.out.as_str()), (0, "\n"));
        let set = invoke(
            Some(s.clone()),
            false,
            "",
            "settings",
            &["set", "light_model", "claude-sonnet-5"],
        );
        assert_eq!(set.code, 0, "{set:?}");
        assert_eq!(set.out, "light_model=claude-sonnet-5\n");
        assert_eq!(s.light_model(), "claude-sonnet-5");
        assert_eq!(s.run_model("task"), "");
        let set = invoke(
            Some(s.clone()),
            false,
            "",
            "settings",
            &["set", "owner.presence", "Away"],
        );
        assert_eq!(set.code, 0, "{set:?}");
        assert_eq!(set.out, "owner.presence=Away\n");
        assert_eq!(s.setting("owner.presence", ""), "away");
        assert!(s.owner_away());

        crate::register();
        let cmd = pulse_cli::lookup("settings").unwrap();
        let out = pulse_cli::Buffer::new();
        let err = pulse_cli::Buffer::new();
        let mut c = pulse_cli::Context {
            store: Some(s.clone()),
            home: s.home.clone(),
            out: Box::new(out.clone()),
            err: Box::new(err.clone()),
            json: false,
            quiet: false,
            color: false,
            agent: "proj-lead".into(),
            project: String::new(),
            session: String::new(),
            now: TEST_NOW.into(),
            args: vec![],
        };
        let fs = FlagSet::new("settings");
        let args = vec![
            "set".to_string(),
            "owner.presence".to_string(),
            "present".to_string(),
        ];
        let run_err = (cmd.run)(&mut c, &fs, &args).unwrap_err();
        let ce = run_err.downcast_ref::<pulse_cli::Error>().unwrap();
        assert_eq!(ce.exit_code(), 2);
        assert!(
            run_err
                .to_string()
                .contains("owner.presence is the owner's setting")
        );
        assert_eq!(s.setting("owner.presence", ""), "away");

        let args = vec![
            "set".to_string(),
            "push.credential".to_string(),
            "file:///tmp/key".to_string(),
        ];
        let run_err = (cmd.run)(&mut c, &fs, &args).unwrap_err();
        let ce = run_err.downcast_ref::<pulse_cli::Error>().unwrap();
        assert_eq!(ce.exit_code(), 2);
        assert!(
            run_err
                .to_string()
                .contains("push.credential is the owner's setting")
        );
        assert_eq!(s.setting("push.credential", ""), "");
        let set = invoke(
            Some(s.clone()),
            false,
            "",
            "settings",
            &["set", "push.credential", "file:///tmp/key"],
        );
        assert_eq!(set.code, 0, "{set:?}");
        assert_eq!(set.out, "push.credential=file:///tmp/key\n");
        assert_eq!(s.setting("push.credential", ""), "file:///tmp/key");
    }

    #[test]
    fn settings_list_appends_effective_extras() {
        let (_dir, s) = temporary_store();
        let legacy_rows = s.setting_rows().unwrap();
        assert_eq!(legacy_rows.len(), pulse_store::DEFAULT_SETTINGS.len());
        let mut prefix = String::new();
        for row in &legacy_rows {
            writeln!(
                &mut prefix,
                "{:<18} {:<40} {} {}",
                row.name, row.value, row.updated_at, row.updated_by
            )
            .unwrap();
        }
        let before = event_count(&s);
        for args in [vec![], vec!["list"], vec!["get"]] {
            let got = invoke(Some(s.clone()), false, "", "settings", &args);
            let mut want = prefix.clone();
            for name in pulse_store::extra_setting_names() {
                writeln!(
                    &mut want,
                    "{:<18} {:<40}  default",
                    name,
                    s.setting(&name, "")
                )
                .unwrap();
            }
            assert_eq!((got.code, got.out), (0, want), "{args:?}");
        }
        let rows = s.setting_rows().unwrap();
        assert_eq!(rows.len(), legacy_rows.len());
        assert_eq!(event_count(&s), before);
        s.set_setting("owner", "compute.max_vms", "2").unwrap();
        let got = invoke(Some(s.clone()), false, "", "settings", &[]);
        assert!(got.out.starts_with(&prefix), "{}", got.out);
        let needle = format!(
            "{:<18} {:<40} {} {}\n",
            "compute.max_vms", "2", TEST_NOW, "owner"
        );
        assert!(got.out.contains(&needle), "{}", got.out);
        assert_eq!(got.out.matches("compute.max_vms").count(), 1);
    }

    #[test]
    fn heldout_path_never_appears_in_agent_views() {
        let (_dir, s) = temporary_store();
        let private_path = "/tmp/owner-fixtures";
        let set = invoke(
            Some(s.clone()),
            false,
            "",
            "settings",
            &["set", "evolve.heldout_dir", private_path],
        );
        assert_eq!(set.code, 0);
        assert!(!set.out.contains(private_path));
        for (name, args) in [
            ("settings", vec![]),
            ("settings", vec!["get", "evolve.heldout_dir"]),
            ("status", vec![]),
            ("events", vec![]),
        ] {
            let got = invoke(Some(s.clone()), false, "", name, &args);
            assert!(!got.out.contains(private_path), "{name}: {got:?}");
            assert!(got.out.contains("[owner-private]"), "{name}: {got:?}");
        }
        assert_eq!(s.setting("evolve.heldout_dir", ""), private_path);
    }

    #[test]
    fn snapshot_json_stable() {
        let (_dir, s) = temporary_store();
        for name in [
            "status", "projects", "jobs", "agents", "events", "settings", "signers",
        ] {
            let a = invoke(Some(s.clone()), true, "", name, &[]);
            let b = invoke(Some(s.clone()), true, "", name, &[]);
            assert_eq!(a, b, "{name}");
            serde_json::from_str::<serde_json::Value>(&a.out).unwrap();
        }
    }
}
