"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { ActivityIcon, CloudIcon, GaugeIcon, PlugIcon, PowerIcon, RefreshCwIcon, SettingsIcon, SparklesIcon, UserRoundIcon } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Clamp } from "@/components/pulse/clamp";
import { EmptyState, ErrorNotice, Notice, PageTitle, SkeletonPage } from "@/components/pulse/panel";
import { Mono } from "@/components/pulse/who";
import { RunningStrip } from "@/components/settings/running";
import { CredentialRow } from "@/components/settings/credential";
import { HarnessSettingsCard, isHarnessSetting } from "@/components/settings/harness";
import { SettingRow } from "@/components/settings/setting-row";
import type { Setting, SettingsPayload } from "@/lib/api";
import { COMPUTE_PROVIDERS, settingLabel } from "@/lib/setting-labels";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

interface Group {
  id: string;
  title: string;
  blurb: string;
  keys: string[];
  match?: (name: string) => boolean;
  // Clusters inside a section: a heading and the keys under it. A cluster
  // with a provider shows only while compute.provider names it (ADR-042 D6).
  clusters?: { title: string; keys: string[]; provider?: string }[];
  // A read-only strip rendered above the rows (the daemon section's "Running").
  strip?: () => React.ReactNode;
}

// Four sections, as the store groups them: the 16 legacy rows every store
// has (Runner caps), then store.ExtraSettingNames() - compute.*, evolve.*
// and daemon.* - which are not written at Create. Unknown keys fall into
// "Other" so a new cap the daemon learns never disappears from the owner's
// view.
const RUNNER_CLUSTERS = [
  { title: "The machine", keys: ["stop", "workers", "parallel_per_project", "jobs_per_day", "minutes_per_job", "max_ai_credits", "runner_cmd", "prune_after_days"] },
  { title: "The worker", keys: ["worker_model", "light_model", "landing", "landing.push", "push.credential"] },
  { title: "The board", keys: ["quorum", "expiry_days", "stale_days", "claim_hours", "lead.ballot_hours"] },
  { title: "The lead", keys: ["lead.idle_hours", "lead.idle_max_jobs", "lead.stuck_thread_jobs", "lead.research_minutes", "lead.research_per_day"] },
  { title: "Amendments", keys: ["amend_quorum", "amend_majority"] },
];
const GROUPS: Group[] = [
  {
    id: "provider",
    title: "Model provider",
    blurb: "Where the native harness sends its model calls when the provider above is openai: OpenAI itself, Azure OpenAI, Ollama, LM Studio or OpenRouter. The API key lives on this machine, never in the store.",
    keys: ["provider.openai.base_url"],
    match: (n) => n.startsWith("provider."),
  },
  {
    id: "owner",
    title: "You",
    blurb: "Whether you are there to answer. On away, each project's lead decides the board's proposals on your behalf with its best judgement and the decision is recorded as the lead's, never as yours; hard stops, money (compute requests), Tier 2 decisions (ADRs), the constitution's protected sections and the kill switch still wait for you. Flip it back to present when you return.",
    keys: ["owner.presence"],
    match: (n) => n.startsWith("owner."),
  },
  {
    id: "runner",
    title: "Runner caps",
    blurb: "How hard the daemon may work, what each job runs with, and the board's thresholds. stop halts everything at once. Two models: worker_model runs the work that changes code (task, improve and experiment jobs); light_model runs the rest - the lead, answers, ballots, reflects, onboarding - and means worker_model while empty, so a cheaper model can take the routine.",
    keys: RUNNER_CLUSTERS.flatMap((c) => c.keys),
    match: (n) => n.startsWith("lead."),
    clusters: RUNNER_CLUSTERS,
  },
  {
    id: "compute",
    title: "Compute",
    blurb: "Caps for machines agents may ask for, the provider that makes them (Azure, AWS, GCP, or Multipass for VMs on the hub machine itself) and the target it makes them in; only the chosen provider's settings matter. Credentials never live here: the hub machine's az login, aws configure, gcloud auth login or running multipassd is the auth. A machine whose Copilot login is refused is the daemon's to mend or retire (ADR-034): it places the executor token you put on file with pulse executor token, else stops the machine PULSE created, retires it and deletes it after the grace hours - a finding each time, never a question.",
    keys: ["compute.provider", "compute.max_vms", "compute.max_hours_per_day", "compute.auto_approve", "compute.budget_credits", "compute.self_heal", "compute.self_retire", "compute.retire_grace_hours", "compute.azure.subscription", "compute.azure.resource_group", "compute.azure.location", "compute.azure.vm_size", "compute.azure.image", "compute.aws.profile", "compute.aws.region", "compute.aws.instance_type", "compute.aws.image", "compute.aws.subnet", "compute.aws.security_group", "compute.gcp.project", "compute.gcp.zone", "compute.gcp.machine_type", "compute.gcp.image", "compute.gcp.network", "compute.multipass.size", "compute.multipass.image"],
    match: (n) => n.startsWith("compute."),
    clusters: [
      { title: "Caps", keys: ["compute.provider", "compute.max_vms", "compute.max_hours_per_day", "compute.auto_approve", "compute.budget_credits"] },
      { title: "Refused logins", keys: ["compute.self_heal", "compute.self_retire", "compute.retire_grace_hours"] },
      { title: "Azure target", provider: "azure", keys: ["compute.azure.subscription", "compute.azure.resource_group", "compute.azure.location", "compute.azure.vm_size", "compute.azure.image"] },
      { title: "AWS target", provider: "aws", keys: ["compute.aws.profile", "compute.aws.region", "compute.aws.instance_type", "compute.aws.image", "compute.aws.subnet", "compute.aws.security_group"] },
      { title: "GCP target", provider: "gcp", keys: ["compute.gcp.project", "compute.gcp.zone", "compute.gcp.machine_type", "compute.gcp.image", "compute.gcp.network"] },
      { title: "Multipass target (local VMs)", provider: "multipass", keys: ["compute.multipass.size", "compute.multipass.image"] },
    ],
  },
  {
    id: "evolve",
    title: "Self-improvement loop",
    blurb: "Bounds on pulse-evolve. The held-out directory is yours alone: agents never see its path. With evolve.heldout on auto the daemon creates the set itself - a private folder under the store and two fixtures written by a sealed model call that sees no repository, board or store - and labels them generated (ADR-033). On owner, you write them from the Held-out fixtures dialog on a project's Self-improvement panel, or with pulse evolve setup DIR; a fixture you write is never overwritten either way.",
    keys: ["evolve.loop", "evolve.limits", "evolve.tolerance", "evolve.heldout", "evolve.heldout_dir", "evolve.draft_hours"],
    match: (n) => n.startsWith("evolve."),
  },
  {
    id: "daemon",
    title: "Running",
    blurb: "Which pulse runs where, and how it keeps itself current. After a landing in the brain the daemon rebuilds pulse from the checkout, smoke-tests the new binary, installs it in place and restarts once its runs finish; the hub follows within a minute; three early exits in a row put the previous build back and turn this off with a finding (ADR-035). The install line is for the first install only.",
    keys: ["daemon.self_update", "daemon.self_update_retry_minutes", "daemon.recover", "daemon.recover_days", "daemon.recover_limit", "daemon.sweep", "daemon.sweep_grace_minutes", "daemon.sweep_branch_days", "daemon.sweep_keep_days", "daemon.min_free_mb"],
    match: (n) => n.startsWith("daemon."),
    clusters: [
      { title: "Self-update", keys: ["daemon.self_update", "daemon.self_update_retry_minutes"] },
      { title: "Failed jobs", keys: ["daemon.recover", "daemon.recover_days", "daemon.recover_limit"] },
      { title: "Housekeeping", keys: ["daemon.sweep", "daemon.sweep_grace_minutes", "daemon.sweep_branch_days", "daemon.sweep_keep_days", "daemon.min_free_mb"] },
    ],
    strip: () => <RunningStrip />,
  },
];

