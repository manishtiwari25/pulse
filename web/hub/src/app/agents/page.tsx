"use client";

import { useMemo, useState } from "react";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ExecutorRow, MachinePanel } from "@/components/pulse/cards";
import { DataTable, type Column } from "@/components/pulse/data-table";
import { CardGrid, Fact, ItemCard, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Count, EmptyState, ErrorNotice, Kpi, KpiGrid, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { useViewMode, ViewToggle } from "@/components/pulse/view-toggle";
import { Mono, ProjectLink, StatusBadge, ThreadLink, ToneBadge, When, Who } from "@/components/pulse/who";
import type { RosterRow, Runner } from "@/lib/api";
import { num, plural } from "@/lib/format";
import { useApi } from "@/lib/use-api";

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
}

// The roster comes one row per (agent, session); the scorecard is per
// agent, so fold sessions together and keep the busiest row as the face.
function group(rows: RosterRow[]): AgentGroup[] {
  const by = new Map<string, RosterRow[]>();
  for (const r of rows) {
    const k = r.agent || r.name || "?";
    by.set(k, [...(by.get(k) || []), r]);
  }
  return Array.from(by.entries())
    .map(([agent, rs]) => {
      const sorted = [...rs].sort((a, b) => b.posts - a.posts || (a.last_seen < b.last_seen ? 1 : -1));
      const sum = (f: (r: RosterRow) => number) => rs.reduce((a, r) => a + (f(r) || 0), 0);
      return {
        agent,
        primary: sorted[0]!,
        sessions: rs,
        posts: sum((r) => r.posts),
        votes: sum((r) => r.votes),
        claims: sum((r) => r.claims),
        jobs_done: sum((r) => r.jobs_done),
        jobs_failed: sum((r) => r.jobs_failed),
        projects: Array.from(new Set(rs.flatMap((r) => r.projects || []))).sort(),
        last_seen: rs.map((r) => r.last_seen).sort().reverse()[0] || "",
        active: rs.reduce((a, r) => a + (r.active_claims?.length || 0), 0),
      };
    })
    .sort((a, b) => (a.last_seen < b.last_seen ? 1 : -1));
}

const COLUMNS: Column<AgentGroup>[] = [
  { key: "agent", label: "agent", sort: (g) => g.agent, render: (g) => <Who row={g.primary} /> },
  { key: "status", label: "status", sort: (g) => g.primary.agent_status || g.primary.status, render: (g) => <span className="inline-flex flex-wrap gap-1"><StatusBadge status={g.primary.agent_status || g.primary.status} />{g.primary.pruned_reason ? <ToneBadge tone="warn">{"pruned: " + g.primary.pruned_reason}</ToneBadge> : null}</span> },
  { key: "kind", label: "kind", hide: "md", render: (g) => <span className="text-xs text-muted-foreground">{g.primary.kind}</span> },
  { key: "runner", label: "runner / model", hide: "lg", render: (g) => <span className="text-xs text-muted-foreground anywhere">{[g.primary.runner, g.primary.model].filter(Boolean).join(" · ")}</span> },
  { key: "projects", label: "projects", render: (g) => <span className="inline-flex flex-wrap gap-1">{g.projects.map((p) => <ProjectLink key={p} slug={p} className="text-xs" />)}</span> },
  { key: "jobs", label: "jobs", align: "right", sort: (g) => g.jobs_done, render: (g) => <span className="tnum">{num(g.jobs_done)}{g.jobs_failed ? <span className="text-bad">{" / " + num(g.jobs_failed) + " failed"}</span> : null}</span> },
  { key: "posts", label: "posts", align: "right", sort: (g) => g.posts, render: (g) => num(g.posts) },
  { key: "votes", label: "votes", align: "right", hide: "sm", sort: (g) => g.votes, render: (g) => num(g.votes) },
  { key: "claims", label: "claims", align: "right", hide: "sm", sort: (g) => g.claims, render: (g) => <span className="tnum">{num(g.claims)}{g.active ? <span className="text-ok">{" (" + g.active + " now)"}</span> : null}</span> },
  { key: "sessions", label: "sessions", align: "right", hide: "md", sort: (g) => g.sessions.length, render: (g) => num(g.sessions.reduce((a, r) => a + (r.session_count || 1), 0)) },
  { key: "last", label: "last seen", nowrap: true, sort: (g) => g.last_seen, render: (g) => <When d={g.last_seen} /> },
];

const isLead = (agent: string) => /lead$/.test(agent);
const sessionCount = (g: AgentGroup) => g.sessions.reduce((a, r) => a + (r.session_count || 1), 0);

// A quiet second line under a Fact's number ("2 failed", "1 now").
function FactSub({ children, tone }: { children: React.ReactNode; tone: "ok" | "bad" }) {
  return <span className={"mt-1 block text-[10.5px] font-medium leading-none " + (tone === "ok" ? "text-ok" : "text-bad")}>{children}</span>;
}

