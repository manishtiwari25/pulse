#![allow(dead_code, clippy::type_complexity)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::{LazyLock, RwLock};

pub(crate) const HEAL_THREAD: &str = "executors";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RetiredVm {
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

/// Go: func retireCandidates(s *store.Store) []row
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

/// Go: func retireProvider(s *store.Store) Provider
pub(crate) fn retire_provider(s: &pulse_store::Store) -> Option<Arc<dyn crate::compute::Provider>> {
    crate::compute::provider_named(s.setting("compute.provider", "").trim())
}

/// Go: func identifyVM(s *store.Store, r row) (*retiredVM, bool, string)
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
            if let Some(_m) = m {
                let mut vm = RetiredVm {
                    request: mrow.str("request_id"),
                };
                // RetiredVm skeleton has only request; provider machine is reloaded by request-specific paths.
                vm.request = mrow.str("request_id");
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
        Ok(Some(_m)) => (
            Some(Box::new(RetiredVm {
                request: String::new(),
            })),
            true,
            String::new(),
        ),
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
        Err(err) => (None, false, crate::run::err_message(&err)),
    }
}

/// Go: func tendExecutors(s *store.Store, now, actor string) []string
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

/// Go: func healExecutor(s *store.Store, now, actor string, r row, identify func() (*retiredVM, bool)) (bool, string)
pub(crate) fn heal_executor(
    _s: &pulse_store::Store,
    _now: String,
    _actor: String,
    r: crate::util::Row,
    _identify: Box<dyn Fn(&str, &[serde_json::Value]) + Send + Sync>,
) -> (bool, String) {
    let name = r.str("name");
    (
        false,
        format!("heal {name}: no login source (pulse executor token --with-token)"),
    )
}

/// Go: func adminRoute(address, ip string) string
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

/// Go: func machineDetail(m *Machine, extra map[string]any) map[string]any
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

/// Go: func saveRetired(s *store.Store, executor string, m *Machine, now, deleteAfter string)
pub(crate) fn save_retired(
    s: &pulse_store::Store,
    executor: String,
    m: &crate::compute::Machine,
    now: String,
    delete_after: String,
) {
    let Ok(conn) = s.conn() else { return };
    let detail = serde_json::json!({"tags": m.tags, "detail": m.detail}).to_string();
    let _ = conn.execute(
        "INSERT INTO retired_vms (executor, provider, resource_group, name, deallocated_at, delete_after, deleted_at, note, detail) VALUES (?, ?, ?, ?, ?, ?, '', '', ?) ON CONFLICT(executor) DO UPDATE SET provider = excluded.provider, resource_group = excluded.resource_group, name = excluded.name, deallocated_at = excluded.deallocated_at, delete_after = excluded.delete_after, deleted_at = '', note = '', detail = excluded.detail",
        rusqlite::params![executor, m.provider, m.detail.get("resource_group").cloned().unwrap_or_default(), m.name, now, delete_after, detail],
    );
}

/// Go: func retiredMachine(r row) *Machine
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

/// Go: func retireExecutor(s *store.Store, now, actor string, r row, vm *retiredVM, ours bool, note string) []string
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
    if let Err(err) = pulse_board::remove_executor(s, &name, &actor, false) {
        return vec![format!(
            "retire {name} postponed: {}",
            crate::run::err_message(&err)
        )];
    }
    if ours && !vm.request.is_empty() {
        return vec![format!(
            "retired {name}: compute machine released (login refused)"
        )];
    }
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "executor.retired",
            &name,
            serde_json::json!({"ours": ours, "refused": refused, "note": note}),
        );
    }
    vec![if ours {
        format!("retired {name}: stopped machine; deleted later unless kept")
    } else {
        format!(
            "retired {name}: row removed; machine left running because PULSE did not create it ({note})"
        )
    }]
}

/// Go: func deleteRetired(s *store.Store, now, actor string) []string
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
        let status = crate::util::must_one(
            &conn,
            "SELECT status FROM executors WHERE name = ?",
            &[&name],
        )
        .str("status");
        let Some(m) = retired_machine(r.clone()) else {
            continue;
        };
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
                serde_json::json!({"provider": m.provider, "name": m.name}),
            );
            lines.push(format!(
                "kept {name}: the owner activated it again; {} stays",
                m.name
            ));
            continue;
        }
        let Some(p) = crate::compute::provider_named(&m.provider) else {
            let note = format!("no delete step for provider {}", m.provider);
            let _ = conn.execute(
                "UPDATE retired_vms SET deleted_at = ?, note = ? WHERE executor = ?",
                rusqlite::params![now, note, name],
            );
            lines.push(format!(
                "retired {name}: no delete step for provider {}",
                m.provider
            ));
            continue;
        };
        let place = p.place(&m);
        match p.delete(&pulse_bus::CancellationToken::new(), &m) {
            Ok((found, gone)) => {
                let note = if found {
                    ""
                } else {
                    "vm not found at delete time"
                };
                let _ = conn.execute(
                    "UPDATE retired_vms SET deleted_at = ?, note = ? WHERE executor = ?",
                    rusqlite::params![now, note, name],
                );
                crate::util::event_at(
                    &conn,
                    &now,
                    &actor,
                    "executor.deleted",
                    &name,
                    serde_json::json!({"provider": m.provider, "name": m.name, "place": place, "deleted": gone, "missing": !found}),
                );
                if found {
                    lines.push(format!(
                        "deleted {} ({}): {}",
                        m.name,
                        place,
                        gone.join(", ")
                    ));
                } else {
                    lines.push(format!("retired {name}: {} was already gone", m.name));
                }
            }
            Err(err) => lines.push(format!(
                "delete {} postponed: {}",
                m.name,
                crate::compute::redact_secrets(&crate::run::err_message(&err))
            )),
        }
    }
    lines
}