// Mirrors store.ModelSettings, store.ComputeSettings, store.EvolveSettings and store.DaemonSettings in the Go store.
// These rows are not inserted at Create, so when the hub omits one the
// screen still lists it at its effective (default) value and the owner can
// set it from here.
const EXTRA_DEFAULTS: Omit<Setting, "updated_at" | "updated_by">[] = [
  { name: "owner.presence", value: "present", default: "present", kind: "choice", min: null, choices: ["present", "away"], description: "present: proposals, compute requests and owner questions wait for you; away: each project's lead decides the board's proposals on your behalf with its best judgement, recorded as the lead's decision and never as yours - hard stops, money (compute), Tier 2 decisions (ADRs), the constitution's protected sections and the kill switch still wait for you" },
  { name: "light_model", value: "", default: "", kind: "text", min: null, choices: null, description: "the model of the light runs - lead, answer, ballot, reflect, onboard and clean jobs, the sealed held-out call and the login check; empty means worker_model" },
  { name: "compute.provider", value: "azure", default: "azure", kind: "choice", min: null, choices: ["azure", "aws", "gcp", "multipass"], description: "where agents get more machines from: azure (az login), aws (aws configure / aws sso login), gcp (gcloud auth login) or multipass (local VMs on the hub machine); each provider reads its own compute.<provider>.* settings" },
  { name: "compute.max_vms", value: "0", default: "0", kind: "count", min: 0, choices: null, description: "VMs that may exist at once across all requests; 0 means compute is off" },
  { name: "compute.max_hours_per_day", value: "0", default: "0", kind: "count", min: 0, choices: null, description: "VM-hours the runner may approve per UTC day; 0 means none" },
  { name: "compute.auto_approve", value: "0", default: "0", kind: "switch", min: null, choices: null, description: "1 lets within-cap requests start without the owner; 0 holds every request for a decision" },
  { name: "compute.budget_credits", value: "0", default: "0", kind: "count", min: 0, choices: null, description: "credits the compute requests may spend in total; 0 means no budget" },
  { name: "compute.self_heal", value: "1", default: "1", kind: "switch", min: null, choices: null, description: "1: a refused login is replaced from the executor token on file (pulse executor token); 0: never" },
  { name: "compute.self_retire", value: "1", default: "1", kind: "switch", min: null, choices: null, description: "1: a machine PULSE created whose login stays refused is stopped and retired, then deleted after the grace hours; 0: it is left as it is" },
  { name: "compute.retire_grace_hours", value: "24", default: "24", kind: "count", min: 1, choices: null, description: "hours a retired machine stays stopped before it is deleted; starting it again at the provider (the finding names the command) and pulse executor activate within them keep it" },
  { name: "compute.azure.subscription", value: "", default: "", kind: "identifier", min: null, choices: null, description: "Azure subscription id the VMs are created in" },
  { name: "compute.azure.resource_group", value: "", default: "", kind: "identifier", min: null, choices: null, description: "resource group that holds the executor VMs" },
  { name: "compute.azure.location", value: "", default: "", kind: "identifier", min: null, choices: null, description: "Azure region, e.g. westeurope" },
  { name: "compute.azure.vm_size", value: "Standard_B2ms", default: "Standard_B2ms", kind: "identifier", min: null, choices: null, description: "VM size used when a request does not name one" },
  { name: "compute.azure.image", value: "Ubuntu2404", default: "Ubuntu2404", kind: "identifier", min: null, choices: null, description: "image alias the VMs boot from" },
  { name: "compute.aws.profile", value: "", default: "", kind: "identifier", min: null, choices: null, description: "AWS CLI profile the runner provisions with; empty means the CLI's default (auth is aws configure or aws sso login on this machine)" },
  { name: "compute.aws.region", value: "", default: "", kind: "identifier", min: null, choices: null, description: "AWS region the instances are created in, e.g. eu-west-1" },
  { name: "compute.aws.instance_type", value: "t3.medium", default: "t3.medium", kind: "identifier", min: null, choices: null, description: "EC2 instance type for a provisioned hand, e.g. t3.medium" },
  { name: "compute.aws.image", value: "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id", default: "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id", kind: "identifier", min: null, choices: null, description: "AMI id (ami-...) or an SSM public parameter path resolved at launch; the default is Canonical's current Ubuntu 24.04 amd64 image" },
  { name: "compute.aws.subnet", value: "", default: "", kind: "identifier", min: null, choices: null, description: "subnet id the instances join; empty means the default VPC's default subnet in the region" },
  { name: "compute.aws.security_group", value: "", default: "", kind: "identifier", min: null, choices: null, description: "security group id the instances get; empty means a group named pulse-hands that allows ssh in, created when missing" },
  { name: "compute.gcp.project", value: "", default: "", kind: "identifier", min: null, choices: null, description: "Google Cloud project the instances are created in; empty means gcloud's current project (auth is gcloud auth login on this machine)" },
  { name: "compute.gcp.zone", value: "", default: "", kind: "identifier", min: null, choices: null, description: "Compute Engine zone, e.g. europe-west4-a" },
  { name: "compute.gcp.machine_type", value: "e2-medium", default: "e2-medium", kind: "identifier", min: null, choices: null, description: "machine type for a provisioned hand, e.g. e2-medium" },
  { name: "compute.gcp.image", value: "ubuntu-2404-lts-amd64", default: "ubuntu-2404-lts-amd64", kind: "identifier", min: null, choices: null, description: "Ubuntu image family in ubuntu-os-cloud, e.g. ubuntu-2404-lts-amd64, or a full image path projects/P/global/images/I" },
  { name: "compute.gcp.network", value: "", default: "", kind: "identifier", min: null, choices: null, description: "VPC network the instances join; empty means default. A firewall rule pulse-hands-ssh (tcp 22 to the tag pulse-hands) is created on it when missing" },
  { name: "compute.multipass.size", value: "cpus=2,mem=4G,disk=32G", default: "cpus=2,mem=4G,disk=32G", kind: "identifier", min: null, choices: null, description: "size of a local multipass hand as cpus=N,mem=SIZE,disk=SIZE, e.g. cpus=2,mem=4G,disk=32G" },
  { name: "compute.multipass.image", value: "24.04", default: "24.04", kind: "identifier", min: null, choices: null, description: "multipass image a local hand launches from, e.g. 24.04" },
  { name: "evolve.loop", value: "0", default: "0", kind: "switch", min: null, choices: null, description: "1 lets pulse-evolve run cycles back to back; 0 keeps the loop off" },
  { name: "evolve.limits", value: "none", default: "none", kind: "text", min: null, choices: null, description: "caps the constitution writes for the loop, or none" },
  { name: "evolve.tolerance", value: "0.02", default: "0.02", kind: "text", min: null, choices: null, description: "fraction a measured check may regress before a cycle is discarded" },
  { name: "evolve.heldout", value: "auto", default: "auto", kind: "choice", min: null, choices: ["auto", "owner"], description: "auto: the daemon writes the private held-out set itself, sealed and labelled generated; owner: you write it from the console (ADR-033)" },
  // Owner-private: the hub answers "[owner-private]" instead of the real path once set.
  { name: "evolve.heldout_dir", value: "", default: "", kind: "identifier", min: null, choices: null, description: "path to the owner's held-out evaluators; never shown to agents", secret: true },
  { name: "evolve.draft_hours", value: "24", default: "24", kind: "count", min: 0, choices: null, description: "hours between the drafting jobs the planner queues for a project whose constitution or evaluators is still the seeded template text (ADR-046); 0 leaves drafting to the console's Draft it for me button" },
  { name: "daemon.self_update", value: "on", default: "on", kind: "choice", min: null, choices: ["on", "off"], description: "on: after a landing in the brain the daemon rebuilds pulse from the clean checkout, smoke-tests it, installs it and restarts after its runs in flight; off: builds are yours (ADR-035; the launcher turns it off after a crash-loop)" },
  { name: "daemon.self_update_retry_minutes", value: "60", default: "60", kind: "count", min: 1, choices: null, description: "minutes the daemon waits before building again after a failed build or smoke test" },
  { name: "landing.push", value: "off", default: "off", kind: "choice", min: null, choices: ["off", "main"], description: "main: after a landing the daemon pushes the project's default branch to its upstream, fast-forward only, with the git credentials this machine already has - or with push.credential when it is set - and pushes what landed earlier and was not pushed yet; a failed push shows in the landing line of pulse status and the hub. off: you push (ADR-037)" },
  { name: "push.credential", value: "", default: "", kind: "reference", min: null, choices: null, description: "where the daemon's own SSH push key lives, as a reference and never the key itself: op://VAULT/ITEM/FIELD?ssh-format=openssh (1Password, read with a service-account token kept by `pulse cred token`), keychain://SERVICE/ACCOUNT (a macOS Keychain item) or file:///ABSOLUTE/PATH (a 0600 file). The key exists on disk only for the seconds of a push; when it cannot be resolved the landing is held with the reason and the daemon never falls back to your credentials. Empty: push with what git already uses. Set it with `pulse cred set`, which probes the remote first (ADR-041)" },
];

