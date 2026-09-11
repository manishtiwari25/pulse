"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { BotIcon, ClipboardIcon, ClockIcon, CopyIcon, FileTextIcon, PlayIcon, TerminalIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { DataTable, type Column } from "@/components/pulse/data-table";
import { Markdown } from "@/components/pulse/markdown";
import { EmptyState, ErrorNotice, Kpi, KpiGrid, Kv, PageTitle, Panel, SkeletonPage } from "@/components/pulse/panel";
import { Mono, ProjectLink, ToneBadge, When } from "@/components/pulse/who";
import type { HarnessSession, HarnessSessionResponse, HarnessSessionStatus, HarnessSessionsResponse, HarnessStep, ProjectsPayload } from "@/lib/api";
import { enc, query } from "@/lib/api";
import { num, parseDate, rel, span, statusTone, truncate, type Tone } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useApi, useLiveEvents, useNow } from "@/lib/use-api";
import { cn } from "@/lib/utils";

const RUNNING: HarnessSessionStatus[] = ["running"];
const FAILURES: HarnessSessionStatus[] = ["step_limit", "time_limit", "format_error", "provider_error", "not_logged_in", "cancelled", "lost"];
const STATUSES: { value: string; label: string }[] = [
  { value: "all", label: "All statuses" },
  { value: "running", label: "Running" },
  { value: "submitted", label: "Submitted" },
  { value: "step_limit", label: "Step limit" },
  { value: "time_limit", label: "Time limit" },
  { value: "format_error", label: "Format error" },
  { value: "provider_error", label: "Provider error" },
  { value: "not_logged_in", label: "Not logged in" },
  { value: "cancelled", label: "Cancelled" },
  { value: "lost", label: "Lost" },
];

function compact(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1).replace(/\.0$/, "") + "m";
  if (n >= 1_000) return (n / 1_000).toFixed(n >= 10_000 ? 0 : 1).replace(/\.0$/, "") + "k";
  return num(n);
}

function duration(start?: string, end?: string, now = Date.now()): string {
  const s = parseDate(start)?.getTime();
  if (!s) return "";
  const e = parseDate(end)?.getTime() || now;
  return span(Math.max(1, (e - s) / 1000));
}

function exitKey(session: Pick<HarnessSession, "status" | "exit_status">): string {
  return session.status === "running" ? "running" : session.exit_status || session.status;
}

function exitTone(session: Pick<HarnessSession, "status" | "exit_status">): Tone {
  const status = exitKey(session);
  if (status === "submitted") return "ok";
  if (status === "running") return "info";
  if (status === "step_limit" || status === "time_limit" || status === "format_error") return "warn";
  if (status === "provider_error" || status === "not_logged_in") return "bad";
  if (status === "cancelled" || status === "lost") return "neutral";
  return statusTone(status);
}

function HarnessStatusBadge({ session }: { session: Pick<HarnessSession, "status" | "exit_status"> }) {
  const status = exitKey(session);
  return (
    <ToneBadge tone={exitTone(session)} dot={status !== "lost" && status !== "cancelled"} className={cn(status === "running" && "[&>span:first-child]:animate-ping")}>
      {status.replace(/_/g, " ")}
    </ToneBadge>
  );
}

const EXIT_CARD: Record<Tone, string> = {
  neutral: "border-border bg-muted/30",
  ok: "border-ok/40 bg-ok/10",
  warn: "border-warn/40 bg-warn/10",
  bad: "border-bad/40 bg-bad/10",
  info: "border-info/40 bg-info/10",
  accent: "border-accent-2/40 bg-accent-2/10",
};

function executorName(executor: string | undefined): string {
  return (executor || "").trim();
}

// The badge names remote hands only; every session has some executor.
function remoteExecutor(executor: string | undefined): string {
  const e = executorName(executor);
  return e && e !== "local" ? e : "";
}

