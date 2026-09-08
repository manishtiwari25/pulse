"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { Input } from "@/components/ui/input";
import { EmptyState, ErrorNotice, Notice, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { Mono } from "@/components/pulse/who";
import { SettingRow } from "@/components/settings/setting-row";
import type { Setting, SettingsPayload } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

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
    blurb: "Caps for machines agents may ask for, and the Azure target they are created in. Credentials never live here: the hub machine's az login is the auth. A machine whose Copilot login is refused is the daemon's to mend or retire (ADR-034): it places the executor token you put on file with pulse executor token, else deallocates the VM PULSE created, retires it and deletes it after the grace hours - a finding each time, never a question.",
    keys: ["compute.provider", "compute.max_vms", "compute.max_hours_per_day", "compute.auto_approve", "compute.budget_credits", "compute.self_heal", "compute.self_retire", "compute.retire_grace_hours", "compute.azure.subscription", "compute.azure.resource_group", "compute.azure.location", "compute.azure.vm_size", "compute.azure.image"],
    match: (n) => n.startsWith("compute."),
    clusters: [
      { title: "Caps", keys: ["compute.provider", "compute.max_vms", "compute.max_hours_per_day", "compute.auto_approve", "compute.budget_credits"] },
      { title: "Refused logins", keys: ["compute.self_heal", "compute.self_retire", "compute.retire_grace_hours"] },
      { title: "Azure target", keys: ["compute.azure.subscription", "compute.azure.resource_group", "compute.azure.location", "compute.azure.vm_size", "compute.azure.image"] },
    ],
  },
  {
    id: "evolve",
    title: "Self-improvement loop",
    blurb: "Bounds on pulse-evolve. The held-out directory is yours alone: agents never see its path. With evolve.heldout on auto the daemon creates the set itself - a private folder under the store and two fixtures written by a sealed model call that sees no repository, board or store - and labels them generated (ADR-033). On owner, you write them from the Held-out fixtures dialog on a project's Self-improvement panel, or with pulse evolve setup DIR; a fixture you write is never overwritten either way.",
    keys: ["evolve.loop", "evolve.limits", "evolve.tolerance", "evolve.heldout", "evolve.heldout_dir"],
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
  { name: "compute.self_heal", value: "1", default: "1", kind: "switch", min: null, choices: null, description: "1: a refused login is replaced from the executor token on file (pulse executor token); 0: never" },
  { name: "compute.self_retire", value: "1", default: "1", kind: "switch", min: null, choices: null, description: "1: a VM PULSE created whose login stays refused is deallocated and retired, then deleted after the grace hours; 0: it is left as it is" },
  { name: "compute.retire_grace_hours", value: "24", default: "24", kind: "count", min: 1, choices: null, description: "hours a retired VM stays deallocated before it is deleted; az vm start and pulse executor activate within them keep it" },
  { name: "compute.azure.subscription", value: "", default: "", kind: "identifier", min: null, choices: null, description: "Azure subscription id the VMs are created in" },
  { name: "compute.azure.resource_group", value: "", default: "", kind: "identifier", min: null, choices: null, description: "resource group that holds the executor VMs" },
  { name: "compute.azure.location", value: "", default: "", kind: "identifier", min: null, choices: null, description: "Azure region, e.g. westeurope" },
  { name: "compute.azure.vm_size", value: "Standard_B2ms", default: "Standard_B2ms", kind: "identifier", min: null, choices: null, description: "VM size used when a request does not name one" },
  { name: "compute.azure.image", value: "Ubuntu2404", default: "Ubuntu2404", kind: "identifier", min: null, choices: null, description: "image alias the VMs boot from" },
  { name: "evolve.loop", value: "0", default: "0", kind: "switch", min: null, choices: null, description: "1 lets pulse-evolve run cycles back to back; 0 keeps the loop off" },
  { name: "evolve.limits", value: "none", default: "none", kind: "text", min: null, choices: null, description: "caps the constitution writes for the loop, or none" },
  { name: "evolve.tolerance", value: "0.02", default: "0.02", kind: "text", min: null, choices: null, description: "fraction a measured check may regress before a cycle is discarded" },
  { name: "evolve.heldout", value: "auto", default: "auto", kind: "choice", min: null, choices: ["auto", "owner"], description: "auto: the daemon writes the private held-out set itself, sealed and labelled generated; owner: you write it from the console (ADR-033)" },
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
  "compute.self_heal": { on: "on - a refused login is replaced from the executor token", off: "off - a refused login stays refused" },
  "compute.self_retire": { on: "on - a refused VM PULSE created is deallocated, retired, then deleted", off: "off - a refused VM is left running" },
  "evolve.loop": { on: "on - cycles run back to back", off: "off - the loop waits" },
};

// The sticky stack above the content: 48px app bar + the title row and the
// section pills. Anchored sections scroll to sit just under it.
const ANCHOR = "scroll-mt-32";
const PILL = "inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full border px-2.5 py-0.5 hover:bg-muted";

