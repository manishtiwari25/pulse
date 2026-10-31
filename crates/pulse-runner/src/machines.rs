//! Adding a machine from somewhere other than the shell - the console's
//! "Add machine" dialog, through the hub. Two ways in, the same ones
//! `pulse pair` and `pulse executor add` offer:
//!
//! - **pair**: the hub reaches an admin account over ssh, creates the
//!   `pulse` user, installs the hands, registers the executor and probes
//!   it (`pair::pair_executor`). Minutes of work, so the hub runs it on a
//!   thread and shows the report as it lands.
//! - **register**: a machine someone already prepared by hand is added as
//!   a drained executor and, when asked, probed at once
//!   (`executor::add_executor` + `probe_executor`).
//!
//! Both validate up front so a bad name or address answers the request,
//! not a thread. The crate's own entry points stay crate-private; this is
//! the one surface the hub sees.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegisterMachine {
    pub name: String,
    /// `user@host[:port]` the daemon reaches the hands with (usually `pulse@host`).
    pub address: String,
    /// Absolute work directory on the machine; empty means `/srv/pulse`.
    pub workdir: String,
    pub workers: i32,
    /// Project slugs the machine takes, comma-separated; empty or `*` for all.
    pub projects: String,
    /// Labels the owner gives it (`gpu,docker`); empty for none.
    pub labels: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MachineAdded {
    pub name: String,
    pub status: String,
    pub offers: String,
    pub line: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MachineProbe {
    pub ok: bool,
    pub status: String,
    pub note: String,
    pub line: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PairMachine {
    pub name: String,
    /// `admin@host[:port]`: an account with passwordless sudo the hub can ssh to.
    pub admin: String,
    pub workdir: String,
    pub workers: i32,
    pub projects: String,
    pub labels: String,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PairOutcome {
    pub ok: bool,
    pub lines: Vec<String>,
    pub changed: Vec<String>,
}

pub const DEFAULT_WORKDIR: &str = crate::hands::HANDS_DEFAULT_WORKDIR;

fn workdir_or_default(workdir: &str) -> String {
    let w = workdir.trim();
    if w.is_empty() {
        DEFAULT_WORKDIR.to_string()
    } else {
        w.to_string()
    }
}

fn workers_or_default(workers: i32) -> i32 {
    if workers <= 0 { 1 } else { workers }
}

/// Checks the words a machine would be labelled with the way `pulse executor
/// add --labels` does, before anything is written.
fn check_labels(labels: &str) -> Result<()> {
    if labels.trim().is_empty() {
        return Ok(());
    }
    pulse_store::needs::parse(labels)
        .map(|_| ())
        .map_err(|e| crate::util::fail(2, e).into())
}

/// Adds a prepared machine as an executor: validated and written the way
/// `pulse executor add NAME --ssh --workdir --workers --projects --labels`
/// is. The machine is `drained` until a probe passes (`probe_machine`, or
/// the daemon's own round). Returns what the shell command would have
/// printed as `line`.
pub fn register_machine(
    s: &pulse_store::Store,
    now: &str,
    m: &RegisterMachine,
    actor: &str,
) -> Result<MachineAdded> {
    if m.address.trim().is_empty() {
        return Err(crate::util::fail(
            2,
            "The machine's ssh address is needed (user@host[:port], usually pulse@host)",
        )
        .into());
    }
    check_labels(&m.labels)?;
    let workdir = workdir_or_default(&m.workdir);
    let mut r = crate::executor::add_executor(
        s,
        now,
        &m.name,
        &m.address,
        &workdir,
        workers_or_default(m.workers),
        &m.projects,
        actor,
    )?;
    let name = r.str("name");
    if !m.labels.trim().is_empty() {
        crate::executor::set_labels(s, now, &name, &m.labels, actor)?;
        r = crate::executor::executor_row(s, &name);
    }
    let offers = crate::executor::caps_text(&r);
    let status = r.str("status");
    let line = format!(
        "added {name} ({}, {}; offers {offers}); it is drained until `pulse executor probe {name}` passes",
        r.str("address"),
        r.str("workdir")
    );
    Ok(MachineAdded {
        name,
        status,
        offers,
        line,
    })
}

/// Probes a machine the way `pulse executor probe NAME` does: reaches the
/// hands over ssh (a minute at most) and records the verdict, which moves
/// the row between `drained` and `active`.
pub fn probe_machine(
    s: &pulse_store::Store,
    now: &str,
    name: &str,
    actor: &str,
) -> Result<MachineProbe> {
    let (ok, note) = crate::executor::probe_executor(s, name, actor, now)?;
    let r = crate::executor::executor_row(s, name);
    let status = r.str("status");
    let line = if ok {
        format!("{name} ok, {status}: {note}")
    } else {
        format!("{name} FAILED ({status}): {note}")
    };
    Ok(MachineProbe {
        ok,
        status,
        note,
        line,
    })
}

/// Validates a pairing the way `pulse pair` does before it touches ssh:
/// the name, the admin address, the work directory, the workers and the
/// labels. Returns the executor's name as it will be stored.
pub fn pair_check(m: &PairMachine) -> Result<String> {
    if m.admin.trim().is_empty() {
        return Err(crate::util::fail(
            2,
            "The admin ssh address is needed (admin@host[:port], an account with passwordless sudo)",
        )
        .into());
    }
    check_labels(&m.labels)?;
    let (name, _, _, _, _) = crate::pair::pair_validate(
        m.name.clone(),
        m.admin.clone(),
        workdir_or_default(&m.workdir),
        workers_or_default(m.workers),
    )?;
    Ok(name)
}

/// Pairs a machine: everything `pulse pair ADMIN --name NAME` does, then the
/// labels when the pairing held. Takes minutes; callers run it off the
/// request thread. `actor` is the human unless an authorized agent asks.
pub fn pair_machine(
    s: &pulse_store::Store,
    now: &str,
    m: &PairMachine,
    actor: &str,
) -> Result<PairOutcome> {
    let name = pair_check(m)?;
    let rep = crate::pair::pair_executor(
        s,
        m.admin.trim().to_string(),
        name.clone(),
        now.to_string(),
        crate::pair::PairOpts {
            workdir: workdir_or_default(&m.workdir),
            workers: workers_or_default(m.workers),
            projects: m.projects.clone(),
            dry_run: m.dry_run,
            actor: actor.to_string(),
            ..Default::default()
        },
    )?
    .ok_or_else(|| anyhow!("pair did not return a report"))?;
    let mut out = PairOutcome {
        ok: rep.ok,
        lines: rep.lines,
        changed: rep.changed,
    };
    if out.ok && !m.dry_run && !m.labels.trim().is_empty() {
        match crate::executor::set_labels(s, now, &name, &m.labels, actor) {
            Ok(_) => {
                let r = crate::executor::executor_row(s, &name);
                out.lines.push(format!(
                    "labels {}: offers {}",
                    m.labels.trim(),
                    crate::executor::caps_text(&r)
                ));
            }
            Err(e) => out.lines.push(format!(
                "labels not set: {}; pulse executor label {name} {}",
                crate::run::err_message(&e),
                m.labels.trim()
            )),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_validates_then_adds_labels_and_reports() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let now = crate::test_helpers::TEST_NOW;
        let bad = register_machine(
            &s,
            now,
            &RegisterMachine {
                name: "box".into(),
                address: "pulse@10.0.0.9".into(),
                labels: "GPU!".into(),
                ..Default::default()
            },
            "human",
        )
        .unwrap_err();
        assert!(
            crate::run::err_message(&bad).contains("GPU!"),
            "labels are checked first: {bad}"
        );
        let none = register_machine(
            &s,
            now,
            &RegisterMachine {
                name: "box".into(),
                ..Default::default()
            },
            "human",
        )
        .unwrap_err();
        assert!(crate::run::err_message(&none).contains("ssh address is needed"));
        let added = register_machine(
            &s,
            now,
            &RegisterMachine {
                name: "Box One".into(),
                address: "pulse@10.0.0.9".into(),
                workdir: "".into(),
                workers: 0,
                projects: "".into(),
                labels: "gpu, docker".into(),
            },
            "human",
        )
        .unwrap();
        assert_eq!(added.name, "box-one");
        assert_eq!(added.status, "drained");
        assert!(added.offers.contains("gpu"), "{}", added.offers);
        assert!(added.offers.contains("docker"), "{}", added.offers);
        assert!(
            added
                .line
                .contains("it is drained until `pulse executor probe box-one` passes"),
            "{}",
            added.line
        );
        let r = crate::executor::executor_row(&s, "box-one");
        assert_eq!(r.str("workdir"), DEFAULT_WORKDIR);
        assert_eq!(r.int("workers"), 1);
        assert_eq!(r.str("projects"), "*");
        let twice = register_machine(
            &s,
            now,
            &RegisterMachine {
                name: "box-one".into(),
                address: "pulse@10.0.0.9".into(),
                ..Default::default()
            },
            "human",
        )
        .unwrap_err();
        assert!(crate::run::err_message(&twice).contains("already exists"));
    }

    #[test]
    fn pair_check_refuses_before_ssh() {
        let bad_name = pair_check(&PairMachine {
            name: "local".into(),
            admin: "admin@10.0.0.5".into(),
            ..Default::default()
        })
        .unwrap_err();
        assert!(crate::run::err_message(&bad_name).contains("Executor name must be"));
        let no_admin = pair_check(&PairMachine {
            name: "box".into(),
            ..Default::default()
        })
        .unwrap_err();
        assert!(crate::run::err_message(&no_admin).contains("admin ssh address is needed"));
        let bad_admin = pair_check(&PairMachine {
            name: "box".into(),
            admin: "not an address".into(),
            ..Default::default()
        })
        .unwrap_err();
        assert!(!crate::run::err_message(&bad_admin).is_empty());
        let relative = pair_check(&PairMachine {
            name: "box".into(),
            admin: "admin@10.0.0.5".into(),
            workdir: "srv/pulse".into(),
            ..Default::default()
        })
        .unwrap_err();
        assert!(crate::run::err_message(&relative).contains("absolute path"));
        assert_eq!(
            pair_check(&PairMachine {
                name: "Box Two".into(),
                admin: "admin@10.0.0.5:2222".into(),
                labels: "xcode".into(),
                ..Default::default()
            })
            .unwrap(),
            "box-two"
        );
    }
}
