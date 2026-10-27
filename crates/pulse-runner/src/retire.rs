#![allow(dead_code, clippy::type_complexity)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::{LazyLock, RwLock};

pub(crate) const HEAL_THREAD: &str = "executors";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RetiredVm {
    pub machine: crate::compute::Machine,
    pub request: String,
}

pub(crate) static SSH_HOSTNAME: LazyLock<RwLock<Box<dyn Fn(&str) -> String + Send + Sync>>> =
    LazyLock::new(|| {
        RwLock::new(Box::new(|host| {
            let out = std::process::Command::new("ssh")
                .args(["-G", host])
                .output();
            let Ok(out) = out else {
                return host.to_string();
            };
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() == 2 && f[0] == "hostname" {
                    return f[1].to_string();
                }
            }
            host.to_string()
        }))
    });

pub(crate) fn set_ssh_hostname(
    f: Box<dyn Fn(&str) -> String + Send + Sync>,
) -> Box<dyn Fn(&str) -> String + Send + Sync> {
    std::mem::replace(
        &mut *SSH_HOSTNAME.write().expect("ssh_hostname poisoned"),
        f,
    )
}

pub(crate) fn retire_candidates(s: &pulse_store::Store) -> Vec<crate::util::Row> {
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    crate::util::must_rows(
        &conn,
        "SELECT * FROM executors WHERE status = 'drained' AND name != ? ORDER BY name",
        &[&crate::executor::LOCAL_EXECUTOR],
    )
    .into_iter()
    .filter(|r| crate::executor::login_fault(&r.str("probe_note")) && r.int("backoff_minutes") > 15)
    .collect()
}

pub(crate) fn retire_provider(s: &pulse_store::Store) -> Option<Arc<dyn crate::compute::Provider>> {
    crate::compute::provider_named(s.setting("compute.provider", "").trim())
}

pub(crate) fn identify_vm(
    s: &pulse_store::Store,
    r: crate::util::Row,
) -> (Option<Box<RetiredVm>>, bool, String) {
    let name = r.str("name");
    if let Ok(conn) = s.conn() {
        let mrow = crate::util::must_one(
            &conn,
            "SELECT request_id FROM compute_machines WHERE executor = ? AND released_at = '' ORDER BY created_at DESC LIMIT 1",
            &[&name],
        );
        if !mrow.0.is_empty() {
            let (m, _) = crate::compute::load_machine(s, &mrow.str("request_id"));
            if let Some(m) = m {
                let vm = RetiredVm {
                    machine: m,
                    request: mrow.str("request_id"),
                };
                return (Some(Box::new(vm)), true, String::new());
            }
        }
    }
    let host = r
        .str("address")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string();
    let ip = if host.is_empty() {
        String::new()
    } else {
        (SSH_HOSTNAME.read().expect("ssh hostname"))(&host).replace('\'', "")
    };
    let Some(p) = retire_provider(s) else {
        return (
            None,
            false,
            format!(
                "no compute provider {} is built in",
                crate::board::py_repr(s.setting("compute.provider", ""))
            ),
        );
    };
    match p.find(&pulse_bus::CancellationToken::new(), s, &name, &ip) {
        Ok(Some(mut m)) => {
            if m.provider.is_empty() {
                m.provider = p.name();
            }
            (
                Some(Box::new(RetiredVm {
                    machine: m,
                    request: String::new(),
                })),
                true,
                String::new(),
            )
        }
        Ok(None) => (
            None,
            false,
            format!(
                "no {} tagged pulse=hands is named {} or answers at {}",
                p.kind(),
                name,
                if ip.is_empty() { "-" } else { &ip }
            ),
        ),
        Err(err) => {
            let msg = crate::run::err_message(&err);
            let note = if p.name() == "azure" && !msg.starts_with("the Azure CLI could not answer")
            {
                format!("the Azure CLI could not answer: {msg}")
            } else {
                msg
            };
            (None, false, note)
        }
    }
}

pub(crate) fn tend_executors(s: &pulse_store::Store, now: String, actor: String) -> Vec<String> {
    let mut lines = Vec::new();
    let heal = s.setting("compute.self_heal", "1") == "1";
    let retire = s.setting("compute.self_retire", "1") == "1";
    if heal || retire {
        for r in retire_candidates(s) {
            let mut healed = false;
            if heal && r.str("healed_at").is_empty() {
                let (ok, line) = heal_executor(
                    s,
                    now.clone(),
                    actor.clone(),
                    r.clone(),
                    Box::new(|_, _| {}),
                );
                healed = ok;
                lines.push(line);
            }
            if healed || !retire {
                continue;
            }
            let (vm, ours, note) = identify_vm(s, r.clone());
            if let Some(vm) = vm {
                lines.extend(retire_executor(
                    s,
                    now.clone(),
                    actor.clone(),
                    r,
                    &vm,
                    ours,
                    note,
                ));
            } else {
                lines.extend(retire_executor(
                    s,
                    now.clone(),
                    actor.clone(),
                    r,
                    &RetiredVm::default(),
                    false,
                    note,
                ));
            }
        }
    }
    lines.extend(delete_retired(s, now, actor));
    lines
}

