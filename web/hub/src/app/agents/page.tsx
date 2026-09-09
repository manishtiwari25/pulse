"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { BotIcon, CircleDotIcon, CrownIcon, GaugeIcon, HardDriveIcon, LayersIcon, LoaderIcon, ServerIcon, TriangleAlertIcon, WifiIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ExecutorRow, LandingHolds } from "@/components/pulse/cards";
import { DataTable, type Column } from "@/components/pulse/data-table";
import { CardGrid, Fact, ItemCard, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Count, EmptyState, ErrorNotice, Kpi, KpiGrid, Kv, Lead, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { useViewMode, ViewToggle } from "@/components/pulse/view-toggle";
import { Avatar, Mono, ProjectLink, StatusBadge, ThreadLink, ToneBadge, When, Who } from "@/components/pulse/who";
import type { AgentPresence, RosterRow, Runner } from "@/lib/api";
import { num, plural, statusTone, truncate } from "@/lib/format";
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
  last_useful_at: string;
}

const identityStatus = (r: RosterRow) => r.agent_status || r.status;
const timestamp = (s?: string) => (s ? Date.parse(s) || 0 : 0);
const currentSession = (r: RosterRow) => !!r.presence_session && (r.session === r.presence_session || (!r.session && r.sessions?.includes(r.presence_session)));

// The server owns freshness and supplies the deadline. Expiring a cached
// online badge also works when a refresh hangs or the tab was asleep.
function presence(r: RosterRow, now: number): AgentPresence {
  if (identityStatus(r) === "pruned") return "pruned";
  if (r.kind === "legacy") return "legacy";
  if (r.kind === "human") return "unknown";
  if (r.presence === "online") {
    const beat = timestamp(r.last_heartbeat_at);
    const checked = timestamp(r.presence_checked_at);
    const expires = timestamp(r.presence_expires_at);
    if (!currentSession(r) || !beat || checked < beat || expires <= checked) return "unknown";
    return now < expires ? "online" : "offline";
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
        presence_reason: state === "offline" && primary.presence === "online" ? "No fresh heartbeat before the server's deadline." : primary.presence_reason || "No supervised heartbeat evidence.",
        last_heartbeat_at: primary.last_heartbeat_at || "",
        last_useful_at: rs.map((r) => r.last_useful_at || "").sort().reverse()[0] || "",
      };
    })
    .sort((a, b) => (a.last_seen < b.last_seen ? 1 : -1));
}

function PresenceBadge({ g }: { g: AgentGroup }) {
  return <ToneBadge dot tone={g.presence === "online" ? "ok" : g.presence === "idle" ? "info" : g.presence === "offline" ? "warn" : g.presence === "pruned" ? "bad" : "neutral"} title={g.presence_reason}>{g.presence}</ToneBadge>;
}

const sessionCount = (g: AgentGroup) => g.sessions.reduce((a, r) => a + (r.session_count ?? r.sessions?.length ?? Number(!!r.session)), 0);
const currentFocus = (g: AgentGroup) =>
  [...g.sessions].sort((a, b) => timestamp(b.focus_since) - timestamp(a.focus_since)).find((r) => r.current_task || r.current_job || r.current_thread) ?? null;