function withExtraDefaults(rows: Setting[]): Setting[] {
  const have = new Set(rows.map((s) => s.name));
  const missing = EXTRA_DEFAULTS.filter((d) => !have.has(d.name)).map((d) => ({ ...d, updated_at: "", updated_by: "default" }));
  return missing.length ? [...rows, ...missing] : rows;
}

const SETTING_LABELS: Record<string, { on?: string; off?: string; choices?: Record<string, string> }> = {
  stop: { on: "STOPPED - nothing runs", off: "running" },
  "owner.presence": { choices: { present: "present — proposals wait for you", away: "away — the leads decide for you" } },
  "compute.auto_approve": { on: "on - within-cap requests start without you", off: "off - every request waits for you" },
  "compute.self_heal": { on: "on - a refused login is replaced from the executor token", off: "off - a refused login stays refused" },
  "compute.self_retire": { on: "on - a refused VM PULSE created is deallocated, retired, then deleted", off: "off - a refused VM is left running" },
  "evolve.loop": { on: "on - cycles run back to back", off: "off - the loop waits" },
};

// Section margins complement the shell's scroll padding so headings clear
// the sticky page title and section pills.
const ANCHOR = "scroll-mt-32";
const PILL = "inline-flex h-7 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full border px-3 text-xs hover:bg-muted";
const SECTION_ICONS: Record<string, React.ComponentType<{ className?: string }>> = {
  stop: PowerIcon,
  provider: PlugIcon,
  owner: UserRoundIcon,
  runner: GaugeIcon,
  compute: CloudIcon,
  evolve: SparklesIcon,
  daemon: ActivityIcon,
  other: SettingsIcon,
};
const SECTION_TONES: Record<string, string> = {
  stop: "bg-bad/10 text-bad",
  provider: "bg-muted text-foreground",
  owner: "bg-info/10 text-info",
  runner: "bg-primary/10 text-primary",
  compute: "bg-accent-2/10 text-accent-2",
  evolve: "bg-ok/10 text-ok",
  daemon: "bg-warn/10 text-warn",
  other: "bg-muted text-muted-foreground",
};