pub(crate) fn heal_executor(
    s: &pulse_store::Store,
    now: String,
    actor: String,
    r: crate::util::Row,
    _identify: Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
) -> (bool, String) {
    let name = r.str("name");
    let (tok, origin) = match crate::token::login_source(s) {
        Ok(x) => x,
        Err(err) => {
            return (
                false,
                format!(
                    "heal {name}: login source: {}",
                    crate::run::err_message(&err)
                ),
            );
        }
    };
    if tok.is_empty() {
        return (
            false,
            format!("heal {name}: no login source (pulse executor token --with-token)"),
        );
    }
    let mut admin = r.str("admin");
    if admin.is_empty() {
        let (vm, ours, _) = identify_vm(s, r.clone());
        if ours && let Some(vm) = vm {
            admin = admin_route(
                r.str("address"),
                vm.machine
                    .detail
                    .get("public_ip")
                    .cloned()
                    .unwrap_or_default(),
            );
        }
    }
    if admin.is_empty() {
        return (
            false,
            format!(
                "heal {name}: no admin route (no admin address on record and the machine is not one PULSE made)"
            ),
        );
    }
    if let Ok(conn) = s.conn() {
        let _ = conn.execute(
            "UPDATE executors SET healed_at = ? WHERE name = ?",
            rusqlite::params![now, name],
        );
    }
    let mut reader: &[u8] = &tok;
    let res = (*crate::executor::PLACE_LOGIN
        .read()
        .expect("place login poisoned"))(
        s, &now, &name, &admin, &mut reader, false, false, &actor
    );
    let res = match res {
        Ok(res) => res,
        Err(err) => {
            return (
                false,
                format!(
                    "heal {name}: {}",
                    crate::executor::redact_tokens(&crate::run::err_message(&err))
                ),
            );
        }
    };
    let after = crate::executor::executor_row(s, &name);
    if !res.ok || after.str("status") != "active" {
        let last = res.lines.last().cloned().unwrap_or_default();
        return (
            false,
            format!(
                "heal {name} refused: {}",
                crate::executor::redact_tokens(&last)
            ),
        );
    }
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "executor.healed",
            &name,
            serde_json::json!({"source": origin, "admin": admin}),
        );
    }
    if let proj = crate::token::finding_project(s)
        && !proj.is_empty()
    {
        crate::board::create_finding_or_warn(
            s,
            now.clone(),
            &format!("executor {name} healed"),
            crate::board::PostOpts {
                kind: "finding".into(),
                project: proj,
                area: crate::compute::COMPUTE_AREA.into(),
                thread: HEAL_THREAD.into(),
                summary: format!("healed {name}: login placed from {origin}"),
                body: format!(
                    "Executor `{name}` was drained by a refused login ({}). The daemon placed the {origin} over {admin} and the login check passed; the executor is active again (ADR-034). No credential was created; the one placed reaches no repository.\n",
                    crate::executor::redact_tokens(&crate::util::trunc(&r.str("probe_note"), 200)),
                ),
                agent: crate::board::actor_agent(s, actor),
                date: now.clone(),
                ..Default::default()
            },
        );
    }
    (true, format!("healed {name}: login placed from {origin}"))
}

pub(crate) fn admin_route(address: String, ip: String) -> String {
    let mut host = address.trim().to_string();
    if let Some((_, after)) = host.split_once('@') {
        host = after.to_string();
    }
    if host.is_empty() {
        host = ip;
    }
    if host.is_empty() {
        String::new()
    } else {
        format!("{}@{}", crate::provider::ADMIN_USER, host)
    }
}