function AgentNow({ g, compact }: { g: AgentGroup; compact?: boolean }) {
  const r = currentFocus(g);
  if (!r) return compact ? <span className="text-muted-foreground">idle</span> : null;
  const taskId = r.current_task || "";
  const taskProject = r.current_project || g.projects[0] || "";
  const taskTitle = r.current_task_title || "";
  const taskText = (
    <>
      <Mono>{taskId}</Mono>
      {taskTitle ? <span className="min-w-0 truncate text-foreground">{truncate(taskTitle, compact ? 48 : 72)}</span> : null}
    </>
  );
  if (compact && taskId) {
    return (
      <span className="inline-flex min-w-0 flex-wrap items-center gap-1.5 text-xs">
        {taskProject ? (
          <Link href={hrefs.task(taskProject, taskId)} className="inline-flex min-w-0 items-center gap-1.5 hover:underline">
            {taskText}
          </Link>
        ) : (
          <span className="inline-flex min-w-0 items-center gap-1.5">{taskText}</span>
        )}
        {r.current_job ? <Mono className="text-muted-foreground">{r.current_job}</Mono> : null}
      </span>
    );
  }
  const taskLine = taskId ? (
    <span className={cn("inline-flex min-w-0 flex-wrap items-center gap-1.5", compact ? "text-xs" : "text-[13px]")}>
      <span className="text-muted-foreground">on task</span>
      {taskProject ? (
        <Link href={hrefs.task(taskProject, taskId)} className="inline-flex min-w-0 items-center gap-1.5 hover:underline">
          {taskText}
        </Link>
      ) : (
        <span className="inline-flex min-w-0 items-center gap-1.5">{taskText}</span>
      )}
    </span>
  ) : null;
  if (r.current_job) {
    return (
      <span className="grid min-w-0 gap-0.5">
        {taskLine}
        <span className={cn("inline-flex min-w-0 flex-wrap items-center gap-1.5", compact ? "text-xs" : "text-[13px]")}>
          <span className="font-medium text-foreground">Now</span>
          {r.current_job_kind ? <ToneBadge tone="info">{r.current_job_kind}</ToneBadge> : null}
          <span className="min-w-0 truncate text-foreground">{truncate(r.current_job_title || r.current_job, 80)}</span>
          <Mono>{r.current_job}</Mono>
          {r.executor ? <span className="text-muted-foreground">{"on " + r.executor}</span> : null}
          {r.current_thread ? <Mono className="text-muted-foreground">{r.current_thread}</Mono> : null}
        </span>
      </span>
    );
  }
  if (taskLine) return taskLine;
  return (
    <span className={cn("inline-flex min-w-0 flex-wrap items-center gap-1.5", compact ? "text-xs" : "text-[13px]")}>
      <span className="text-muted-foreground">claims</span>
      <Mono>{r.current_thread}</Mono>
    </span>
  );
}

const COLUMNS: Column<AgentGroup>[] = [
  { key: "agent", label: "agent", sort: (g) => g.agent, render: (g) => <Who row={g.primary} /> },
  { key: "presence", label: "presence", sort: (g) => g.presence, render: (g) => <PresenceBadge g={g} /> },
  { key: "now", label: "now", sort: (g) => currentFocus(g)?.focus_since || currentFocus(g)?.current_task || "", render: (g) => <AgentNow g={g} compact /> },
  { key: "status", label: "identity", sort: (g) => identityStatus(g.primary), render: (g) => <span className="inline-flex flex-wrap gap-1"><StatusBadge status={identityStatus(g.primary)} />{g.primary.pruned_reason ? <ToneBadge tone="warn" className="h-auto max-w-full whitespace-normal break-words text-left">{"pruned: " + g.primary.pruned_reason}</ToneBadge> : null}</span> },
  { key: "kind", label: "kind", hide: "md", render: (g) => <span className="text-xs text-muted-foreground">{g.primary.kind}</span> },
  { key: "runner", label: "runner / model", hide: "lg", render: (g) => <span className="text-xs text-muted-foreground anywhere">{[g.primary.runner, g.primary.model].filter(Boolean).join(" · ")}</span> },
  { key: "projects", label: "projects", render: (g) => <span className="inline-flex flex-wrap gap-1">{g.projects.map((p) => <ProjectLink key={p} slug={p} className="text-xs" />)}</span> },
  { key: "jobs", label: "jobs", align: "right", sort: (g) => g.jobs_done, render: (g) => <span className="tnum">{num(g.jobs_done)}{g.jobs_failed ? <span className="text-bad">{" / " + num(g.jobs_failed) + " failed"}</span> : null}</span> },
  { key: "posts", label: "posts", align: "right", sort: (g) => g.posts, render: (g) => num(g.posts) },
  { key: "votes", label: "votes", align: "right", hide: "sm", sort: (g) => g.votes, render: (g) => num(g.votes) },
  { key: "claims", label: "claims", align: "right", hide: "sm", sort: (g) => g.claims, render: (g) => <span className="tnum">{num(g.claims)}{g.active ? <span className="text-ok">{" (" + g.active + " now)"}</span> : null}</span> },
  { key: "sessions", label: "sessions", align: "right", hide: "md", sort: (g) => sessionCount(g), render: (g) => num(sessionCount(g)) },
  { key: "heartbeat", label: "last heartbeat", nowrap: true, sort: (g) => g.last_heartbeat_at, render: (g) => <When d={g.last_heartbeat_at} /> },
  { key: "useful", label: "last useful output", nowrap: true, sort: (g) => g.last_useful_at, render: (g) => <When d={g.last_useful_at} /> },
  { key: "last", label: "last activity", nowrap: true, sort: (g) => g.last_seen, render: (g) => <When d={g.last_seen} /> },
];