function updateSearch(pathname: string, router: ReturnType<typeof useRouter>, next: Record<string, string>) {
  const clean = Object.fromEntries(Object.entries(next).filter(([, v]) => v && v !== "all"));
  router.replace(pathname + query(clean), { scroll: false });
}

function Filters({ project, status, agent, executor, executors, projects, onChange, onExecutorChange, onClear }: { project: string; status: string; agent: string; executor: string; executors: string[]; projects: ProjectsPayload | undefined; onChange: (next: { project: string; status: string; agent: string }) => void; onExecutorChange: (next: string) => void; onClear: () => void }) {
  const [agentState, setAgentState] = useState({ value: agent, draft: agent });
  const agentDraft = agentState.value === agent ? agentState.draft : agent;
  const setAgentDraft = (draft: string) => setAgentState({ value: agent, draft });
  const commitAgent = () => onChange({ project, status, agent: agentDraft.trim() });
  const hasFilters = !!(project || status || agent || executor);
  return (
    <div className="flex min-w-0 flex-wrap items-end gap-2">
      <div className="grid gap-1">
        <Label>Project</Label>
        <Select value={project || "all"} onValueChange={(v) => onChange({ project: v === "all" ? "" : v, status, agent })}>
          <SelectTrigger className="w-44">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All projects</SelectItem>
            {(projects?.projects ?? []).map((p) => (
              <SelectItem key={p.slug} value={p.slug}>
                {p.slug}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="grid gap-1">
        <Label>Status</Label>
        <Select value={status || "all"} onValueChange={(v) => onChange({ project, status: v === "all" ? "" : v, agent })}>
          <SelectTrigger className="w-44">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {STATUSES.map((s) => (
              <SelectItem key={s.value} value={s.value}>
                {s.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="grid gap-1">
        <Label htmlFor="harness-agent">Agent</Label>
        <Input
          id="harness-agent"
          value={agentDraft}
          placeholder="agent name"
          onChange={(e) => setAgentDraft(e.target.value)}
          onBlur={commitAgent}
          onKeyDown={(e) => {
            if (e.key === "Enter") commitAgent();
            if (e.key === "Escape") setAgentDraft(agent);
          }}
          className="w-44"
        />
      </div>
      {executors.length ? (
        <div className="grid gap-1">
          <Label>Executor</Label>
          <Select value={executor || "all"} onValueChange={(v) => onExecutorChange(v === "all" ? "" : v)}>
            <SelectTrigger className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All executors</SelectItem>
              {executors.map((e) => (
                <SelectItem key={e} value={e}>
                  on {e}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      ) : null}
      <Button type="button" variant="outline" size="sm" onClick={onClear} disabled={!hasFilters} className="mb-0.5">
        Clear filters
      </Button>
    </div>
  );
}

function SessionList({ rows, selected, onSelect }: { rows: HarnessSession[]; selected: string; onSelect: (id: string) => void }) {
  const now = useNow();
  const columns = useMemo<Column<HarnessSession>[]>(
    () => [
      { key: "status", label: "status", sort: (r) => exitKey(r), render: (r) => <HarnessStatusBadge session={r} /> },
      {
        key: "job",
        label: "job",
        sort: (r) => r.job_title,
        render: (r) => (
          <span className="grid gap-0.5">
            <span className="font-medium text-foreground">{r.project && r.job_id && /^TASK-/.test(r.job_id) ? <Link href={hrefs.task(r.project, r.job_id)} className="hover:underline">{truncate(r.job_title || r.job_id, 72)}</Link> : truncate(r.job_title || r.job_id, 72)}</span>
            <span className="flex flex-wrap gap-1.5 text-xs text-muted-foreground">
              <ProjectLink slug={r.project} className="text-xs" />
              <Mono>{r.job_id}</Mono>
              {remoteExecutor(r.executor) ? <ToneBadge tone="neutral">on {remoteExecutor(r.executor)}</ToneBadge> : null}
              {r.id === selected ? <ToneBadge tone="info">Selected</ToneBadge> : null}
            </span>
          </span>
        ),
      },
      { key: "agent", label: "agent", sort: (r) => r.agent, render: (r) => <span className="text-xs">{r.agent}</span> },
      { key: "model", label: "model", hide: "lg", sort: (r) => r.model, render: (r) => <Mono>{r.model}</Mono> },
      { key: "steps", label: "steps", align: "right", sort: (r) => r.steps, render: (r) => compact(r.steps) + " / " + compact(r.calls) },
      { key: "tokens", label: "tokens", align: "right", hide: "md", sort: (r) => r.prompt_tokens + r.completion_tokens, render: (r) => compact(r.prompt_tokens + r.completion_tokens) },
      {
        key: "cost",
        label: (
          <Tooltip>
            <TooltipTrigger asChild>
              <span>cost</span>
            </TooltipTrigger>
            <TooltipContent>provider units - Copilot: premium requests</TooltipContent>
          </Tooltip>
        ),
        align: "right",
        hide: "md",
        sort: (r) => r.cost,
        render: (r) => (r.cost ? num(r.cost) : "—"),
      },
      { key: "started", label: "started", nowrap: true, sort: (r) => r.started_at, render: (r) => <When d={r.started_at} /> },
      { key: "updated", label: "updated", nowrap: true, hide: "sm", sort: (r) => r.updated_at, render: (r) => rel(r.updated_at, now) },
      { key: "duration", label: "duration", align: "right", hide: "md", sort: (r) => Date.parse(r.ended_at || "") || Date.now() - (Date.parse(r.started_at) || Date.now()), render: (r) => duration(r.started_at, r.ended_at, now) },
    ],
    [now, selected],
  );
  return (
    <DataTable
      columns={columns}
      rows={rows}
      rowKey={(r) => r.id}
      onRow={(r) => onSelect(r.id)}
      dense
      caption="Harness sessions"
      initialSort="updated"
      rowAriaSelected={(r) => r.id === selected}
      rowClassName={(r) => (r.id === selected ? "border-l-2 border-l-info bg-primary/5" : undefined)}
      emptyText="No harness sessions match these filters."
    />
  );
}

function StepTokens({ s }: { s: HarnessStep }) {
  const total = (s.prompt_tokens || 0) + (s.completion_tokens || 0);
  if (!total && !s.model) return null;
  return (
    <span className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
      {s.model ? <Mono>{s.model}</Mono> : null}
      {total ? <span className="tnum">{compact(total)} tokens</span> : null}
    </span>
  );
}

function Observation({ s }: { s: HarnessStep }) {
  const [open, setOpen] = useState((s.content || "").split("\n").length <= 30);
  const lines = (s.content || "").split("\n");
  const long = lines.length > 30;
  const shown = open ? s.content : lines.slice(0, 30).join("\n");
  const bad = (s.exit_code ?? 0) !== 0;
  return (
    <article className="rounded-lg border bg-muted/30">
      <header className="flex flex-wrap items-center justify-between gap-2 border-b px-3 py-2">
        <span className="flex min-w-0 items-center gap-2">
          <TerminalIcon className="size-4 text-muted-foreground" />
          <Mono className="truncate">{s.command || "observation"}</Mono>
        </span>
        <span className="flex items-center gap-1.5">
          {s.duration_ms ? <ToneBadge tone="neutral">{s.duration_ms} ms</ToneBadge> : null}
          {s.exit_code !== null ? <ToneBadge tone={bad ? "bad" : "ok"}>{"exit " + s.exit_code}</ToneBadge> : null}
        </span>
      </header>
      <pre className="max-h-[52vh] overflow-auto whitespace-pre-wrap break-words p-3 font-mono text-xs leading-relaxed">{shown || "(no output)"}</pre>
      {long ? (
        <button type="button" className="border-t px-3 py-2 text-xs text-muted-foreground hover:text-foreground" onClick={() => setOpen((v) => !v)}>
          {open ? "Collapse output" : "Show all " + num(lines.length) + " lines"}
        </button>
      ) : null}
    </article>
  );
}

function ReportLines({ text }: { text: string }) {
  const keys = /^(SUMMARY|FINDING|SUGGESTION|QUESTION|VOTE):/;
  return (
    <div className="grid gap-1">
      {text.split("\n").map((line, i) => {
        const hit = keys.exec(line);
        return (
          <div key={i} className={cn("rounded-md px-2 py-1 text-sm", hit && "border bg-card")}>
            {hit ? <ToneBadge tone={hit[1] === "FINDING" ? "warn" : hit[1] === "SUMMARY" ? "ok" : "info"} className="mr-2">{hit[1]}</ToneBadge> : null}
            <span className="whitespace-pre-wrap">{hit ? line.slice(hit[0].length).trimStart() : line}</span>
          </div>
        );
      })}
    </div>
  );
}

function StepCard({ s, session }: { s: HarnessStep; session: HarnessSession }) {
  const head = (
    <div className="mb-1 flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
      <span className="tnum">step {s.n}</span>
      <span className="flex flex-wrap items-center gap-2">
        {s.at ? <When d={s.at} /> : null}
        <StepTokens s={s} />
      </span>
    </div>
  );
  if (s.kind === "assistant")
    return (
      <article className="rounded-xl border bg-card p-3 shadow-card">
        {head}
        <Markdown source={s.content} />
      </article>
    );
  if (s.kind === "command")
    return (
      <article className="rounded-xl border bg-zinc-950 p-3 text-zinc-100 shadow-card dark:bg-black">
        {head}
        <pre className="whitespace-pre-wrap break-words font-mono text-xs">
          <span className="text-emerald-300">$</span> {s.command}
        </pre>
      </article>
    );
  if (s.kind === "observation") return <Observation s={s} />;
  if (s.kind === "note")
    return (
      <article className="rounded-xl border border-dashed bg-muted/30 p-3 text-muted-foreground">
        {head}
        <p className="whitespace-pre-wrap text-sm">{s.content}</p>
      </article>
    );
  if (s.kind === "error")
    return (
      <article className="rounded-xl border border-warn/40 bg-warn/10 p-3">
        {head}
        <p className="whitespace-pre-wrap text-sm">{s.content}</p>
      </article>
    );
  return (
    <article className={cn("rounded-xl border p-3", EXIT_CARD[exitTone(session)])}>
      {head}
      <div className="grid gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <HarnessStatusBadge session={session} />
          <span className="font-medium">{s.content || session.exit_status || "finished"}</span>
        </div>
        {session.status === "submitted" && session.submission ? <ReportLines text={session.submission} /> : null}
      </div>
    </article>
  );
}

function transcriptText(d: HarnessSessionResponse): string {
  const lines = [
    `${d.session.id} ${d.session.status}`,
    `${d.session.project} ${d.session.job_id} ${d.session.job_title}`,
    `${d.session.agent} ${d.session.provider}/${d.session.model}`,
    "",
  ];
  for (const s of d.steps) {
    lines.push(`## ${s.n} ${s.kind} ${s.at || ""}`.trim());
    if (s.command) lines.push("$ " + s.command);
    if (s.content) lines.push(s.content);
    if (s.exit_code !== null) lines.push("exit " + s.exit_code);
    lines.push("");
  }
  if (d.session.submission) lines.push("## submission", d.session.submission);
  return lines.join("\n");
}

function Transcript({ id }: { id: string }) {
  const q = useApi<HarnessSessionResponse>(id ? "harness/sessions/" + enc(id) : null, { refresh: 5000 });
  const [follow, setFollow] = useState(true);
  const bottom = useRef<HTMLDivElement | null>(null);
  const d = q.data;
  const running = d?.session.status === "running";
  const now = useNow();
  useEffect(() => {
    if (follow && running) bottom.current?.scrollIntoView({ block: "end" });
  }, [d?.steps.length, follow, running]);
  if (q.error && !d) return <ErrorNotice error={q.error} onRetry={() => void q.reload()} />;
  if (!d) return <SkeletonPage rows={2} />;
  const s = d.session;
  const totalTokens = s.prompt_tokens + s.completion_tokens;
  return (
    <Panel
      title="Transcript"
      icon={FileTextIcon}
      actions={
        <>
          <Button size="sm" variant={follow ? "secondary" : "outline"} onClick={() => setFollow((v) => !v)}>
            <PlayIcon /> {follow ? "Following" : "Follow"}
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() =>
              navigator.clipboard
                .writeText(transcriptText(d))
                .then(() => toast.success("Transcript copied."))
                .catch(() => toast.error("Could not copy the transcript."))
            }
          >
            <CopyIcon /> Copy as text
          </Button>
        </>
      }
    >
      <div className="grid gap-4">
        <div className="grid gap-3 rounded-lg border bg-muted/30 p-3" aria-live={running ? "polite" : undefined}>
          <div className="flex flex-wrap items-center gap-2">
            <HarnessStatusBadge session={s} />
            <span className="font-medium">{s.agent}</span>
            {s.executor ? <span className="text-muted-foreground">on <Mono>{s.executor}</Mono></span> : null}
            <span className="text-muted-foreground">{s.job_title}</span>
          </div>
          <Kv
            rows={[
              ["project", <ProjectLink key="p" slug={s.project} />],
              ["job", <Mono key="j">{s.job_id}</Mono>],
              ["model", <span key="m" className="inline-flex flex-wrap gap-1.5"><Mono>{s.provider}</Mono><Mono>{s.model}</Mono></span>],
              ...(s.executor ? [["executor", <Mono key="e">{s.executor}</Mono>] as [React.ReactNode, React.ReactNode]] : []),
              ["cwd", <Mono key="cwd">{s.cwd}</Mono>],
              [
                "trajectory",
                <span key="t" className="inline-flex min-w-0 flex-wrap items-center gap-1.5">
                  <Mono>{s.trajectory}</Mono>
                  <Button size="icon-xs" variant="ghost" aria-label="Copy trajectory path" onClick={() => navigator.clipboard.writeText(s.trajectory).then(() => toast.success("Trajectory path copied."))}>
                    <ClipboardIcon />
                  </Button>
                </span>,
              ],
              ["elapsed", <span key="el" className="tnum">{duration(s.started_at, s.ended_at, now)}</span>],
              ["tokens", <span key="tok" className="tnum">{compact(totalTokens)} ({compact(s.prompt_tokens)} prompt · {compact(s.completion_tokens)} completion)</span>],
            ]}
          />
          {s.error ? <p className="rounded-md border border-bad/30 bg-bad/10 px-2 py-1.5 text-sm text-bad">{s.error}</p> : null}
        </div>
        <div className="grid gap-3">{d.steps.map((step) => <StepCard key={step.n} s={step} session={s} />)}</div>
        <div ref={bottom} />
      </div>
    </Panel>
  );
}

export function HarnessPageClient() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const project = sp.get("project") || "";
  const status = sp.get("status") || "";
  const agent = sp.get("agent") || "";
  const selected = sp.get("id") || "";
  const [limit, setLimit] = useState(50);
  const [executor, setExecutor] = useState("");
  const path = "harness/sessions" + query({ project, status, agent, limit });
  const sessions = useApi<HarnessSessionsResponse>(path, { refresh: 5000 });
  const projects = useApi<ProjectsPayload>("projects", { refresh: 60000 });
  const { events } = useLiveEvents();
  const loadedRows = useMemo(() => sessions.data?.sessions ?? [], [sessions.data]);
  const executors = useMemo(() => Array.from(new Set(loadedRows.map((r) => executorName(r.executor)).filter(Boolean))).sort(), [loadedRows]);
  const rows = useMemo(() => (executor ? loadedRows.filter((r) => executorName(r.executor) === executor) : loadedRows), [executor, loadedRows]);
  const running = rows.filter((r) => RUNNING.includes(r.status)).length;
  const failed = rows.filter((r) => FAILURES.includes(r.status)).length;
  const submitted = rows.filter((r) => r.status === "submitted").length;
  const eventKinds = useMemo(() => events.filter((e) => e.action.startsWith("harness.")).slice(0, 3), [events]);
  const choose = useCallback(
    (id: string) => updateSearch(pathname, router, { project, status, agent, id }),
    [pathname, router, project, status, agent],
  );
  const selectedKnown = selected && rows.some((r) => r.id === selected);
  const canLoadMore = loadedRows.length >= limit && limit < 500;
  return (
    <>
      <PageTitle
        icon={TerminalIcon}
        sub="Every native harness run is recorded here: model calls, commands, observations, policy errors and the final submission."
        actions={
          <Button asChild variant="outline" size="sm">
            <Link href={hrefs.settings("harness")}>Settings → Harness</Link>
          </Button>
        }
        bar={
          <Filters
            project={project}
            status={status}
            agent={agent}
            executor={executor}
            executors={executors}
            projects={projects.data}
            onChange={(next) => updateSearch(pathname, router, { ...next, id: selected })}
            onExecutorChange={setExecutor}
            onClear={() => {
              setExecutor("");
              updateSearch(pathname, router, { id: selected });
            }}
          />
        }
      >
        Harness
      </PageTitle>
      {sessions.error && !sessions.data ? <ErrorNotice error={sessions.error} onRetry={() => void sessions.reload()} /> : null}
      {!sessions.data ? (
        <SkeletonPage rows={3} />
      ) : rows.length === 0 ? (
        <EmptyState
          title="No harness runs yet"
          icon={<BotIcon className="size-4" />}
          action={
            <Button asChild size="sm" variant="outline">
              <Link href={hrefs.settings("harness")}>Open Harness settings</Link>
            </Button>
          }
        >
          The harness records every run here once an agent works. Sign in with GitHub, choose models, and the first job will leave a transcript.
        </EmptyState>
      ) : (
        <div className="grid gap-4">
          <KpiGrid>
            <Kpi n={rows.length} label="sessions shown" icon={FileTextIcon} />
            <Kpi n={running} label="running" tone={running ? "info" : "neutral"} icon={PlayIcon} />
            <Kpi n={submitted} label="submitted" tone={submitted ? "ok" : "neutral"} icon={ClipboardIcon} />
            <Kpi n={failed} label="attention" tone={failed ? "bad" : "neutral"} icon={ClockIcon} />
          </KpiGrid>
          <div className="grid gap-4 xl:grid-cols-[minmax(0,0.95fr)_minmax(420px,1.05fr)]">
            <Panel
              title="Sessions"
              icon={BotIcon}
              actions={
                eventKinds.length ? (
                  <span className="hidden items-center gap-1.5 lg:flex">
                    {eventKinds.map((e) => (
                      <ToneBadge key={e.id} tone="info">{e.action}</ToneBadge>
                    ))}
                  </span>
                ) : null
              }
              dense
            >
              <div className="grid gap-3 p-3">
                <SessionList rows={rows} selected={selected} onSelect={choose} />
                <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
                  <span>
                    Showing {num(rows.length)} of the newest {num(loadedRows.length)}
                  </span>
                  {canLoadMore ? (
                    <Button type="button" size="sm" variant="outline" onClick={() => setLimit((n) => Math.min(500, n + 50))}>
                      Load more
                    </Button>
                  ) : null}
                </div>
              </div>
            </Panel>
            {selected && (selectedKnown || !rows.length) ? (
              <Transcript id={selected} />
            ) : (
              <Panel title="Transcript" icon={FileTextIcon}>
                <EmptyState>Select a session to read its transcript</EmptyState>
              </Panel>
            )}
          </div>
        </div>
      )}
    </>
  );
}