// The rows of one section. The push key has its own row (a picker, not a
// text field). When the section knows compute providers, only the chosen
// provider's target shows - unless the owner is searching, when every match
// counts.
function GroupRows({ g, rows, canEdit, provider, searching }: { g: Group; rows: Setting[]; canEdit: boolean; provider: string; searching: boolean }) {
  const row = (s: Setting) => (s.name === "push.credential" ? <CredentialRow key={s.name} s={s} canEdit={canEdit} /> : <SettingRow key={s.name} s={s} canEdit={canEdit} labels={SETTING_LABELS[s.name]} />);
  const clusters = g.clusters;
  const strip = g.strip ? <div className="border-b">{g.strip()}</div> : null;
  if (!clusters)
    return (
      <>
        {strip}
        <div className="divide-y">{rows.map(row)}</div>
      </>
    );
  const loose = rows.filter((s) => !clusters.some((c) => c.keys.includes(s.name)));
  const others = clusters.filter((c) => c.provider && c.provider !== provider).map((c) => COMPUTE_PROVIDERS[c.provider!] ?? c.provider!);
  return (
    <>
      {strip}
      {clusters.map((c) => {
        if (c.provider && c.provider !== provider && !searching) return null;
        const inCluster = rows.filter((s) => c.keys.includes(s.name));
        if (!inCluster.length) return null;
        const chosen = c.provider && c.provider === provider;
        return (
          <div key={c.title} className="border-t first:border-t-0">
            <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5 px-4 pt-3">
              <h3 className="text-[11px] font-medium uppercase tracking-wider text-muted-foreground">{c.title}</h3>
              {chosen && others.length && !searching ? <span className="text-[11.5px] text-muted-foreground">Only the chosen provider&apos;s target is shown - pick {others.join(", ")} above to set up another.</span> : null}
              {c.provider && !chosen ? <span className="text-[11.5px] text-warn">Not the chosen provider - these rows sleep until Cloud provider says {COMPUTE_PROVIDERS[c.provider] ?? c.provider}.</span> : null}
            </div>
            <div className="divide-y">{inCluster.map(row)}</div>
          </div>
        );
      })}
      {loose.length ? <div className="divide-y border-t first:border-t-0">{loose.map(row)}</div> : null}
    </>
  );
}