const isLead = (agent: string) => /lead$/.test(agent);
const presenceDot = (p: AgentPresence) => (p === "online" ? "bg-ok" : p === "idle" ? "bg-warn" : "bg-muted-foreground");

// A quiet second line under a Fact's number ("2 failed", "1 now").
function FactSub({ children, tone }: { children: React.ReactNode; tone: "ok" | "bad" }) {
  return <span className={"mt-1 block text-[11.5px] font-medium leading-none " + (tone === "ok" ? "text-ok" : "text-bad")}>{children}</span>;
}

function AgentMachinePanel({ r }: { r: Runner | undefined | null }) {
  if (!r) return <Panel title="The machine" icon={ServerIcon}>{<EmptyState title="Runner unavailable">The runner state is not available.</EmptyState>}</Panel>;
  if (!r.available) return <Panel title="The machine" icon={ServerIcon}>{<EmptyState title="No daemon state" hint="pulse daemon">No store yet, or no daemon has run. Start one with the CLI.</EmptyState>}</Panel>;
  const d = r.daemon;
  const stopped = r.runner && !r.runner.allowed;
  return (
    <Panel
      title="The machine"
      icon={ServerIcon}
      actions={
        <span className="flex items-center gap-1.5">
          <ToneBadge tone={stopped ? "bad" : d.state === "running" || d.state === "alive" ? "ok" : statusTone(d.state)} dot>
            {stopped ? "STOP" : d.state}
          </ToneBadge>
          <Link href={hrefs.settings("runner")} className="hover:underline">
            caps
          </Link>
        </span>
      }
    >
      <Kv
        rows={[
          ["daemon", d.text || d.state],
          ["last loop", <When key="at" d={d.at} />],
          ["slots", <span key="slots" className="tnum">{r.hands.free + " free of " + r.hands.slots + " on " + plural(r.hands.executors_active, "active executor")}</span>],
          ["workers", <span key="workers" className="tnum">{r.workers.busy + " busy · " + r.workers.idle + " idle" + (r.workers.idle_names.length ? " (" + r.workers.idle_names.join(", ") + ")" : "") + " · target " + r.workers.target}</span>],
          ["jobs", <span key="jobs" className="tnum">{r.jobs.queued + " queued · " + r.jobs.leased + " leased · " + r.jobs.running + " running" + (r.jobs.blocked ? " · " + r.jobs.blocked + " blocked" : "")}</span>],
          ["runs today", <span key="runs" className="tnum">{r.jobs.runs_today + (r.jobs.cap ? " of " + r.jobs.cap : " (no daily cap)")}</span>],
        ]}
      />
      {stopped ? <p className="mt-2 text-xs text-bad">{"The runner is stopped: " + (r.runner?.reason || "the stop switch is on") + "."}</p> : null}
      <LandingHolds holds={r.landing} />
      {r.executors.length ? (
        <ul className="mt-3 grid gap-1.5">
          {r.executors.map((e) => (
            <ExecutorRow key={e.name} e={e} />
          ))}
        </ul>
      ) : null}
    </Panel>
  );
}