function GroupRows({ g, rows, canEdit }: { g: Group; rows: Setting[]; canEdit: boolean }) {
  const row = (s: Setting) => <SettingRow key={s.name} s={s} canEdit={canEdit} labels={SWITCH_LABELS[s.name]} />;
  const clusters = g.clusters;
  if (!clusters) return <div className="divide-y">{rows.map(row)}</div>;
  const loose = rows.filter((s) => !clusters.some((c) => c.keys.includes(s.name)));
  return (
    <>
      {clusters.map((c) => {
        const inCluster = rows.filter((s) => c.keys.includes(s.name));
        if (!inCluster.length) return null;
        return (
          <div key={c.title} className="border-t first:border-t-0">
            <h3 className="px-4 pt-2.5 pb-0.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{c.title}</h3>
            <div className="divide-y">{inCluster.map(row)}</div>
          </div>
        );
      })}
      {loose.length ? <div className="divide-y border-t first:border-t-0">{loose.map(row)}</div> : null}
    </>
  );
}

export default function SettingsPage() {
  const q = useApi<SettingsPayload>("settings", { refresh: 30000 });
  const [find, setFind] = useState("");
  const jumped = useRef(false);
  useEffect(() => {
    // The sections exist only once settings arrive, so the browser's own
    // #hash jump on load finds nothing; do it once here.
    if (jumped.current || !q.data) return;
    jumped.current = true;
    const id = window.location.hash.slice(1);
    if (id) document.getElementById(id)?.scrollIntoView();
  }, [q.data]);
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
  const stopped = stop?.value === "1";
  const needle = find.trim().toLowerCase();
  const match = (s: Setting) => !needle || s.name.toLowerCase().includes(needle) || s.description.toLowerCase().includes(needle);
  // The kill switch has its own card at the top, so it leaves the runner cluster.
  const killShown = !!stop && match(stop);
  const base = groups.map(({ g, rows }) => ({ g, rows: stop ? rows.filter((s) => s.name !== "stop") : rows }));
  const total = base.reduce((n, x) => n + x.rows.length, 0) + (stop ? 1 : 0);
  const shown = base.map(({ g, rows }) => ({ g, all: rows.length, rows: rows.filter(match) })).filter((x) => x.rows.length);
  const hit = new Set(shown.map((x) => x.g.id));
  const matched = shown.reduce((n, x) => n + x.rows.length, 0) + (killShown ? 1 : 0);
  return (
    <>
      <PageTitle
        sub={<>The owner&apos;s caps, as the store holds them. Every change is recorded with who made it; the daemon reads them on its next loop. Same keys as <Mono>pulse setting set</Mono>.</>}
        actions={
          <>
            {needle ? <span className="tnum whitespace-nowrap text-xs text-muted-foreground">{matched} of {total}</span> : null}
            <Input
              value={find}
              onChange={(e) => setFind(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") setFind("");
              }}
              placeholder="find a setting…"
              aria-label="Find a setting"
              className="h-7 w-36 sm:w-56"
            />
          </>
        }
        bar={
          <nav aria-label="Sections" className="flex items-center gap-1.5 text-xs">
            {stop ? (
              <a href="#stop" className={cn(PILL, stopped && "border-bad/40 text-bad", !killShown && "pointer-events-none opacity-40")} aria-disabled={!killShown || undefined}>
                <span className={cn("size-1.5 rounded-full", stopped ? "bg-bad" : "bg-ok")} aria-hidden />
                Kill switch
              </a>
            ) : null}
            {groups.map(({ g }) => (
              <a key={g.id} href={"#" + g.id} className={cn(PILL, !hit.has(g.id) && "pointer-events-none opacity-40")} aria-disabled={!hit.has(g.id) || undefined}>
                {g.title}
              </a>
            ))}
          </nav>
        }
      >
        Settings
      </PageTitle>
      {!d.available ? <Notice tone="warn" className="mb-3">The settings store is not available on this hub - it runs read-only.</Notice> : null}
      {d.available && !d.can_edit ? <Notice tone="info" className="mb-3">This hub is read-only for settings. Change them on the owner&apos;s machine.</Notice> : null}
      {stopped ? <Notice tone="bad" className="mb-3">STOP is on: the daemon runs nothing until you turn it off below.</Notice> : null}
      <div className="grid gap-3">
        {killShown && stop ? (
          <section id="stop" className={ANCHOR}>
            <Panel dense className={cn(stopped && "bg-bad/5 ring-bad/40")}>
              <SettingRow s={stop} canEdit={d.can_edit} labels={SWITCH_LABELS.stop} title="Kill switch" />
            </Panel>
          </section>
        ) : null}
        {shown.map(({ g, rows, all }) => (
          <section key={g.id} id={g.id} className={ANCHOR}>
            <Panel
              dense
              title={
                <>
                  {g.title}
                  {needle ? <span className="tnum ml-2 font-normal text-muted-foreground">{rows.length} of {all}</span> : null}
                  <span className="mt-0.5 block text-[12px] font-normal leading-snug tracking-normal text-muted-foreground anywhere">{g.blurb}</span>
                </>
              }
              contentClassName="pb-1"
            >
              <GroupRows g={g} rows={rows} canEdit={d.can_edit} />
            </Panel>
          </section>
        ))}
        {needle && !matched ? <EmptyState>No setting matches &ldquo;{find.trim()}&rdquo; - try part of a key such as quorum or azure.</EmptyState> : null}
      </div>
    </>
  );
}