function SectionCard({ id, title, blurb, count, children, alert }: { id: string; title: React.ReactNode; blurb: React.ReactNode; count?: React.ReactNode; children: React.ReactNode; alert?: boolean }) {
  const Icon = SECTION_ICONS[id] ?? RefreshCwIcon;
  return (
    <div className={cn("overflow-hidden rounded-xl border bg-card shadow-card", alert && "bg-bad/5 ring-1 ring-bad/40")}>
      <header className="flex items-start gap-3 border-b px-4 py-3">
        <span className={cn("flex size-8 shrink-0 items-center justify-center rounded-md", SECTION_TONES[id] ?? SECTION_TONES.other)} aria-hidden>
          <Icon className="size-4" />
        </span>
        <div className="min-w-0">
          <h2 className="text-sm font-semibold tracking-tight">
            {title}
            {count}
          </h2>
          <Clamp className="mt-0.5">{blurb}</Clamp>
        </div>
      </header>
      {children}
    </div>
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
    const used = new Set<string>(all.filter((s) => isHarnessSetting(s.name)).map((s) => s.name));
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
        <PageTitle icon={SettingsIcon}>Settings</PageTitle>
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      </>
    );
  if (!q.data) return <SkeletonPage rows={4} />;
  const d = q.data;
  const stop = d.settings.find((s) => s.name === "stop");
  const stopped = stop?.value === "1";
  const needle = find.trim().toLowerCase();
  const match = (s: Setting) => !needle || s.name.toLowerCase().includes(needle) || settingLabel(s.name).toLowerCase().includes(needle) || s.description.toLowerCase().includes(needle);
  const provider = d.settings.find((s) => s.name === "compute.provider")?.value || "azure";
  // Sleeping provider rows leave the count too, unless a search wakes them.
  const awake = (s: Setting) => !!needle || !GROUPS.some((g) => g.clusters?.some((c) => c.provider && c.provider !== provider && c.keys.includes(s.name)));
  const harnessShown = !needle || "harness built-in native copilot github model provider login max steps timeout output limit".includes(needle);
  // The kill switch has its own card at the top, so it leaves the runner cluster.
  const killShown = !!stop && match(stop);
  const base = groups.map(({ g, rows }) => ({ g, rows: rows.filter((s) => (!stop || s.name !== "stop") && awake(s)) }));
  const total = base.reduce((n, x) => n + x.rows.length, 0) + (stop ? 1 : 0);
  const shown = base.map(({ g, rows }) => ({ g, all: rows.length, rows: rows.filter(match) })).filter((x) => x.rows.length);
  const hit = new Set(shown.map((x) => x.g.id));
  const matched = shown.reduce((n, x) => n + x.rows.length, 0) + (killShown ? 1 : 0);
  return (
    <>
      <PageTitle
        icon={SettingsIcon}
        sub={<>What the daemon may do, in your words. Every change is recorded with who made it and read on the daemon&apos;s next loop; the small key under each name is what <Mono>pulse setting set</Mono> takes.</>}
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
            <a href="#harness" className={cn(PILL, !harnessShown && "pointer-events-none opacity-40")} aria-disabled={!harnessShown || undefined}>
              Harness
            </a>
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
            <SectionCard id="stop" title="Kill switch" blurb="Stop or resume the daemon immediately. This row stays separate because it is the owner's emergency brake." alert={stopped}>
              <SettingRow s={stop} canEdit={d.can_edit} labels={SETTING_LABELS.stop} title="Kill switch" />
            </SectionCard>
          </section>
        ) : null}
        {harnessShown ? <HarnessSettingsCard /> : null}
        {shown.map(({ g, rows, all }) => (
          <section key={g.id} id={g.id} className={ANCHOR}>
            <SectionCard
              id={g.id}
              title={g.title}
              blurb={g.blurb}
              count={needle ? <span className="tnum ml-2 font-normal text-muted-foreground">{rows.length} of {all}</span> : null}
            >
              <GroupRows g={g} rows={rows} canEdit={d.can_edit} provider={provider} searching={!!needle} />
            </SectionCard>
          </section>
        ))}
        {needle && !matched ? <EmptyState>No setting matches &ldquo;{find.trim()}&rdquo; - try a word from its name such as push, model or region.</EmptyState> : null}
      </div>
    </>
  );
}