// One agent's scorecard. Its active claims are listed inside, so in card
// view the card is also the "working now" list.
function AgentCard({ g }: { g: AgentGroup }) {
  const p = g.primary;
  const claims = g.presence === "online" ? g.sessions.flatMap((s) => s.active_claims || []) : [];
  const name = p.agent || p.name || g.agent;
  const runnerModel = [p.runner, p.model].filter(Boolean).join(" · ");
  const lead = isLead(g.agent);
  return (
    <ItemCard highlight={claims.length > 0} ariaLabel={g.agent} className="min-w-0">
      <ItemCardHeader>
        <PresenceBadge g={g} />
        {p.pruned_reason ? <ToneBadge tone="warn" className="h-auto max-w-full whitespace-normal break-words text-left">{"pruned: " + p.pruned_reason}</ToneBadge> : null}
        <ToneBadge tone={lead ? "accent" : "neutral"}>{lead ? "lead" : "worker"}</ToneBadge>
        <span className="text-muted-foreground">{p.kind}</span>
      </ItemCardHeader>
      <div className="flex min-w-0 items-center gap-2.5">
        <span className="relative shrink-0">
          <Avatar seed={name} size={36} />
          <span className={cn("absolute -right-0.5 -bottom-0.5 size-3 rounded-full ring-2 ring-card", presenceDot(g.presence))} aria-hidden />
        </span>
        <div className="min-w-0">
          <ItemCardTitle className="font-semibold">{name}</ItemCardTitle>
          {runnerModel ? <p className="truncate text-xs text-muted-foreground">{runnerModel}</p> : null}
        </div>
      </div>
      <p className="text-[13px] text-muted-foreground anywhere">Identity: {identityStatus(p)}. {g.presence_reason}</p>
      <AgentNow g={g} />
      {claims.length ? (
        <div className="grid gap-1 rounded-lg border border-ok/30 bg-ok/5 px-3 py-2">
          <div className="text-[10.5px] font-medium uppercase tracking-wider text-ok">working now</div>
          {claims.map((c) => (
            <ItemCardBody key={c.id} clamp={1} className="leading-snug">
              {c.slug ? <ProjectLink slug={c.slug} className="text-foreground" /> : null}
              {c.slug ? " " : null}
              {c.slug ? <ThreadLink slug={c.slug} k={c.thread_key} className="text-foreground" /> : <Mono className="text-foreground">{c.thread_key}</Mono>}
              {c.summary ? " · " + c.summary : null}
            </ItemCardBody>
          ))}
        </div>
      ) : null}
      <div className="grid grid-cols-4 gap-1.5">
        <Fact n={<>{num(g.jobs_done)}{g.jobs_failed ? <FactSub tone="bad">{num(g.jobs_failed) + " failed"}</FactSub> : null}</>} label="jobs" className="flex flex-col justify-between" />
        <Fact n={num(g.posts)} label="posts" className="flex flex-col justify-between" />
        <Fact n={num(g.votes)} label="votes" className="flex flex-col justify-between" />
        <Fact n={<>{num(g.claims)}{g.active ? <FactSub tone="ok">{num(g.active) + " now"}</FactSub> : null}</>} label="claims" className="flex flex-col justify-between" />
      </div>
      <Kv rows={[["last heartbeat", <When key="h" d={g.last_heartbeat_at} />], ["last useful output", <When key="u" d={g.last_useful_at} />]]} className="text-xs" />
      <ItemCardMeta className="mt-auto">
        {g.projects.map((s) => (
          <ProjectLink key={s} slug={s} className="rounded-full bg-muted px-2 text-xs text-foreground" />
        ))}
        <span className="ml-auto tnum">{plural(sessionCount(g), "session")}</span>
      </ItemCardMeta>
    </ItemCard>
  );
}

