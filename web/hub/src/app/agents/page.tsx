"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { BotIcon, HardDriveIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ExecutorRow, MachinePanel } from "@/components/compute/machine";
import { DataTable, type Column } from "@/components/pulse/data-table";
import { CardGrid, ItemCard, ItemCardBody, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { EmptyState, ErrorNotice, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { useViewMode, ViewToggle } from "@/components/pulse/view-toggle";
import { Avatar, ProjectLink, ThreadLink, ToneBadge, When, Who } from "@/components/pulse/who";
import type { AgentPresence, RosterRow, Runner } from "@/lib/api";
import { num, plural, rel, truncate } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useApi, useNow } from "@/lib/use-api";
import { cn } from "@/lib/utils";

interface AgentGroup {
  agent: string;
  primary: RosterRow;
  sessions: RosterRow[];
  posts: number;
  votes: number;
  claims: number;
  jobs_done: number;
  jobs_failed: number;
  projects: string[];
  last_seen: string;
  active: number;
  presence: AgentPresence;
  presence_reason: string;
  last_heartbeat_at: string;
  heartbeat_source: string;
  last_useful_at: string;
}

const identityStatus = (r: RosterRow) => r.agent_status || r.status;
const timestamp = (s?: string) => (s ? Date.parse(s) || 0 : 0);
const currentSession = (r: RosterRow) => !!r.presence_session && (r.session === r.presence_session || (!r.session && r.sessions?.includes(r.presence_session)));

// The server owns freshness and supplies the deadline. Expiring a cached
// online badge also works when a refresh hangs or the tab was asleep. Any
// heartbeat keeps an agent alive (a command, a hook, a lease, the runner);
// only a supervised execution beat has to belong to the current session.
function presence(r: RosterRow, now: number): AgentPresence {
  if (identityStatus(r) === "pruned") return "pruned";
  if (r.kind === "legacy") return "legacy";
  if (r.kind === "human") return "unknown";
  if (r.presence === "online") {
    const beat = timestamp(r.last_heartbeat_at);
    const checked = timestamp(r.presence_checked_at);
    const expires = timestamp(r.presence_expires_at);
    if (!beat || checked < beat || expires <= checked) return "unknown";
    if (r.heartbeat_source === "execution" && !currentSession(r)) return "unknown";
    return now < expires ? "online" : "dead";
  }
  if (r.presence === "idle")
    return (r.kind === "worker" || r.kind === "lead") && ["active", "idle"].includes(identityStatus(r)) ? "idle" : "unknown";
  return r.presence || "unknown";
}

const live = (g: AgentGroup) => g.presence === "online" || g.presence === "idle";

// Current APIs aggregate identities; older fixtures may be session-shaped.
// Prefer the latest evaluated current session, never its historical post count.
function group(rows: RosterRow[], now: number): AgentGroup[] {
  const by = new Map<string, RosterRow[]>();
  for (const r of rows) {
    const k = r.agent || r.name || "?";
    by.set(k, [...(by.get(k) || []), r]);
  }
  return Array.from(by.entries())
    .map(([agent, rs]) => {
      const sorted = [...rs].sort((a, b) =>
        Number(identityStatus(b) === "pruned") - Number(identityStatus(a) === "pruned") ||
        timestamp(b.presence_checked_at) - timestamp(a.presence_checked_at) ||
        Number(currentSession(b)) - Number(currentSession(a)) ||
        timestamp(b.last_seen) - timestamp(a.last_seen));
      const primary = sorted[0]!;
      const state = presence(primary, now);
      const sum = (f: (r: RosterRow) => number) => rs.reduce((a, r) => a + (f(r) || 0), 0);
      return {
        agent,
        primary,
        sessions: rs,
        posts: sum((r) => r.posts),
        votes: sum((r) => r.votes),
        claims: sum((r) => r.claims),
        jobs_done: sum((r) => r.jobs_done),
        jobs_failed: sum((r) => r.jobs_failed),
        projects: Array.from(new Set(rs.flatMap((r) => r.projects || []))).sort(),
        last_seen: rs.map((r) => r.last_seen).sort().reverse()[0] || "",
        active: state === "online" ? rs.reduce((a, r) => a + (r.active_claims?.length || 0), 0) : 0,
        presence: state,
        presence_reason: state === "dead" && primary.presence === "online" ? "No check-in before the server's deadline." : primary.presence_reason || "No check-in on record.",
        last_heartbeat_at: primary.last_heartbeat_at || "",
        heartbeat_source: primary.heartbeat_source || "",
        last_useful_at: rs.map((r) => r.last_useful_at || "").sort().reverse()[0] || "",
      };
    })
    .sort((a, b) => (a.last_seen < b.last_seen ? 1 : -1));
}

// ----- words ----------------------------------------------------------------

const ago = (d: string, now: number) => (timestamp(d) ? rel(d, now) : "");

const PRESENCE: Record<AgentPresence, { word: string; tone: "ok" | "info" | "bad" | "neutral"; dot: string }> = {
  online: { word: "Online", tone: "ok", dot: "bg-ok" },
  idle: { word: "Idle", tone: "info", dot: "bg-info" },
  dead: { word: "Offline", tone: "bad", dot: "bg-bad" },
  unknown: { word: "Not checked in", tone: "neutral", dot: "bg-muted-foreground/50" },
  legacy: { word: "Older identity", tone: "neutral", dot: "bg-muted-foreground/50" },
  pruned: { word: "Removed", tone: "neutral", dot: "bg-muted-foreground/50" },
};

// The order the status filter lists them in.
const PRESENCE_ORDER: AgentPresence[] = ["online", "idle", "dead", "unknown", "legacy", "pruned"];

// What the agent runs on, in words; the raw name stays in the tooltip.
const RUNNER_WORD: Record<string, string> = {
  "claude-code": "Claude Code",
  "copilot-cli": "Copilot CLI",
  codex: "Codex",
  "pulse-daemon": "PULSE runner",
};
const runnerWord = (r?: string | null) => (r ? RUNNER_WORD[r] ?? r : "");

const sessionCount = (g: AgentGroup) => g.sessions.reduce((a, r) => a + (r.session_count ?? r.sessions?.length ?? Number(!!r.session)), 0);

const isLead = (g: AgentGroup) => g.primary.kind === "lead" || /lead$/.test(g.agent);
const isHuman = (g: AgentGroup) => g.primary.kind === "human" || g.agent === "human" || g.agent === "owner";
function roleWord(g: AgentGroup): string {
  if (isHuman(g)) return "Owner";
  if (isLead(g)) return "Lead agent";
  if (g.primary.kind === "legacy") return "Agent";
  return "Worker";
}
// "Worker, Copilot CLI": the role and what it runs on.
function roleLine(g: AgentGroup): string {
  const on = isHuman(g) ? "" : runnerWord(g.primary.runner);
  return roleWord(g) + (on ? ", " + on : "");
}
const runnerTip = (g: AgentGroup, name: string) => {
  const meta = [g.primary.runner, g.primary.model].filter(Boolean).join(", ");
  const n = sessionCount(g);
  return (meta ? name + " (" + meta + ")" : name) + (n ? ", " + plural(n, "session") : "");
};

// "Checked in just now", "Last check-in 6 minutes ago", "Never checked in".
function checkInText(g: AgentGroup, now: number): string {
  if (!g.last_heartbeat_at) return g.presence === "idle" ? "Ready for the next job" : "Never checked in";
  const when = ago(g.last_heartbeat_at, now);
  return g.presence === "online" ? "Checked in " + when : "Last check-in " + when;
}

// "no useful output since … (limit 14 days)" reads "Removed after 14 days without useful output".
function prunedText(reason: string): string {
  const days = /limit\s+(\d+)\s+days?/i.exec(reason);
  if (days && /useful/i.test(reason)) return "Removed after " + days[1] + " days without useful output";
  return "Removed: " + reason;
}

function PresenceBadge({ g }: { g: AgentGroup }) {
  const p = PRESENCE[g.presence] ?? PRESENCE.unknown;
  return (
    <ToneBadge dot tone={p.tone} title={g.presence_reason}>
      {p.word}
    </ToneBadge>
  );
}

const currentFocus = (g: AgentGroup) =>
  [...g.sessions].sort((a, b) => timestamp(b.focus_since) - timestamp(a.focus_since)).find((r) => r.current_task || r.current_job || r.current_thread) ?? null;

// The facts as a sentence: "7 posts, 3 votes, 1 thing picked up".
function factsText(g: AgentGroup): string {
  const parts = [
    g.jobs_done ? plural(g.jobs_done, "job") + " done" : "",
    g.jobs_failed ? num(g.jobs_failed) + " failed" : "",
    g.posts ? plural(g.posts, "post") : "",
    g.votes ? plural(g.votes, "vote") : "",
    g.claims ? (g.claims === 1 ? "1 thing" : num(g.claims) + " things") + " picked up" : "",
    sessionCount(g) > 1 ? plural(sessionCount(g), "session") : "",
  ].filter(Boolean);
  return parts.length ? parts.join(", ") : "Nothing on record yet";
}

// What the agent is doing right now, as one sentence with the thing linked.
function AgentNow({ g, compact }: { g: AgentGroup; compact?: boolean }) {
  const r = currentFocus(g);
  const claims = g.presence === "online" ? g.sessions.flatMap((s) => s.active_claims || []) : [];
  const cls = cn("min-w-0 leading-relaxed anywhere", compact ? "text-[13px]" : "text-sm");
  if (r?.current_task) {
    const project = r.current_project || g.projects[0] || "";
    const title = r.current_task_title || r.current_task;
    const tip = [r.current_task, r.current_job ? "job " + r.current_job + (r.current_job_kind ? " (" + r.current_job_kind + ")" : "") : "", r.executor ? "on " + r.executor : ""].filter(Boolean).join(", ");
    return (
      <p className={cls} title={tip}>
        <span className="text-muted-foreground">Working on </span>
        {project ? (
          <Link href={hrefs.task(project, r.current_task)} className="font-medium text-foreground underline-offset-2 hover:underline">
            {truncate(title, compact ? 56 : 90)}
          </Link>
        ) : (
          <span className="font-medium text-foreground">{truncate(title, compact ? 56 : 90)}</span>
        )}
        {project && !compact ? <span className="text-muted-foreground">{" in " + project}</span> : null}
        {r.current_job ? <span className="block text-[13px] text-muted-foreground">{"Running: " + truncate(r.current_job_title || "a job", compact ? 56 : 90)}</span> : null}
      </p>
    );
  }
  if (r?.current_job) {
    const tip = "Job " + r.current_job + (r.current_job_kind ? " (" + r.current_job_kind + ")" : "") + (r.executor ? " on " + r.executor : "") + (r.current_thread ? ", thread " + r.current_thread : "");
    return (
      <p className={cls} title={tip}>
        <span className="text-muted-foreground">Running a job: </span>
        <span className="font-medium text-foreground">{truncate(r.current_job_title || "untitled job", compact ? 56 : 90)}</span>
      </p>
    );
  }
  const claim = claims[0];
  if (claim || r?.current_thread) {
    const slug = claim?.slug || r?.current_project || g.projects[0] || "";
    const key = claim?.thread_key || r?.current_thread || "";
    return (
      <p className={cls}>
        <span className="text-muted-foreground">Picked up </span>
        {claim?.summary ? <span className="font-medium text-foreground">{truncate(claim.summary, compact ? 56 : 90)}</span> : null}
        {claim?.summary ? <span className="text-muted-foreground"> in </span> : null}
        {slug ? <ThreadLink slug={slug} k={key} className={cn(compact ? "text-[13px]" : "text-sm", !claim?.summary && "font-medium text-foreground")} /> : <span>{key}</span>}
        {claims.length > 1 ? <span className="text-muted-foreground">{" and " + plural(claims.length - 1, "more thing")}</span> : null}
      </p>
    );
  }
  if (compact) return <span className="text-[13px] text-muted-foreground">{g.presence === "online" || g.presence === "idle" ? "Nothing right now" : "—"}</span>;
  return <p className={cn(cls, "text-muted-foreground")}>{g.presence === "online" || g.presence === "idle" ? "Not working on anything right now." : "Not working right now."}</p>;
}

function Projects({ list, className }: { list: string[]; className?: string }) {
  if (!list.length) return null;
  return (
    <span className={cn("min-w-0", className)}>
      {list.map((p, i) => (
        <span key={p}>
          <span className="whitespace-nowrap">
            {p.startsWith("commons") ? <span title={p}>shared board</span> : <ProjectLink slug={p} className="font-normal text-foreground" />}
            {i < list.length - 1 ? "," : ""}
          </span>
          {i < list.length - 1 ? " " : ""}
        </span>
      ))}
    </span>
  );
}

// The table's first cell: avatar, name and role; runner and model in the tooltip.
function AgentName({ g }: { g: AgentGroup }) {
  const p = g.primary;
  const name = p.agent || p.name || g.agent;
  return (
    <span className="inline-flex min-w-0 max-w-44 items-center gap-2" title={runnerTip(g, name)}>
      <Avatar seed={name} size={24} human={isHuman(g)} />
      <span className="min-w-0">
        <span className="block truncate text-sm font-medium text-foreground">{isHuman(g) ? "You" : name}</span>
        <span className="block truncate text-xs text-muted-foreground">{roleLine(g)}</span>
      </span>
    </span>
  );
}

// Second facts sit on a second line so the table fits the page width
// without scrolling: the check-in under the status, failures under the jobs,
// the last useful output under the last activity.
const COLUMNS: Column<AgentGroup>[] = [
  { key: "agent", label: "Agent", sort: (g) => g.agent, render: (g) => <AgentName g={g} /> },
  {
    key: "presence",
    label: "Status",
    nowrap: true,
    sort: (g) => g.presence,
    render: (g) => (
      <span className="grid justify-items-start gap-1">
        <PresenceBadge g={g} />
        {g.last_heartbeat_at ? (
          <span className="text-xs text-muted-foreground" title={g.heartbeat_source ? "Last check-in, from " + g.heartbeat_source : "Last check-in"}>
            Checked in <When d={g.last_heartbeat_at} />
          </span>
        ) : null}
      </span>
    ),
  },
  { key: "now", label: "Doing now", className: "min-w-44", sort: (g) => currentFocus(g)?.focus_since || currentFocus(g)?.current_task || "", render: (g) => <AgentNow g={g} compact /> },
  { key: "projects", label: "Projects", className: "min-w-28", render: (g) => <Projects list={g.projects} className="text-[13px]" /> },
  { key: "jobs", label: "Jobs", align: "right", nowrap: true, sort: (g) => g.jobs_done, render: (g) => <span className="tnum">{num(g.jobs_done)}{g.jobs_failed ? <span className="block text-xs text-bad">{num(g.jobs_failed) + " failed"}</span> : null}</span> },
  { key: "posts", label: "Posts", align: "right", sort: (g) => g.posts, render: (g) => <span className="tnum">{num(g.posts)}</span> },
  { key: "votes", label: "Votes", align: "right", hide: "sm", sort: (g) => g.votes, render: (g) => <span className="tnum">{num(g.votes)}</span> },
  { key: "claims", label: "Picked up", align: "right", nowrap: true, hide: "sm", sort: (g) => g.claims, render: (g) => <span className="tnum">{num(g.claims)}{g.active ? <span className="block text-xs text-ok">{g.active + " now"}</span> : null}</span> },
  {
    key: "last",
    label: "Last active",
    nowrap: true,
    sort: (g) => g.last_seen,
    render: (g) => (
      <span className="grid gap-1">
        <When d={g.last_seen} />
        <span className="text-xs text-muted-foreground" title="Last useful output">
          {g.last_useful_at ? <>Output <When d={g.last_useful_at} /></> : "No output yet"}
        </span>
      </span>
    ),
  },
];

// One agent: who it is, whether it is around, what it is doing, what it did.
function AgentCard({ g, now }: { g: AgentGroup; now: number }) {
  const p = g.primary;
  const name = p.agent || p.name || g.agent;
  const human = isHuman(g);
  const presence = PRESENCE[g.presence] ?? PRESENCE.unknown;
  const useful = g.last_useful_at ? ago(g.last_useful_at, now) : "";
  return (
    <ItemCard ariaLabel={(human ? "You" : name) + ", " + presence.word.toLowerCase()} className="min-w-0">
      <div className="flex min-w-0 items-start gap-3">
        <span className="relative shrink-0">
          <Avatar seed={name} size={40} human={human} />
          <span className={cn("absolute -right-0.5 -bottom-0.5 size-3 rounded-full ring-2 ring-card", presence.dot)} aria-hidden />
        </span>
        <div className="min-w-0 flex-1">
          <ItemCardTitle>
            <span title={runnerTip(g, name)}>{human ? "You" : name}</span>
          </ItemCardTitle>
          <p className="flex min-w-0 flex-wrap gap-x-3 text-[13px] leading-relaxed text-muted-foreground anywhere">
            <span>{roleLine(g)}</span>
            {p.pruned_reason ? (
              <span title={p.pruned_reason}>{prunedText(p.pruned_reason)}</span>
            ) : (
              <span title={g.presence_reason + (g.heartbeat_source ? " (from " + g.heartbeat_source + ")" : "")}>{checkInText(g, now)}</span>
            )}
          </p>
        </div>
        <PresenceBadge g={g} />
      </div>
      <AgentNow g={g} />
      <ItemCardBody clamp={0} className="text-foreground">
        {factsText(g)}
      </ItemCardBody>
      <ItemCardMeta className="mt-auto border-t border-border/70 pt-3">
        {g.projects.length ? (
          <span className="min-w-0">
            <span>
              Works on <Projects list={g.projects} />
            </span>
          </span>
        ) : null}
        <span>{useful ? "Last useful output " + useful : "No useful output yet"}</span>
      </ItemCardMeta>
    </ItemCard>
  );
}

const SCOPES = [
  { value: "live", label: "Live" },
  { value: "history", label: "History" },
  { value: "all", label: "All" },
] as const;

const SUB = "Who is working, who is idle, and what each one did.";

export default function AgentsPage() {
  const q = useApi<RosterRow[]>("roster", { refresh: 30000 });
  const now = useNow(1000);
  const runner = useApi<Runner>("runner");
  const [mode, setMode] = useViewMode("agents");
  const [status, setStatus] = useState("all");
  const [scope, setScope] = useState<"live" | "history" | "all">("live");
  const [text, setText] = useState("");
  const groups = useMemo(() => group(q.data ?? [], now), [q.data, now]);
  // The filter uses the same words as the status badge.
  const statuses = useMemo(() => PRESENCE_ORDER.filter((p) => groups.some((g) => g.presence === p)), [groups]);
  const matching = useMemo(() => {
    const t = text.trim().toLowerCase();
    return groups.filter((g) => (status === "all" || g.presence === status) && (!t || g.agent.toLowerCase().includes(t) || g.projects.some((p) => p.toLowerCase().includes(t))));
  }, [groups, status, text]);
  const rows = matching.filter((g) => scope === "all" || (scope === "live" ? live(g) : !live(g)));
  const liveCount = matching.filter(live).length;
  if (q.error)
    return (
      <>
        <PageTitle icon={BotIcon} sub={SUB}>Agents</PageTitle>
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      </>
    );
  if (!q.data) return <SkeletonPage rows={3} />;
  const active = rows.filter((g) => g.active > 0);
  const r = runner.data;
  // Cards put whoever is working now first; the table keeps its own sort.
  const working = (g: AgentGroup) => Number(g.active > 0 || !!currentFocus(g)) * 2 + Number(g.presence === "online");
  const cards = [...rows].sort((a, b) => working(b) - working(a));
  const online = rows.filter((g) => g.presence === "online").length;
  const leads = rows.filter(isLead).length;
  const runs = r?.available ? (r.jobs.cap ? num(r.jobs.runs_today) + " of " + plural(r.jobs.cap, "run") + " used today." : plural(r.jobs.runs_today, "run") + " today, no daily limit.") : "";
  const offline = rows.filter((g) => g.presence === "dead").length;
  const failed = rows.reduce((a, g) => a + g.jobs_failed, 0);
  const pool = r?.available && r.workers?.pool ? plural(r.workers.pool, "worker") + " in the pool, " + num(r.workers.idle) + " idle." : "";
  const summary = rows.length
    ? [plural(rows.length, "agent") + ": " + num(online) + " online" + (offline ? ", " + num(offline) + " offline" : "") + (leads ? ", " + plural(leads, "project lead") : "") + ".", failed ? plural(failed, "job") + " failed." : "", pool, runs].filter(Boolean).join(" ")
    : "";
  const scopeCount = (s: (typeof SCOPES)[number]["value"]) => (s === "live" ? liveCount : s === "history" ? matching.length - liveCount : matching.length);
  // Phones: the tabs and the view toggle share the first row, the
  // status and search the second. Wider screens: one row.
  const filters = (
    <div className="flex min-w-0 flex-wrap items-center gap-2 py-0.5">
      <Tabs value={scope} onValueChange={(v) => setScope(v as typeof scope)} className="scroll-strip min-w-0 max-w-full gap-0">
        <TabsList aria-label="Which agents">
          {SCOPES.map((s) => (
            <TabsTrigger key={s.value} value={s.value} className="flex-none gap-1.5 px-2.5">
              {s.label}
              <span className={cn("tnum rounded-full bg-muted px-1.5 text-xs leading-5 text-muted-foreground", scope === s.value && "bg-primary/10 text-[var(--primary-strong)]")}>{num(scopeCount(s.value))}</span>
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>
      <div className="order-last flex w-full min-w-0 gap-2 sm:order-none sm:w-auto">
        <Select value={status} onValueChange={setStatus}>
          <SelectTrigger aria-label="Status" className="h-9 w-36 shrink-0 sm:w-40">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All statuses</SelectItem>
            {statuses.map((s) => (
              <SelectItem key={s} value={s}>
                {PRESENCE[s].word}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Input type="search" aria-label="Search agents or projects" placeholder="Search agents" value={text} onChange={(e) => setText(e.target.value)} className="h-9 min-w-0 flex-1 sm:w-60 sm:flex-none" />
      </div>
      <ViewToggle value={mode} onChange={setMode} className="ml-auto shrink-0" />
    </div>
  );
  return (
    <>
      <PageTitle icon={BotIcon} sub={SUB} bar={filters}>
        Agents
      </PageTitle>
      {/* The table needs the full width; the machine context moves under it. */}
      <div className={cn("grid gap-6", mode === "cards" && "xl:grid-cols-[minmax(0,1fr)_360px]")}>
        <div className="grid min-w-0 content-start gap-4">
          {summary ? <p className="text-[15px] leading-relaxed text-muted-foreground anywhere" aria-live="polite">{summary}</p> : null}
          {!rows.length ? (
            <EmptyState
              title="No agents in this view"
              icon={<BotIcon className="size-[18px]" />}
              action={groups.length ? <Button size="sm" variant="outline" onClick={() => { setScope("all"); setStatus("all"); setText(""); }}>Show all agents</Button> : null}
            >
              {groups.length ? (scope === "live" ? "No agent is online or idle right now. History shows the ones that worked before." : "Nothing matches this view. Try All, or clear the search.") : "No agent has started yet. Agents show up here as soon as the runner gives one a job, or one posts on a board."}
            </EmptyState>
          ) : mode === "cards" ? (
            <CardGrid cols={2}>
              {cards.map((g) => (
                <AgentCard key={g.agent} g={g} now={now} />
              ))}
            </CardGrid>
          ) : (
            <DataTable columns={COLUMNS} rows={rows} rowKey={(g) => g.agent} initialSort="last" caption="Agents and what they did" className="min-w-0" />
          )}
          {/* The cards say what each agent picked up; the table only counts it. */}
          {mode === "table" && active.length ? (
            <Panel title="Picked up right now">
              <ul className="grid gap-2 text-sm">
                {active.flatMap((g) => g.sessions.flatMap((s) => (s.active_claims || []).map((c) => (
                  <li key={g.agent + c.id} className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 anywhere">
                    <Who row={s} compact />
                    <span className="text-foreground">{c.summary}</span>
                    {c.slug ? <span className="text-[13px] text-muted-foreground">in <ThreadLink slug={c.slug} k={c.thread_key} /></span> : <span className="text-[13px] text-muted-foreground">{c.thread_key}</span>}
                  </li>
                ))))}
              </ul>
            </Panel>
          ) : null}
        </div>
        <aside className={cn("grid min-w-0 content-start gap-4", mode === "table" && "lg:grid-cols-2")} aria-label="Where the agents run">
          <MachinePanel r={r} machines={false} />
          <Panel
            title="Machines"
            icon={HardDriveIcon}
            actions={
              <Link href={hrefs.compute()} className="text-[13px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline">
                Manage machines
              </Link>
            }
          >
            {r?.executors?.length ? (
              <ul className="grid gap-1.5">
                {r.executors.map((e) => (
                  <ExecutorRow key={e.name} e={e} />
                ))}
              </ul>
            ) : (
              <p className="text-sm leading-relaxed text-muted-foreground">Agents run on this machine only. Add another machine on the Machines page when you need more room.</p>
            )}
          </Panel>
        </aside>
      </div>
    </>
  );
}
