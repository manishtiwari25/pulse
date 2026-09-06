"use client";

import { useMemo } from "react";
import { ErrorNotice, Notice, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { Mono } from "@/components/pulse/who";
import { SettingRow } from "@/components/settings/setting-row";
import type { Setting, SettingsPayload } from "@/lib/api";
import { useApi } from "@/lib/use-api";

interface Group {
  id: string;
  title: string;
  blurb: string;
  keys: string[];
  match?: (name: string) => boolean;
  // Clusters inside a section: a heading and the keys under it.
  clusters?: { title: string; keys: string[] }[];
}

// Three sections, as the store groups them: the 16 legacy rows every store
// has (Runner caps), then store.ExtraSettingNames() - compute.* and
// evolve.* - which are not written at Create. Unknown keys fall into
// "Other" so a new cap the daemon learns never disappears from the owner's
// view.
const RUNNER_CLUSTERS = [
  { title: "The machine", keys: ["stop", "workers", "parallel_per_project", "jobs_per_day", "minutes_per_job", "max_ai_credits", "runner_cmd", "prune_after_days"] },
  { title: "The worker", keys: ["worker_model", "landing"] },
  { title: "The board", keys: ["quorum", "expiry_days", "stale_days", "claim_hours"] },
  { title: "Amendments", keys: ["amend_quorum", "amend_majority"] },
];
const GROUPS: Group[] = [
  {
    id: "runner",
    title: "Runner caps",
    blurb: "How hard the daemon may work, what each job runs with, and the board's thresholds. stop halts everything at once.",
    keys: RUNNER_CLUSTERS.flatMap((c) => c.keys),
    clusters: RUNNER_CLUSTERS,
  },
  {
    id: "compute",
    title: "Compute (Azure)",
    blurb: "Caps for machines agents may ask for, and the Azure target they are created in. Credentials never live here: the hub machine's az login is the auth.",
    keys: ["compute.provider", "compute.max_vms", "compute.max_hours_per_day", "compute.auto_approve", "compute.budget_credits", "compute.azure.subscription", "compute.azure.resource_group", "compute.azure.location", "compute.azure.vm_size", "compute.azure.image"],
    match: (n) => n.startsWith("compute."),
    clusters: [
      { title: "Caps", keys: ["compute.provider", "compute.max_vms", "compute.max_hours_per_day", "compute.auto_approve", "compute.budget_credits"] },
      { title: "Azure target", keys: ["compute.azure.subscription", "compute.azure.resource_group", "compute.azure.location", "compute.azure.vm_size", "compute.azure.image"] },
    ],
  },
  {
    id: "evolve",
    title: "Self-improvement loop",
    blurb: "Bounds on pulse-evolve. The held-out directory is yours alone: agents never see its path.",
    keys: ["evolve.loop", "evolve.limits", "evolve.tolerance", "evolve.heldout_dir"],
    match: (n) => n.startsWith("evolve."),
  },
];

// Mirrors store.ComputeSettings and store.EvolveSettings in the Go store.
// These rows are not inserted at Create, so when the hub omits one the
// screen still lists it at its effective (default) value and the owner can
// set it from here.
const EXTRA_DEFAULTS: Omit<Setting, "updated_at" | "updated_by">[] = [
  { name: "compute.provider", value: "azure", default: "azure", kind: "choice", min: null, choices: ["azure"], description: "which cloud the compute requests provision on; auth is the hub machine's az login" },
  { name: "compute.max_vms", value: "0", default: "0", kind: "count", min: 0, choices: null, description: "VMs that may exist at once across all requests; 0 means compute is off" },
  { name: "compute.max_hours_per_day", value: "0", default: "0", kind: "count", min: 0, choices: null, description: "VM-hours the runner may approve per UTC day; 0 means none" },
  { name: "compute.auto_approve", value: "0", default: "0", kind: "switch", min: null, choices: null, description: "1 lets within-cap requests start without the owner; 0 holds every request for a decision" },
  { name: "compute.budget_credits", value: "0", default: "0", kind: "count", min: 0, choices: null, description: "credits the compute requests may spend in total; 0 means no budget" },
  { name: "compute.azure.subscription", value: "", default: "", kind: "identifier", min: null, choices: null, description: "Azure subscription id the VMs are created in" },
  { name: "compute.azure.resource_group", value: "", default: "", kind: "identifier", min: null, choices: null, description: "resource group that holds the executor VMs" },
  { name: "compute.azure.location", value: "", default: "", kind: "identifier", min: null, choices: null, description: "Azure region, e.g. westeurope" },
  { name: "compute.azure.vm_size", value: "Standard_B2ms", default: "Standard_B2ms", kind: "identifier", min: null, choices: null, description: "VM size used when a request does not name one" },
  { name: "compute.azure.image", value: "Ubuntu2404", default: "Ubuntu2404", kind: "identifier", min: null, choices: null, description: "image alias the VMs boot from" },
  { name: "evolve.loop", value: "0", default: "0", kind: "switch", min: null, choices: null, description: "1 lets pulse-evolve run cycles back to back; 0 keeps the loop off" },
  { name: "evolve.limits", value: "none", default: "none", kind: "text", min: null, choices: null, description: "caps the constitution writes for the loop, or none" },
  { name: "evolve.tolerance", value: "0.02", default: "0.02", kind: "text", min: null, choices: null, description: "fraction a measured check may regress before a cycle is discarded" },
  // Owner-private: the hub answers "[owner-private]" instead of the real path once set.
  { name: "evolve.heldout_dir", value: "", default: "", kind: "identifier", min: null, choices: null, description: "path to the owner's held-out evaluators; never shown to agents", secret: true },
];

function withExtraDefaults(rows: Setting[]): Setting[] {
  const have = new Set(rows.map((s) => s.name));
  const missing = EXTRA_DEFAULTS.filter((d) => !have.has(d.name)).map((d) => ({ ...d, updated_at: "", updated_by: "default" }));
  return missing.length ? [...rows, ...missing] : rows;
}

const SWITCH_LABELS: Record<string, { on: string; off: string }> = {
  stop: { on: "STOPPED - nothing runs", off: "running" },
  "compute.auto_approve": { on: "on - within-cap requests start without you", off: "off - every request waits for you" },
  "evolve.loop": { on: "on - cycles run back to back", off: "off - the loop waits" },
};

export default function SettingsPage() {
  const q = useApi<SettingsPayload>("settings", { refresh: 30000 });
  const groups = useMemo(() => {
    const all = q.data ? withExtraDefaults(q.data.settings) : [];
    const used = new Set<string>();
    const out: { g: Group; rows: Setting[] }[] = [];
    for (const g of GROUPS) {
      const rows: Setting[] = [];
      for (const k of g.keys) {
        const s = all.find((x) => x.name === k);
        if (s) {
          rows.push(s);
          used.add(s.name);
        }
      }
      if (g.match) for (const s of all) if (!used.has(s.name) && g.match(s.name)) { rows.push(s); used.add(s.name); }
      if (rows.length) out.push({ g, rows });
    }
    const rest = all.filter((s) => !used.has(s.name)).sort((a, b) => a.name.localeCompare(b.name));
    if (rest.length) out.push({ g: { id: "other", title: "Other", blurb: "Keys the store knows that this UI has no group for yet.", keys: [] }, rows: rest });
    return out;
  }, [q.data]);

  if (q.error && !q.data)
    return (
      <>
        <PageTitle>Settings</PageTitle>
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      </>
    );
  if (!q.data) return <SkeletonPage rows={4} />;
  const d = q.data;
  const stop = d.settings.find((s) => s.name === "stop");
  return (
    <>
      <PageTitle sub={<>The owner&apos;s caps, as the store holds them. Every change is recorded with who made it; the daemon reads them on its next loop. Same keys as <Mono>pulse setting set</Mono>.</>}>Settings</PageTitle>
      {!d.available ? <Notice tone="warn">The settings store is not available on this hub - it runs read-only.</Notice> : null}
      {d.available && !d.can_edit ? <Notice tone="info">This hub is read-only for settings. Change them on the owner&apos;s machine.</Notice> : null}
      {stop?.value === "1" ? <Notice tone="bad">STOP is on: the daemon runs nothing until you turn it off below.</Notice> : null}
      <nav aria-label="Sections" className="mb-4 flex flex-wrap gap-1.5 text-xs">
        {groups.map(({ g }) => (
          <a key={g.id} href={"#" + g.id} className="rounded-full border px-2.5 py-1 hover:bg-muted">
            {g.title}
          </a>
        ))}
      </nav>
      <div className="grid gap-4">
        {groups.map(({ g, rows }) => (
          <section key={g.id} id={g.id} className="scroll-mt-16">
            <Panel title={g.title} actions={<span className="max-w-md text-right anywhere">{g.blurb}</span>}>
              {g.clusters ? (
                <>
                  {g.clusters.map((c) => {
                    const inCluster = rows.filter((s) => c.keys.includes(s.name));
                    if (!inCluster.length) return null;
                    return (
                      <div key={c.title}>
                        <h3 className="px-3 pt-3 pb-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{c.title}</h3>
                        {inCluster.map((s) => (
                          <SettingRow key={s.name} s={s} canEdit={d.can_edit} labels={SWITCH_LABELS[s.name]} />
                        ))}
                      </div>
                    );
                  })}
                  {rows
                    .filter((s) => !g.clusters!.some((c) => c.keys.includes(s.name)))
                    .map((s) => (
                      <SettingRow key={s.name} s={s} canEdit={d.can_edit} labels={SWITCH_LABELS[s.name]} />
                    ))}
                </>
              ) : (
                rows.map((s) => <SettingRow key={s.name} s={s} canEdit={d.can_edit} labels={SWITCH_LABELS[s.name]} />)
              )}
            </Panel>
          </section>
        ))}
      </div>
    </>
  );
}