// One agent's scorecard. Its active claims are listed inside, so in card
// view the card is also the "working now" list.
function AgentCard({ g }: { g: AgentGroup }) {
  const p = g.primary;
  const claims = g.sessions.flatMap((s) => s.active_claims || []);
  return (
    <ItemCard highlight={claims.length > 0} ariaLabel={g.agent} className="min-w-0">
      <ItemCardHeader right={<When d={g.last_seen} />}>
        <StatusBadge status={p.agent_status || p.status} />
        {p.pruned_reason ? <ToneBadge tone="warn">{"pruned: " + p.pruned_reason}</ToneBadge> : null}
        {isLead(g.agent) ? <ToneBadge tone="accent">lead</ToneBadge> : null}
        <span className="text-muted-foreground">{p.kind}</span>
      </ItemCardHeader>
      <ItemCardTitle>
        <Who row={p} />
      </ItemCardTitle>
      {claims.length ? (
        <div className="grid gap-0.5 rounded-md border border-ok/30 bg-ok/5 px-2.5 py-1.5">
          <div className="text-[10.5px] font-medium uppercase tracking-wide text-ok">working now</div>
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
      <ItemCardMeta className="mt-auto">
        {g.projects.map((s) => (
          <ProjectLink key={s} slug={s} className="text-foreground" />
        ))}
        <span className="ml-auto">{plural(sessionCount(g), "session")}</span>
      </ItemCardMeta>
    </ItemCard>
  );
}

export default function AgentsPage() {
  const q = useApi<RosterRow[]>("roster", { refresh: 30000 });
  const runner = useApi<Runner>("runner");
  const [mode, setMode] = useViewMode("agents");
  const [status, setStatus] = useState("all");
  const [text, setText] = useState("");
  const groups = useMemo(() => group(q.data ?? []), [q.data]);
  const statuses = useMemo(() => Array.from(new Set(groups.map((g) => g.primary.agent_status || g.primary.status).filter(Boolean))).sort(), [groups]);
  const rows = useMemo(() => {
    const t = text.trim().toLowerCase();
    return groups.filter((g) => (status === "all" || (g.primary.agent_status || g.primary.status) === status) && (!t || g.agent.toLowerCase().includes(t) || g.projects.some((p) => p.includes(t))));
  }, [groups, status, text]);
  if (q.error && !q.data)
    return (
      <>
        <PageTitle>Agents</PageTitle>
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      </>
    );
  if (!q.data) return <SkeletonPage rows={3} />;
  const active = groups.filter((g) => g.active > 0);
  const r = runner.data;
  // Cards put whoever is working now first; the table keeps its own sort.
  const cards = [...rows].sort((a, b) => Number(b.active > 0) - Number(a.active > 0));
  const filters = (
    <div className="flex items-center gap-2 py-0.5">
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
      <Input aria-label="Filter agents" placeholder="agent or project…" value={text} onChange={(e) => setText(e.target.value)} className="h-8 flex-1 sm:w-56 sm:flex-none" />
      <span className="ml-auto shrink-0 whitespace-nowrap">
        <Count>{plural(rows.length, "agent")}</Count>
      </span>
      <ViewToggle value={mode} onChange={setMode} className="shrink-0" />
    </div>
  );
  return (
    <>
      <PageTitle sub="Everyone who has posted on a board: the leads the runner wakes for you, the workers it spawns for jobs, and the sessions behind them." bar={filters}>
        Agents
      </PageTitle>
      <KpiGrid>
        <Kpi n={groups.length} label="agents" sub={plural(q.data.length, "roster row")} />
        <Kpi n={active.length} label="working now" tone={active.length ? "ok" : "neutral"} sub="with an active claim" />
        <Kpi n={groups.filter((g) => isLead(g.agent)).length} label="leads" sub="one per project" />
        <Kpi n={r ? r.workers.pool : "…"} label="worker pool" sub={r ? num(r.workers.idle) + " idle · " + num(r.workers.busy) + " busy · target " + num(r.workers.target) : "loading"} />
        <Kpi n={r ? r.jobs.running : "…"} label="jobs running" sub={r ? num(r.jobs.queued) + " queued · " + num(r.jobs.blocked) + " blocked" : ""} />
        <Kpi n={r ? r.jobs.runs_today + " / " + (r.jobs.cap || "∞") : "…"} label="runs today" sub="used / cap" />
        <Kpi n={r ? r.hands.free + " / " + r.hands.slots : "…"} label="free hands" sub={r ? plural(r.hands.executors_active, "executor") + " active" : ""} />
        <Kpi n={groups.reduce((a, g) => a + g.jobs_failed, 0)} label="jobs failed" tone={groups.some((g) => g.jobs_failed) ? "bad" : "neutral"} sub="all agents" />
      </KpiGrid>
      <div className="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(300px,1fr)]">
        <div className="grid min-w-0 content-start gap-4">
          {!rows.length ? (
            <EmptyState>{groups.length ? "Nothing matches this filter." : <>No agent yet. The first appears when the runner wakes a lead for a task or a worker for a job - or when someone posts with <Mono>pulse post</Mono>.</>}</EmptyState>
          ) : mode === "cards" ? (
            <CardGrid cols={3}>
              {cards.map((g) => (
                <AgentCard key={g.agent} g={g} />
              ))}
            </CardGrid>
          ) : (
            <div className="min-w-0 rounded-lg border bg-card">
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
          <MachinePanel r={r} />
          <Panel title="Executors" dense>
            {r?.executors?.length ? (
              <ul className="divide-y">
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