pub(crate) fn machine_detail(
    m: &crate::compute::Machine,
    extra: BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    let mut detail = BTreeMap::new();
    for (k, v) in &m.detail {
        if !v.is_empty() {
            detail.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
    }
    detail.extend(extra);
    detail
}

pub(crate) fn save_retired(
    s: &pulse_store::Store,
    executor: String,
    m: &crate::compute::Machine,
    now: String,
    delete_after: String,
) {
    let Ok(conn) = s.conn() else { return };
    let detail =
        pulse_store::gojson::to_string(&serde_json::json!({"tags": m.tags, "detail": m.detail}))
            .unwrap_or_default();
    let _ = conn.execute(
        "INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after, deleted_at, note, detail) VALUES (?, ?, ?, ?, ?, ?, '', '', ?) ON CONFLICT(executor) DO UPDATE SET provider = excluded.provider, resource_group = excluded.resource_group, name = excluded.name, deallocated_at = excluded.deallocated_at, delete_after = excluded.delete_after, deleted_at = '', note = '', detail = excluded.detail",
        rusqlite::params![executor, m.provider, m.detail.get("resource_group").cloned().unwrap_or_default(), m.name, now, delete_after, detail],
    );
}

pub(crate) fn retired_machine(r: crate::util::Row) -> Option<Box<crate::compute::Machine>> {
    let mut m = crate::compute::Machine {
        provider: r.str("provider"),
        name: r.str("name"),
        ..Default::default()
    };
    if !r.str("resource_group").is_empty() {
        m.detail
            .insert("resource_group".into(), r.str("resource_group"));
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r.str("detail")) {
        if let Some(tags) = v.get("tags").and_then(|v| v.as_object()) {
            for (k, v) in tags {
                if let Some(s) = v.as_str() {
                    m.tags.insert(k.clone(), s.to_string());
                }
            }
        }
        if let Some(detail) = v.get("detail").and_then(|v| v.as_object()) {
            for (k, v) in detail {
                if let Some(s) = v.as_str() {
                    m.detail.insert(k.clone(), s.to_string());
                }
            }
        }
    }
    Some(Box::new(m))
}

pub(crate) fn retire_executor(
    s: &pulse_store::Store,
    now: String,
    actor: String,
    r: crate::util::Row,
    vm: &RetiredVm,
    ours: bool,
    note: String,
) -> Vec<String> {
    let name = r.str("name");
    let refused = crate::compute::redact_secrets(&crate::util::trunc(
        r.str("probe_note").trim_start_matches("fault: "),
        300,
    ));
    if let Err(err) = pulse_board::remove_executor(s, &name, &actor, true) {
        return vec![format!(
            "retire {name} postponed: {}",
            crate::run::err_message(&err)
        )];
    }
    if ours && !vm.request.is_empty() {
        let req = match pulse_tasks::get_compute(s, &vm.request) {
            Ok(req) => req,
            Err(err) => {
                return vec![format!(
                    "retire {name} postponed: {}",
                    crate::run::err_message(&err)
                )];
            }
        };
        if !crate::compute::release_compute(
            s,
            &actor,
            req,
            &format!("its login was refused ({refused})"),
            |_| {},
        ) {
            return vec![format!(
                "retire {name} postponed: releasing {} failed; the daemon retries",
                vm.machine.name
            )];
        }
        if let Ok(conn) = s.conn() {
            let mut detail = machine_detail(
                &vm.machine,
                BTreeMap::from([
                    ("provider".into(), serde_json::json!(vm.machine.provider)),
                    ("name".into(), serde_json::json!(vm.machine.name)),
                    ("request".into(), serde_json::json!(vm.request)),
                    ("released".into(), serde_json::json!(true)),
                ]),
            );
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "executor.retired",
                &name,
                serde_json::json!(detail),
            );
            detail.clear();
        }
        return vec![format!(
            "retired {name}: compute machine {} released (login refused)",
            vm.machine.name
        )];
    }
    let mut provider: Option<Arc<dyn crate::compute::Provider>> = None;
    if ours {
        provider = crate::compute::provider_named(&vm.machine.provider);
        let Some(p) = provider.as_ref() else {
            return vec![format!(
                "retire {name} postponed: no compute provider {} is built in",
                crate::board::py_repr(vm.machine.provider.clone())
            )];
        };
        if let Err(err) = p.stop(&pulse_bus::CancellationToken::new(), &vm.machine) {
            return vec![format!(
                "retire {name} postponed: {}",
                crate::compute::redact_secrets(&crate::run::err_message(&err))
            )];
        }
    }
    if let Err(err) = pulse_board::remove_executor(s, &name, &actor, false) {
        return vec![format!(
            "retire {name} postponed: {}",
            crate::run::err_message(&err)
        )];
    }
    let next = "A Copilot-only fine-grained token placed with `pulse executor token --with-token` lets the daemon heal future machines instead.";
    let mut detail = BTreeMap::from([
        ("ours".into(), serde_json::json!(ours)),
        ("refused".into(), serde_json::json!(refused)),
    ]);
    let (summary, body, line) = if ours {
        let p = provider.expect("provider set for owned machine");
        let mut grace = s.setting_int("compute.retire_grace_hours");
        if grace < 1 {
            grace = 24;
        }
        let delete_after = crate::util::plus_hours(&now, grace as f64);
        save_retired(
            s,
            name.clone(),
            &vm.machine,
            now.clone(),
            delete_after.clone(),
        );
        let place = p.place(&vm.machine);
        detail = machine_detail(&vm.machine, detail);
        detail.insert("provider".into(), serde_json::json!(vm.machine.provider));
        detail.insert("name".into(), serde_json::json!(vm.machine.name));
        detail.insert("place".into(), serde_json::json!(place));
        detail.insert("delete_after".into(), serde_json::json!(delete_after));
        (
            format!(
                "retired {name}: login refused; {} {} ({}) stopped now, deleted at {} unless kept",
                p.kind(),
                vm.machine.name,
                p.place(&vm.machine),
                delete_after
            ),
            format!(
                "The daemon retired executor `{name}` (ADR-034).\n\nWhat was refused: {refused}\n\nWhat the daemon did: the {} `{}` in {} ({}) which PULSE created, is stopped now ({}) and stops costing money; the executor row is removed. The daemon deletes the machine and what was created beside it at {} (compute.retire_grace_hours={}).\n\nUndo: `{}` then `pulse executor activate {name}` before {} keeps it.\n\n{}\n",
                p.kind(),
                vm.machine.name,
                p.place(&vm.machine),
                vm.machine.provider,
                now,
                delete_after,
                grace,
                p.start_hint(&vm.machine),
                delete_after,
                next
            ),
            format!(
                "retired {name}: stopped {} ({}); deleted at {} unless kept",
                vm.machine.name,
                p.place(&vm.machine),
                delete_after
            ),
        )
    } else {
        detail.insert("note".into(), serde_json::json!(note));
        (
            format!(
                "retired {name}: login refused; machine left running because PULSE did not create it"
            ),
            format!(
                "The daemon retired executor `{name}` (ADR-034).\n\nWhat was refused: {refused}\n\nWhat the daemon did: the executor row is removed; the machine at {} was left running because PULSE did not create it ({}). Stopping it, or fixing its login and pairing it again, is yours.\n\n{}\n",
                r.str("address"),
                note,
                next
            ),
            format!(
                "retired {name}: row removed; machine left running because PULSE did not create it ({note})"
            ),
        )
    };
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "executor.retired",
            &name,
            serde_json::json!(detail),
        );
    }
    let proj = crate::token::finding_project(s);
    if !proj.is_empty() {
        crate::board::create_finding_or_warn(
            s,
            now.clone(),
            &format!("executor {name} retired"),
            crate::board::PostOpts {
                kind: "finding".into(),
                project: proj,
                area: crate::compute::COMPUTE_AREA.into(),
                thread: crate::compute::COMPUTE_THREAD.into(),
                summary: crate::util::trunc(&summary, 230),
                body,
                agent: crate::board::actor_agent(s, actor),
                date: now,
                ..Default::default()
            },
        );
    }
    vec![line]
}