export default function AgentsPage() {
  const q = useApi<RosterRow[]>("roster", { refresh: 30000 });
  const now = useNow(1000);
  const runner = useApi<Runner>("runner");
  const [mode, setMode] = useViewMode("agents");
  const [status, setStatus] = useState("all");
  const [scope, setScope] = useState<"live" | "history" | "all">("live");
  const [text, setText] = useState("");
  const groups = useMemo(() => group(q.data ?? [], now), [q.data, now]);
  const statuses = useMemo(() => Array.from(new Set(groups.map((g) => identityStatus(g.primary)).filter(Boolean))).sort(), [groups]);
  const matching = useMemo(() => {
    const t = text.trim().toLowerCase();
    return groups.filter((g) => (status === "all" || identityStatus(g.primary) === status) && (!t || g.agent.toLowerCase().includes(t) || g.projects.some((p) => p.toLowerCase().includes(t))));
  }, [groups, status, text]);
  const rows = matching.filter((g) => scope === "all" || (scope === "live" ? live(g) : !live(g)));
  const liveCount = matching.filter(live).length;
  if (q.error)
    return (
      <>
        <PageTitle icon={BotIcon}>Agents</PageTitle>
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      </>
    );
  if (!q.data) return <SkeletonPage rows={3} />;
  const active = rows.filter((g) => g.active > 0);
  const r = runner.data;
  // Cards put whoever is working now first; the table keeps its own sort.
  const cards = [...rows].sort((a, b) => Number(b.active > 0) - Number(a.active > 0));
  const filters = (
    <div className="flex flex-wrap items-center gap-2 py-0.5">
      <div role="group" aria-label="Roster scope" className="inline-flex shrink-0 rounded-lg border bg-muted/40 p-0.5">
        {(["live", "history", "all"] as const).map((s) => (
          <button
            key={s}
            type="button"
            aria-pressed={scope === s}
            onClick={() => setScope(s)}
            className={cn(
              "inline-flex h-7 items-center gap-1.5 rounded-md px-2.5 text-xs font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
              scope === s ? "bg-card text-foreground shadow-card" : "text-muted-foreground hover:bg-muted hover:text-foreground",
            )}
          >
            {s === "live" ? "Live" : s === "history" ? "History" : "All"}
            <span className={cn("tnum rounded-full bg-muted px-1.5 text-[10.5px]", scope === s && "bg-primary/10 text-primary")}>{s === "live" ? liveCount : s === "history" ? matching.length - liveCount : matching.length}</span>
          </button>
        ))}
      </div>
      <Select value={status} onValueChange={setStatus}>
        <SelectTrigger aria-label="Status" className="h-8 w-32 shrink-0 sm:w-40">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all">all statuses</SelectItem>
          {statuses.map((s) => (
            <SelectItem key={s} value={s}>
              {s}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Input aria-label="Filter agents" placeholder="agent or project…" value={text} onChange={(e) => setText(e.target.value)} className="h-8 min-w-32 flex-1 sm:w-48 sm:flex-none" />
      <span className="ml-auto shrink-0 whitespace-nowrap">
        <Count>{plural(rows.length, "agent")}</Count>
      </span>
      <ViewToggle value={mode} onChange={setMode} className="shrink-0" />
    </div>
  );
  return (
    <>
      <PageTitle icon={BotIcon} sub="Live shows supervised online agents and idle reusable workers/leads. History keeps offline, unknown, legacy and pruned identities; nothing is deleted or retired by a missed heartbeat." bar={filters}>
        Agents
      </PageTitle>
      <Lead>One roster for who is live, who has history and how much work the machine can take right now.</Lead>
      <KpiGrid>
        <Kpi icon={BotIcon} n={rows.length} label="agents shown" sub={plural(matching.length, "matching agent")} />
        <Kpi icon={WifiIcon} n={rows.filter((g) => g.presence === "online").length} label="online shown" tone={rows.some((g) => g.presence === "online") ? "ok" : "neutral"} sub="heartbeat within five minutes" />
        <Kpi icon={CrownIcon} n={rows.filter((g) => isLead(g.agent)).length} label="leads shown" sub="reusable project identities" />
        <Kpi icon={LayersIcon} n={r ? r.workers.pool : "…"} label="worker pool" sub={r ? num(r.workers.idle) + " idle · " + num(r.workers.busy) + " busy · target " + num(r.workers.target) : "loading"} />
        <Kpi icon={LoaderIcon} n={r ? r.jobs.running : "…"} label="jobs running" sub={r ? num(r.jobs.queued) + " queued · " + num(r.jobs.blocked) + " blocked" : ""} />
        <Kpi icon={GaugeIcon} n={r ? r.jobs.runs_today + " / " + (r.jobs.cap || "∞") : "…"} label="runs today" sub="used / cap" />
        <Kpi icon={CircleDotIcon} n={r ? r.hands.free + " / " + r.hands.slots : "…"} label="free slots" sub={r ? plural(r.hands.executors_active, "executor") + " active" : ""} />
        <Kpi icon={TriangleAlertIcon} n={rows.reduce((a, g) => a + g.jobs_failed, 0)} label="jobs failed" tone={rows.some((g) => g.jobs_failed) ? "bad" : "neutral"} sub="shown agents" />
      </KpiGrid>
      <div className="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(300px,1fr)]">
        <div className="grid min-w-0 content-start gap-4">
          {!rows.length ? (
            <EmptyState title="No agents in this view" action={groups.length ? <Button size="sm" variant="outline" onClick={() => { setScope("all"); setStatus("all"); setText(""); }}>Show all agents</Button> : null}>{groups.length ? "Nothing matches this view. History or All keeps older identities available." : <>No agent yet. The first appears when the runner wakes a lead for a task or a worker for a job - or when someone posts with <Mono>pulse post</Mono>.</>}</EmptyState>
          ) : mode === "cards" ? (
            <CardGrid cols={3}>
              {cards.map((g) => (
                <AgentCard key={g.agent} g={g} />
              ))}
            </CardGrid>
          ) : (
            <div className="min-w-0 overflow-hidden rounded-lg border bg-card">
              <DataTable columns={COLUMNS} rows={rows} rowKey={(g) => g.agent} initialSort="last" caption="Agent scorecards" />
            </div>
          )}
          {/* The cards list each agent's claims themselves; the table only counts them. */}
          {mode === "table" && active.length ? (
            <Panel title="Working now">
              <ul className="grid gap-1.5 text-[13px]">
                {active.flatMap((g) => g.sessions.flatMap((s) => (s.active_claims || []).map((c) => (
                  <li key={g.agent + c.id} className="flex flex-wrap items-center gap-2 anywhere">
                    <Who row={s} compact />
                    {c.slug ? <ProjectLink slug={c.slug} className="text-xs" /> : null}
                    {c.slug ? <ThreadLink slug={c.slug} k={c.thread_key} className="text-xs" /> : <Mono className="text-xs">{c.thread_key}</Mono>}
                    <span className="text-muted-foreground">{c.summary}</span>
                  </li>
                ))))}
              </ul>
            </Panel>
          ) : null}
        </div>
        <div className="grid gap-4 content-start">
          <AgentMachinePanel r={r} />
          <Panel title="Executors" icon={HardDriveIcon} dense>
            {r?.executors?.length ? (
              <ul className="grid gap-1.5 p-3">
                {r.executors.map((e) => (
                  <ExecutorRow key={e.name} e={e} />
                ))}
              </ul>
            ) : (
              <EmptyState className="m-3">No executor yet - the daemon runs on this machine only.</EmptyState>
            )}
          </Panel>
        </div>
      </div>
    </>
  );
}