pub(crate) fn delete_retired(s: &pulse_store::Store, now: String, actor: String) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(conn) = s.conn() else { return lines };
    let rows = crate::util::must_rows(
        &conn,
        "SELECT * FROM retired_vms WHERE deleted_at = '' AND delete_after != '' AND delete_after <= ? ORDER BY executor",
        &[&now],
    );
    for r in rows {
        let name = r.str("executor");
        let Some(m) = retired_machine(r.clone()) else {
            continue;
        };
        let mut status = String::new();
        let e = crate::util::must_one(
            &conn,
            "SELECT status FROM executors WHERE name = ?",
            &[&name],
        );
        if !e.0.is_empty() {
            status = e.str("status");
        }
        if status == "active" {
            let _ = conn.execute(
                "UPDATE retired_vms SET deleted_at = ?, note = 'kept by the owner' WHERE executor = ?",
                rusqlite::params![now, name],
            );
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "executor.kept",
                &name,
                serde_json::json!(machine_detail(
                    &m,
                    BTreeMap::from([
                        ("provider".to_string(), serde_json::json!(m.provider)),
                        ("name".to_string(), serde_json::json!(m.name)),
                    ]),
                )),
            );
            lines.push(format!(
                "kept {name}: the owner activated it again; {} stays",
                m.name
            ));
            continue;
        }
        let Some(p) = crate::compute::provider_named(&m.provider) else {
            let _ = conn.execute(
                "UPDATE retired_vms SET deleted_at = ?, note = ? WHERE executor = ?",
                rusqlite::params![
                    now,
                    format!("no delete step for provider {}", m.provider),
                    name
                ],
            );
            lines.push(format!(
                "retired {name}: no delete step for provider {}",
                m.provider
            ));
            continue;
        };
        let place = p.place(&m);
        let (found, gone) = match p.delete(&pulse_bus::CancellationToken::new(), &m) {
            Ok(v) => v,
            Err(err) => {
                lines.push(format!(
                    "delete {} postponed: {}",
                    m.name,
                    crate::compute::redact_secrets(&crate::run::err_message(&err))
                ));
                continue;
            }
        };
        let mut detail = machine_detail(
            &m,
            BTreeMap::from([
                ("provider".to_string(), serde_json::json!(m.provider)),
                ("name".to_string(), serde_json::json!(m.name)),
                ("place".to_string(), serde_json::json!(place)),
                ("deleted".to_string(), serde_json::json!(gone)),
            ]),
        );
        if !found {
            let _ = conn.execute(
                "UPDATE retired_vms SET deleted_at = ?, note = 'vm not found at delete time' WHERE executor = ?",
                rusqlite::params![now, name],
            );
            detail.insert("missing".into(), serde_json::json!(true));
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "executor.deleted",
                &name,
                serde_json::json!(detail),
            );
            lines.push(format!("retired {name}: {} was already gone", m.name));
            continue;
        }
        let _ = conn.execute(
            "UPDATE retired_vms SET deleted_at = ? WHERE executor = ?",
            rusqlite::params![now, name],
        );
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "executor.deleted",
            &name,
            serde_json::json!(detail),
        );
        let proj = crate::token::finding_project(s);
        if !proj.is_empty() {
            crate::board::create_finding_or_warn(
                s,
                now.clone(),
                &format!("machine {} deleted", m.name),
                crate::board::PostOpts {
                    kind: "finding".into(),
                    project: proj,
                    area: crate::compute::COMPUTE_AREA.into(),
                    thread: crate::compute::COMPUTE_THREAD.into(),
                    summary: format!("deleted {} ({})", m.name, place),
                    body: format!(
                        "The {} `{}` in {}, stopped on {} when executor `{}` was retired, is deleted with {} (ADR-034). \
                         What may be shared with other machines (on Azure the virtual network and network security group) stays. Only this finding remains of the machine.\n",
                        p.kind(),
                        m.name,
                        place,
                        r.str("deallocated_at"),
                        name,
                        gone.join(", ")
                    ),
                    agent: crate::board::actor_agent(s, actor.clone()),
                    date: now.clone(),
                    ..Default::default()
                },
            );
        }
        lines.push(format!(
            "deleted {} ({}): {}",
            m.name,
            place,
            gone.join(", ")
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_cloud::*;
    use crate::test_helpers_daemon::must_one;
    use crate::test_helpers_environment::drained_by_login;
    use crate::test_helpers_provider::invoke_ok;
    use crate::util::json_map;
    use rusqlite::params;

    #[test]
    fn retire_candidates_rule() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        crate::executor::add_executor(
            &s,
            TEST_NOW,
            "vm-fresh",
            "pulse@fresh",
            "/srv/pulse",
            1,
            "*",
            pulse_store::HUMAN_AGENT,
        )
        .unwrap();
        s.conn().unwrap().execute("UPDATE executors SET status = 'drained', probe_note = 'fault: not logged in', backoff_minutes = ? WHERE name = 'vm-fresh'", params![15]).unwrap();
        crate::executor::add_executor(
            &s,
            TEST_NOW,
            "vm-disk",
            "pulse@disk",
            "/srv/pulse",
            1,
            "*",
            pulse_store::HUMAN_AGENT,
        )
        .unwrap();
        s.conn().unwrap().execute("UPDATE executors SET status = 'drained', probe_note = 'fault: disk full', backoff_minutes = 360 WHERE name = 'vm-disk'", []).unwrap();
        crate::executor::add_executor(
            &s,
            TEST_NOW,
            "vm-ok",
            "pulse@ok",
            "/srv/pulse",
            1,
            "*",
            pulse_store::HUMAN_AGENT,
        )
        .unwrap();
        s.conn().unwrap().execute("UPDATE executors SET status = 'active', probe_note = 'not logged in', backoff_minutes = 30 WHERE name = 'vm-ok'", []).unwrap();
        let got = retire_candidates(&s);
        assert_eq!(got.len(), 1, "candidates: {got:?}");
        assert_eq!(got[0].str("name"), "vm-seven");
    }

    #[test]
    fn tend_retires_our_azure_vm() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        new_bare_repo(&s);
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        let az = RetireAz::with_list(
            r#"[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]"#,
        );
        let _az_guard = install_retire_az(&az);
        let placed = stub_place_login(true);

        let lines = tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert_eq!(placed.calls(), Vec::<PlacedLogin>::new());
        let calls = az.joined();
        assert_eq!(calls.len(), 2, "az calls: {calls:?}");
        assert!(calls[0].starts_with("vm list -d --query [?tags.pulse=='hands' && (name=='vm-seven' || publicIps=='203.0.113.7')].{name:name,rg:resourceGroup,ip:publicIps} -o json"), "{calls:?}");
        assert_eq!(
            calls[1],
            "vm deallocate -g pulse-hands -n pulse-cr-007 -o none"
        );
        assert_eq!(
            must_one(
                &s,
                "SELECT status FROM executors WHERE name = 'vm-seven'",
                &[]
            )
            .str("status"),
            "removed"
        );
        let r = must_one(
            &s,
            "SELECT * FROM retired_vms WHERE executor = 'vm-seven'",
            &[],
        );
        assert_eq!(r.str("provider"), "azure");
        assert_eq!(r.str("resource_group"), "pulse-hands");
        assert_eq!(r.str("name"), "pulse-cr-007");
        assert_eq!(r.str("deallocated_at"), TEST_NOW);
        assert_eq!(
            r.str("delete_after"),
            crate::util::plus_hours(TEST_NOW, 24.0)
        );
        assert_eq!(r.str("deleted_at"), "");
        let ev = must_one(
            &s,
            "SELECT * FROM events WHERE action = 'executor.retired' AND subject = 'vm-seven'",
            &[],
        );
        let d = json_map(&ev.str("detail"));
        assert_eq!(d["delete_after"], crate::util::plus_hours(TEST_NOW, 24.0));
        assert_eq!(d["name"], "pulse-cr-007");
        assert_eq!(d["resource_group"], "pulse-hands");
        assert_eq!(d["provider"], "azure");
        let posts = crate::util::must_rows(
            &*s.conn().unwrap(),
            "SELECT * FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'compute'",
            &[],
        );
        assert_eq!(posts.len(), 1, "findings: {posts:?}");
        assert_eq!(
            posts[0].str("summary"),
            format!(
                "retired vm-seven: login refused; Azure VM pulse-cr-007 (pulse-hands) stopped now, deleted at {} unless kept",
                crate::util::plus_hours(TEST_NOW, 24.0)
            )
        );
        let body = posts[0].str("body");
        for want in [
            "What was refused: copilot refuses the login",
            &format!("stopped now ({TEST_NOW})"),
            &format!(
                "deletes the machine and what was created beside it at {} (compute.retire_grace_hours=24)",
                crate::util::plus_hours(TEST_NOW, 24.0)
            ),
            &format!(
                "Undo: `az vm start -g pulse-hands -n pulse-cr-007` then `pulse executor activate vm-seven` before {} keeps it.",
                crate::util::plus_hours(TEST_NOW, 24.0)
            ),
            "A Copilot-only fine-grained token placed with `pulse executor token --with-token` lets the daemon heal future machines instead.",
        ] {
            assert!(body.contains(want), "body lacks {want:?}:\n{body}");
        }
        assert!(lines[0].contains("no login source"), "lines: {lines:?}");
        assert!(
            lines[1].contains("retired vm-seven: stopped pulse-cr-007"),
            "lines: {lines:?}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question'",
                &[]
            ),
            0
        );
        let before = az.joined().len();
        tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert_eq!(az.joined().len(), before);
    }

    #[test]
    fn tend_without_az_is_not_ours() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        new_bare_repo(&s);
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        let az = RetireAz::broken();
        let _az_guard = install_retire_az(&az);
        let lines = tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert!(!az.has("vm deallocate"), "az calls: {:?}", az.joined());
        assert_eq!(
            must_one(
                &s,
                "SELECT status FROM executors WHERE name = 'vm-seven'",
                &[]
            )
            .str("status"),
            "removed"
        );
        let p = must_one(
            &s,
            "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'",
            &[],
        );
        assert!(
            p.str("body").contains(
                "left running because PULSE did not create it (the Azure CLI could not answer:"
            ),
            "finding: {p:?}"
        );
        assert!(
            lines.join("\n").contains("the Azure CLI could not answer"),
            "lines: {lines:?}"
        );
    }

    #[test]
    fn tend_leaves_foreign_machine_running() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        new_bare_repo(&s);
        drained_by_login(&s, "laptop", "pulse@laptop.local");
        let az = RetireAz::default();
        let _az_guard = install_retire_az(&az);
        tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert!(
            !az.has("vm deallocate") && az.has("vm list"),
            "az calls: {:?}",
            az.joined()
        );
        assert_eq!(
            must_one(
                &s,
                "SELECT status FROM executors WHERE name = 'laptop'",
                &[]
            )
            .str("status"),
            "removed"
        );
        assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM retired_vms", &[]), 0);
        let p = must_one(
            &s,
            "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'",
            &[],
        );
        assert_eq!(
            p.str("summary"),
            "retired laptop: login refused; machine left running because PULSE did not create it"
        );
        assert!(p.str("body").contains("left running because PULSE did not create it (no Azure VM tagged pulse=hands is named laptop or answers at laptop.local)"));
    }

    #[test]
    fn tend_heals_from_executor_token() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        new_bare_repo(&s);
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        crate::executor::set_executor_admin(&s, "vm-seven", "pulseadmin@vm-seven-admin");
        let az = RetireAz::with_list(
            r#"[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]"#,
        );
        let _az_guard = install_retire_az(&az);
        write_executor_token(&s);
        let placed = stub_place_login(true);
        let lines = tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        let calls = placed.calls();
        assert_eq!(calls.len(), 1, "placement: {calls:?}");
        assert_eq!(calls[0].name, "vm-seven");
        assert_eq!(calls[0].admin, "pulseadmin@vm-seven-admin");
        assert_eq!(calls[0].token, TEST_EXECUTOR_TOKEN);
        assert_eq!(calls[0].actor, TEST_DAEMON);
        assert!(
            az.joined().is_empty(),
            "az touched while healing: {:?}",
            az.joined()
        );
        let ex = crate::executor::executor_row(&s, "vm-seven");
        assert_eq!(ex.str("status"), "active");
        assert_eq!(ex.str("healed_at"), "");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.healed' AND subject = 'vm-seven'",
                &[]
            ),
            1
        );
        let p = must_one(
            &s,
            "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'executors'",
            &[],
        );
        assert_eq!(
            p.str("summary"),
            "healed vm-seven: login placed from executor token"
        );
        assert!(!p.str("body").contains(TEST_EXECUTOR_TOKEN));
        assert_eq!(
            lines,
            vec!["healed vm-seven: login placed from executor token"]
        );
        assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM retired_vms", &[]), 0);
    }

    #[test]
    fn tend_heal_refused_retires_and_does_not_heal_twice() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        new_bare_repo(&s);
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        let az = RetireAz::with_list(
            r#"[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]"#,
        );
        let _az_guard = install_retire_az(&az);
        write_executor_token(&s);
        let placed = stub_place_login(false);
        set_setting(&s, "compute.self_retire", "0");
        let lines = tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert_eq!(placed.calls().len(), 1);
        assert_eq!(placed.calls()[0].admin, "pulseadmin@vm-seven");
        let ex = crate::executor::executor_row(&s, "vm-seven");
        assert_eq!(ex.str("status"), "drained");
        assert_eq!(ex.str("healed_at"), TEST_NOW);
        assert!(lines.join("\n").contains("heal vm-seven refused"));
        assert!(!az.has("vm deallocate"));
        tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert_eq!(placed.calls().len(), 1);
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.healed'",
                &[]
            ),
            0
        );
        set_setting(&s, "compute.self_retire", "1");
        tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert_eq!(placed.calls().len(), 1);
        assert!(az.has("vm deallocate -g pulse-hands -n pulse-cr-007 -o none"));
        assert_eq!(
            must_one(
                &s,
                "SELECT status FROM executors WHERE name = 'vm-seven'",
                &[]
            )
            .str("status"),
            "removed"
        );
        set_setting(&s, "compute.self_heal", "0");
        set_setting(&s, "compute.self_retire", "0");
        drained_by_login(&s, "vm-eight", "pulse@vm-eight");
        let before = az.joined().len();
        tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert_eq!(placed.calls().len(), 1);
        assert_eq!(az.joined().len(), before);
        assert_eq!(
            crate::executor::executor_row(&s, "vm-eight").str("status"),
            "drained"
        );
    }
    #[test]
    fn tend_postpones_while_a_run_is_in_flight() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        let (_, slug) = new_bare_repo(&s);
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        let az = RetireAz::with_list(
            r#"[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]"#,
        );
        let _az_guard = install_retire_az(&az);
        crate::test_helpers_daemon::exec1(
            &s,
            "INSERT INTO runs (id, job_id, project, agent, status, started_at, executor) VALUES ('run-1', 'job-1', ?, 'aurora', 'running', ?, 'vm-seven')",
            &[&slug, &TEST_NOW],
        );
        let lines = tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert!(
            !az.has("vm deallocate")
                && crate::executor::executor_row(&s, "vm-seven").str("status") == "drained",
            "retired with a run in flight: {lines:?} {:?}",
            az.joined()
        );
        assert!(
            lines.join("\n").contains("retire vm-seven postponed"),
            "lines: {lines:?}"
        );
    }
    #[test]
    fn tend_deletes_after_grace_and_keeps_what_the_owner_revived() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        new_bare_repo(&s);
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        drained_by_login(&s, "vm-nine", "pulse@vm-nine");
        let az = RetireAz::with_list(
            r#"[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]"#,
        );
        let _az_guard = install_retire_az(&az);
        tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM retired_vms WHERE deleted_at = ''",
                &[]
            ),
            2
        );
        // The owner starts vm-nine's machine again and activates it before the grace ends: the row revives.
        let _probe = crate::test_helpers_hands::install_run_ssh(|_, _argv, _stdin, _| {
            Ok((0, "hands=ok git=ok keys=- keys_ok=yes remotes=0 workdir=/srv/pulse harness=yes pulse=1.2.3 disk_free_mb=20000\n".into(), String::new()))
        });
        let out = invoke_ok(&s, "executor", "", &["activate", "vm-nine"]);
        assert!(out.contains("vm-nine"), "activate: {out:?}");
        let ex = crate::executor::executor_row(&s, "vm-nine");
        assert!(
            !ex.0.is_empty() && ex.str("status") == "active",
            "revived executor: {ex:?}"
        );

        // Before the grace ends nothing is deleted.
        az.reset();
        tend_executors(
            &s,
            crate::util::plus_hours(TEST_NOW, 23.0),
            TEST_DAEMON.into(),
        );
        assert!(az.joined().is_empty(), "deleted early: {:?}", az.joined());

        // At the grace: vm-seven's machine goes (VM, NIC, public IP, OS disk; never the VNet or NSG); vm-nine is kept.
        let later = crate::util::plus_hours(TEST_NOW, 24.0);
        let lines = tend_executors(&s, later.clone(), TEST_DAEMON.into());
        let got = az.joined();
        assert!(
            got.first()
                .is_some_and(|c| c == "vm show -g pulse-hands -n pulse-cr-007 -o json"),
            "delete sequence: {got:?}"
        );
        assert!(
            got.iter()
                .any(|c| c == "vm delete -g pulse-hands -n pulse-cr-007 --yes -o none"),
            "delete sequence: {got:?}"
        );
        assert!(
            got.iter()
                .all(|c| !c.contains("vnet") && !c.contains("nsg") && !c.contains("NSG"))
        );
        let seven = must_one(
            &s,
            "SELECT * FROM retired_vms WHERE executor = 'vm-seven'",
            &[],
        );
        assert_eq!(seven.str("deleted_at"), later);
        assert_eq!(seven.str("note"), "");
        let nine = must_one(
            &s,
            "SELECT * FROM retired_vms WHERE executor = 'vm-nine'",
            &[],
        );
        assert_eq!(nine.str("deleted_at"), later);
        assert_eq!(nine.str("note"), "kept by the owner");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.deleted' AND subject = 'vm-seven'",
                &[]
            ),
            1
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.kept' AND subject = 'vm-nine'",
                &[]
            ),
            1
        );
        assert!(
            lines
                .join("\n")
                .contains("deleted pulse-cr-007 (pulse-hands):")
        );
        assert!(lines.join("\n").contains("kept vm-nine"));
        az.reset();
        tend_executors(&s, crate::util::plus_hours(&later, 1.0), TEST_DAEMON.into());
        assert!(az.joined().is_empty(), "deleted twice: {:?}", az.joined());
    }

    #[test]
    fn tend_delete_closes_a_missing_vm() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        new_bare_repo(&s);
        let az = RetireAz::missing();
        let _az_guard = install_retire_az(&az);
        s.conn().unwrap().execute("INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after) VALUES ('vm-gone', 'azure', 'pulse-hands', 'pulse-cr-gone', ?, ?)", params![TEST_NOW, TEST_NOW]).unwrap();
        let lines = tend_executors(&s, TEST_NOW.into(), TEST_DAEMON.into());
        assert_eq!(
            az.joined(),
            vec!["vm show -g pulse-hands -n pulse-cr-gone -o json"]
        );
        let r = must_one(
            &s,
            "SELECT * FROM retired_vms WHERE executor = 'vm-gone'",
            &[],
        );
        assert_eq!(r.str("deleted_at"), TEST_NOW);
        assert_eq!(r.str("note"), "vm not found at delete time");
        assert!(lines.join("\n").contains("was already gone"));
    }
    #[test]
    fn daemon_tick_tends_executors() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        new_bare_repo(&s);
        drained_by_login(&s, "vm-seven", "pulse@vm-seven");
        let az = RetireAz::with_list(
            r#"[{"name":"pulse-cr-007","rg":"pulse-hands","ip":"203.0.113.7"}]"#,
        );
        let _az_guard = install_retire_az(&az);
        let log = once_daemon(
            &s,
            crate::daemon::DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: nothing'".into(),
                ..Default::default()
            },
        );
        assert!(
            log.contains("retired vm-seven: stopped pulse-cr-007 (pulse-hands)"),
            "log:\n{log}"
        );
        assert!(
            az.has("vm deallocate -g pulse-hands -n pulse-cr-007 -o none"),
            "az calls: {:?}",
            az.joined()
        );
    }
    #[test]
    fn compute_held_without_login_source_and_explained_once_per_day() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        pulse_bus::reset_default();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        for (k, v) in [
            ("compute.provider", "azure"),
            ("compute.max_vms", "2"),
            ("compute.max_hours_per_day", "24"),
            ("compute.azure.resource_group", "rg"),
            ("compute.azure.location", "westeurope"),
            ("compute.azure.vm_size", "Standard_B2ms"),
            ("compute.auto_approve", "1"),
        ] {
            set_setting(&s, k, v);
        }
        let _az = install_run_az(|_, _, _| {
            panic!("az was called without a login source");
        });
        // Auto-approval holds: the request stays pending and its proposal says why.
        let req = pulse_tasks::request_compute(&s, "proj", "aurora", "", 2, "long run").unwrap();
        assert!(
            req.status == "pending" && !req.proposal_post.is_empty(),
            "auto-approved without a source: {req:?}"
        );
        let p = must_one(
            &s,
            "SELECT body FROM posts WHERE id = ?",
            &[&req.proposal_post],
        );
        assert!(
            p.str("body").contains(pulse_tasks::NO_LOGIN_SOURCE),
            "proposal: {p:?}"
        );
        let caps = pulse_tasks::get_caps(&s).unwrap_or_default();
        assert!(!caps.login_source, "caps: {caps:?}");
        // The worker explains the hold once a day, and never buys.
        let cancel = pulse_bus::CancellationToken::new();
        let w =
            crate::compute::new_compute_worker(&cancel, &s, crate::compute::ComputeOpts::default());
        w.tick(&cancel, TEST_NOW);
        w.wait(std::time::Duration::ZERO);
        w.tick(&cancel, &crate::util::plus_hours(TEST_NOW, 1.0));
        w.wait(std::time::Duration::ZERO);
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = 'compute'",
            &[],
        );
        assert_eq!(n, 1, "hold findings on one day: {n}");
        let f = must_one(
            &s,
            "SELECT * FROM posts WHERE kind = 'finding' AND thread = 'compute'",
            &[],
        );
        assert!(
            f.str("summary").starts_with(&format!(
                "{}: compute {} waits; place a Copilot-only token with `pulse executor token --with-token`",
                pulse_tasks::NO_LOGIN_SOURCE,
                req.id
            )),
            "summary: {:?}",
            f.str("summary")
        );
        // An approved request (the owner said yes) is held the same way, not failed.
        pulse_tasks::approve(&s, &req.id, "fine", pulse_store::HUMAN_AGENT).unwrap();
        w.tick(&cancel, &crate::util::plus_hours(TEST_NOW, 2.0));
        w.wait(std::time::Duration::ZERO);
        let got = pulse_tasks::get_compute(&s, &req.id).unwrap_or_default();
        assert_eq!(got.status, "approved", "held request: {got:?}");
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = 'compute'",
            &[],
        );
        assert_eq!(n, 1, "hold findings on one day: {n}");
        // A new day, a fresh worker (as after a daemon restart): one more note, still no machine.
        let next_day = crate::util::plus_hours(TEST_NOW, 24.0);
        let later = pulse_store::Store::open(pulse_store::Options {
            home: s.home.clone(),
            now: next_day.clone(),
            ..Default::default()
        })
        .unwrap();
        let w2 = crate::compute::new_compute_worker(
            &cancel,
            &later,
            crate::compute::ComputeOpts::default(),
        );
        w2.tick(&cancel, &next_day);
        w2.wait(std::time::Duration::ZERO);
        w2.tick(&cancel, &crate::util::plus_hours(&next_day, 1.0));
        w2.wait(std::time::Duration::ZERO);
        let n = count(
            &later,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = 'compute'",
            &[],
        );
        assert_eq!(n, 2, "hold findings over two days: {n}");
        let got = pulse_tasks::get_compute(&later, &req.id).unwrap_or_default();
        assert_eq!(got.status, "approved", "held request: {got:?}");
        // With a source the same request goes on to provisioning (az answers, so it fails there - not a hold).
        // The hands binary is faked: the real default cross-builds pulse from the checkout on any
        // machine with the musl target installed, minutes of release build inside a unit test.
        write_executor_token(&later);
        let old_hands = crate::pair::set_hands_binary(Box::new(|_| {
            Ok((b"fake pulse".to_vec(), "fixture".to_string()))
        }));
        let _az_2 = install_run_az(|_, _, _| Ok((1, String::new(), "no subscription".into())));
        w2.tick(&cancel, &crate::util::plus_hours(&next_day, 2.0));
        w2.wait(std::time::Duration::ZERO);
        let _ = crate::pair::set_hands_binary(old_hands);
        let got = pulse_tasks::get_compute(&later, &req.id).unwrap_or_default();
        assert_eq!(got.status, "failed", "provisioning reached az: {got:?}");
        later.close().unwrap();
    }

    #[test]
    fn caps_allow_holds_without_login_source() {
        let _serial = crate::test_helpers::serial();
        let mut caps = pulse_tasks::Caps {
            max_vms: 2,
            max_hours_per_day: 24,
            login_source: false,
            ..Default::default()
        };
        let req = pulse_tasks::ComputeRequest {
            hours: 1,
            at: TEST_NOW.into(),
            ..Default::default()
        };
        assert_eq!(
            crate::compute::caps_allow(caps.clone(), req.clone(), TEST_NOW),
            pulse_tasks::NO_LOGIN_SOURCE
        );
        caps.login_source = true;
        assert_eq!(
            crate::compute::caps_allow(caps.clone(), req.clone(), TEST_NOW),
            ""
        );
        caps.login_source = false;
        caps.max_vms = 0;
        assert!(crate::compute::caps_allow(caps, req, TEST_NOW).starts_with("cap:"));
    }

    #[test]
    fn executor_login_records_admin_route() {
        let _serial = crate::test_helpers::serial();
        let _lock = crate::test_helpers_provider::provider_test_lock();
        let s = new_store();
        crate::executor::add_executor(
            &s,
            TEST_NOW,
            "vm-seven",
            "pulse@vm-seven",
            "/srv/pulse",
            1,
            "*",
            pulse_store::HUMAN_AGENT,
        )
        .unwrap();
        let old = crate::executor::set_run_ssh(|_ctx, _argv, _stdin, _timeout| {
            Ok((0, String::new(), String::new()))
        });
        let mut empty = std::io::empty();
        let res = crate::executor::executor_login(
            &s,
            TEST_NOW,
            "vm-seven",
            "pulseadmin@vm-seven-admin",
            &mut empty,
            false,
            true,
            pulse_store::HUMAN_AGENT,
        );
        crate::executor::restore_run_ssh(old);
        res.unwrap();
        assert_eq!(
            crate::executor::executor_row(&s, "vm-seven").str("admin"),
            "pulseadmin@vm-seven-admin"
        );
    }
    #[test]
    fn admin_route_prefers_the_executor_host() {
        let _serial = crate::test_helpers::serial();
        for (address, ip, want) in [
            ("pulse@pulse-vm1", "20.0.0.1", "pulseadmin@pulse-vm1"),
            ("pulse-vm1", "20.0.0.1", "pulseadmin@pulse-vm1"),
            ("pulse@", "20.0.0.1", "pulseadmin@20.0.0.1"),
            ("", "20.0.0.1", "pulseadmin@20.0.0.1"),
            ("", "", ""),
        ] {
            assert_eq!(
                admin_route(address.into(), ip.into()),
                want,
                "{address}/{ip}"
            );
        }
    }
}
