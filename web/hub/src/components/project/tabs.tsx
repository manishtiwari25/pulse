"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { ActivityChart } from "@/components/pulse/charts";
import { AmendmentCard, FeedList, ProposalCard, SuggestionCard, WorkingNow } from "@/components/pulse/cards";
import { DataTable } from "@/components/pulse/data-table";
import { DocRev, SeededMarker, SelfImprovementPanel } from "@/components/pulse/evolve";
import { LiveFeed } from "@/components/pulse/live-feed";
import { Markdown } from "@/components/pulse/markdown";
import { Count, EmptyState, ErrorNotice, Kpi, KpiGrid, Kv, Notice, Panel, SkeletonRows, Toolbar } from "@/components/pulse/panel";
import { useViewMode, ViewToggle } from "@/components/pulse/view-toggle";
import { Check, Mono, ProjectLink, StatusBadge, ThreadLink, ToneBadge, When, Who } from "@/components/pulse/who";
import { THREAD_COLUMNS, ThreadCards } from "@/components/board/sheets";
import { ChatPane } from "@/components/chat/chat-pane";
import { TaskCards } from "@/components/tasks/task-card";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { TASK_COLUMNS } from "@/components/tasks/task-sheet";
import { RECORD_COLUMNS } from "@/components/records/records";
import { hrefs } from "@/lib/nav";
import { cn } from "@/lib/utils";
import { num, plural, statusTone } from "@/lib/format";
import { isBaselineRow, projectPath, TASK_STATUSES, type Amendment, type Constitution, type Evolution, type EvolutionArchiveRow, type MessagesPayload, type Notices, type Overview, type ProjectInfo, type Proposal, type RecordsPayload, type Suggestion, type Task, type TasksPayload, type ThreadRow, type Timeline } from "@/lib/api";
import { useApi, type Loaded } from "@/lib/use-api";

function Wait<T>({ q, rows = 5, children }: { q: Loaded<T>; rows?: number; children: (d: T) => React.ReactNode }) {
  if (q.error && !q.data) return <ErrorNotice error={q.error} onRetry={() => void q.reload()} />;
  if (q.data === undefined) return <SkeletonRows n={rows} />;
  return <>{children(q.data)}</>;
}

// A one-of filter as a row of chips: every choice is visible and one tap
// away, which a Select is not on a phone. Counts ride along when known.
function Chips({ label, value, onChange, options, className }: { label: string; value: string; onChange: (v: string) => void; options: { id: string; label: string; n?: number }[]; className?: string }) {
  return (
    <div role="group" aria-label={label} className={cn("flex flex-wrap items-center gap-1", className)}>
      {options.map((o) => {
        const on = o.id === value;
        return (
          <Button key={o.id} size="sm" variant={on ? "default" : "outline"} aria-pressed={on} onClick={() => onChange(o.id)} className="h-7 rounded-full px-2.5">
            {o.label}
            {o.n !== undefined ? <span className={cn("tnum text-[11px]", on ? "opacity-75" : "text-muted-foreground")}>{num(o.n)}</span> : null}
          </Button>
        );
      })}
    </div>
  );
}

const byNewest = <T,>(d: (x: T) => string) => (a: T, b: T) => (d(a) < d(b) ? 1 : d(a) > d(b) ? -1 : 0);
const isActiveTask = (x: Task) => x.status !== "done" && x.status !== "cancelled";
const NONE: never[] = [];

// ----- overview -----------------------------------------------------------

export function OverviewTab({ slug }: { slug: string }) {
  const ov = useApi<Overview>(projectPath(slug, "overview"));
  const info = useApi<ProjectInfo>(projectPath(slug, "project"), { refresh: 60000 });
  const tasks = useApi<TasksPayload>("tasks?project=" + encodeURIComponent(slug));
  const tl = useApi<Timeline>(projectPath(slug, "timeline"), { refresh: 60000 });
  return (
    <Wait q={ov} rows={8}>
      {(o) => {
        const open = (tasks.data?.tasks ?? []).filter((t) => t.status !== "done" && t.status !== "cancelled");
        const i = info.data;
        const bp = i?.board_params;
        return (
          <div className="grid gap-4">
            <KpiGrid>
              <Kpi n={o.proposals_awaiting} label="for you" sub="proposals waiting" tone={o.proposals_awaiting ? "ok" : "neutral"} href={hrefs.project(slug, "proposals")} />
              <Kpi n={open.length} label="open tasks" sub={tasks.data ? plural(open.filter((t) => t.status === "in_progress").length, "in progress") : "loading"} href={hrefs.project(slug, "tasks")} />
              <Kpi n={o.claims_active} label="working now" sub="active claims" tone={o.claims_active ? "ok" : "neutral"} />
              <Kpi n={o.threads_open} label="open threads" sub={num(o.threads_stale) + " stale"} tone={o.threads_stale ? "warn" : "neutral"} href={hrefs.project(slug, "threads")} />
              <Kpi n={o.suggestions_eligible} label="eligible" sub="suggestions past quorum" tone={o.suggestions_eligible ? "accent" : "neutral"} href={hrefs.project(slug, "suggestions")} />
              <Kpi n={o.suggestions_needing_votes} label="need votes" sub={"quorum " + num(o.quorum)} tone={o.suggestions_needing_votes ? "warn" : "neutral"} href={hrefs.project(slug, "suggestions")} />
              <Kpi n={o.amendments_open} label="amendments" sub={num(o.amendments_passed) + " passed"} href={hrefs.project(slug, "amendments")} />
              <Kpi n={o.posts} label="posts" sub={num(o.posts_without_provenance) + " without provenance"} tone={o.posts_without_provenance ? "warn" : "neutral"} />
            </KpiGrid>
            {!o.board_present ? (
              <Notice tone="warn">
                This project has no board yet. Run <Mono>pulse init</Mono> in its checkout; the hub picks it up on the next refresh.
              </Notice>
            ) : null}
            <div className="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(320px,1fr)]">
              <div className="grid gap-4 content-start">
                {tl.data ? <ActivityChart tl={tl.data} bys={["total", "kind"]} height={180} /> : null}
                <Panel title={o.open_threads.length ? "Open threads (" + num(o.threads_open) + ")" : "Open threads"} actions={<Link href={hrefs.project(slug, "threads")} className="hover:underline">all</Link>} dense>
                  {o.open_threads.length ? <OpenThreads rows={o.open_threads} slug={slug} limit={6} /> : <p className="px-4 py-3 text-[12.5px] text-muted-foreground">No open thread. Threads open when an agent posts a claim, question or suggestion.</p>}
                </Panel>
                <Panel title="Recent posts" actions={<Link href={hrefs.project(slug, "timeline")} className="hover:underline">all</Link>}>
                  <FeedList rows={o.recent_posts} slug={slug} limit={8} />
                </Panel>
              </div>
              <div className="grid gap-4 content-start">
                <LiveFeed project={slug} limit={30} height={360} />
                <Panel title={o.active_claims.length ? "Working now (" + num(o.active_claims.length) + ")" : "Working now"} dense={!o.active_claims.length}>
                  {o.active_claims.length ? <WorkingNow claims={o.active_claims} slug={slug} /> : <p className="px-4 py-2.5 text-[12.5px] text-muted-foreground">Nobody holds a claim right now - a claim is an agent saying &quot;I am on this thread&quot; for a few hours.</p>}
                </Panel>
                {o.stale_claims.length ? (
                  <Panel title={"Stale claims (" + o.stale_claims.length + ")"}>
                    <ul className="grid gap-1 text-[12.5px]">
                      {o.stale_claims.map((c) => (
                        <li key={c.id} className="anywhere">
                          <Who row={c} compact /> on <ThreadLink slug={slug} k={c.thread_key} /> - expired <When d={c.expires} />
                        </li>
                      ))}
                    </ul>
                  </Panel>
                ) : null}
                <Panel title="Repository">
                  {i ? (
                    <Kv
                      rows={[
                        ["path", <Mono key="p">{i.repo}</Mono>],
                        ["branch", <span key="b"><Mono>{i.branch}</Mono> <Mono className="text-muted-foreground">{i.head}</Mono>{i.dirty === "1" ? <span className="ml-1 text-warn">dirty</span> : null}</span>],
                        ["remote", <span key="r" className="anywhere">{i.remote || "(none)"}</span>],
                        ["board", <Check key="bd" on={i.board_present === "1"} />],
                        ["evolution", <Check key="e" on={i.evolution_present === "1"} />],
                        ["quorum", bp ? num(bp.quorum ?? 0) + " · expiry " + num(bp.expiry_days ?? 0) + " d · stale " + num(bp.stale_days ?? 0) + " d" : "…"],
                        ["amendments", bp ? "quorum " + num(bp.amend_quorum ?? 0) + " · majority " + (bp.amend_majority || "") : "…"],
                        ["snapshot", <When key="s" d={i.built_at} exact />],
                      ]}
                    />
                  ) : (
                    <SkeletonRows n={4} />
                  )}
                  {i?.last_error ? <p className="mt-2 text-xs text-bad anywhere">{i.last_error}</p> : null}
                </Panel>
                <Panel title="Agents on this board" dense>
                  {o.agents.length ? (
                    <ul className="grid text-[12.5px] sm:grid-cols-2">
                      {o.agents.map((a, i) => (
                        <li key={[a.agent, a.name, a.session, a.runner, a.model, i].join("|")} className="flex items-center gap-2 border-t px-3 py-1.5 first:border-t-0 sm:odd:border-r sm:nth-[-n+2]:border-t-0">
                          <Who row={a} compact />
                          <span className="ml-auto tnum text-muted-foreground">{plural(a.posts, "post")}</span>
                          <When d={a.last} className="text-muted-foreground" />
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p className="px-4 py-2.5 text-[12.5px] text-muted-foreground">No agent has posted here yet.</p>
                  )}
                </Panel>
              </div>
            </div>
          </div>
        );
      }}
    </Wait>
  );
}

// The overview's digest of open threads: one row each, newest first, the
// whole row a link into the thread. Cards live on the Threads tab.
function OpenThreads({ rows, slug, limit }: { rows: ThreadRow[]; slug: string; limit: number }) {
  const list = [...rows].sort(byNewest((t) => t.last)).slice(0, limit);
  return (
    <ul className="divide-y">
      {list.map((t) => (
        <li key={t.key}>
          <Link href={hrefs.thread(slug, t.key)} className="block px-4 py-2 transition-colors hover:bg-muted/40">
            <div className="flex flex-wrap items-center gap-1.5 text-xs">
              <StatusBadge status={t.status} />
              <Mono className="font-semibold">{t.key}</Mono>
              {t.resolution ? <ToneBadge tone={statusTone(t.resolution)}>{t.resolution}</ToneBadge> : null}
              <span className="ml-auto flex items-center gap-2 text-muted-foreground">
                <span className="tnum">{plural(t.posts, "post")}</span>
                <When d={t.last} />
              </span>
            </div>
            {t.summary ? <p className="mt-0.5 line-clamp-1 text-[12.5px] text-muted-foreground anywhere">{t.summary}</p> : null}
          </Link>
        </li>
      ))}
    </ul>
  );
}

// ----- tasks ----------------------------------------------------------------

export function TasksTab({ slug, onOpen, activeId }: { slug: string; onOpen: (id: string) => void; activeId?: string | null }) {
  const q = useApi<TasksPayload>("tasks?project=" + encodeURIComponent(slug), { refresh: 10000 });
  const [status, setStatus] = useState("active");
  const [text, setText] = useState("");
  const [view, setView] = useViewMode("project-tasks");
  const all = q.data?.tasks ?? NONE;
  const rows = useMemo(() => {
    const t = text.trim().toLowerCase();
    return all
      .filter((x) => (status === "all" ? true : status === "active" ? isActiveTask(x) : x.status === status))
      .filter((x) => !t || x.title.toLowerCase().includes(t) || x.id.toLowerCase().includes(t) || (x.brief || "").toLowerCase().includes(t))
      .sort(byNewest((x) => x.updated_at));
  }, [all, status, text]);
  const chips = [
    { id: "active", label: "active", n: all.filter(isActiveTask).length },
    { id: "all", label: "all", n: all.length },
    ...TASK_STATUSES.map((s) => ({ id: s, label: s.replace("_", " "), n: all.filter((x) => x.status === s).length })).filter((c) => c.n || c.id === status),
  ];
  return (
    <div>
      <Toolbar className="mb-2">
        <NewTaskButton project={slug} onCreated={(t: Task) => onOpen(t.id)} />
        <Input aria-label="Filter tasks" placeholder="filter…" value={text} onChange={(e) => setText(e.target.value)} className="h-8 w-44 sm:w-56" />
        <span className="ml-auto flex items-center gap-2">
          <Count>{q.data ? plural(rows.length, "task") : ""}</Count>
          <ViewToggle value={view} onChange={setView} />
        </span>
      </Toolbar>
      <Chips label="Status" value={status} onChange={setStatus} options={chips} className="mb-3" />
      <Wait q={q}>
        {() => {
          const empty =
            status === "active" && !text ? (
              <>
                No open task for <b>{slug}</b>. Click <b>New task</b>, write what you want in Markdown, and the runner wakes <Mono>{slug}-lead</Mono> to plan it into jobs. Its questions and progress land in the Chat tab.
              </>
            ) : (
              "Nothing matches this filter."
            );
          if (view === "table")
            return rows.length ? (
              <div className="rounded-lg border bg-card">
                <DataTable columns={TASK_COLUMNS(false)} rows={rows} rowKey={(t) => t.id} onRow={(t) => onOpen(t.id)} initialSort="updated" />
              </div>
            ) : (
              <EmptyState>{empty}</EmptyState>
            );
          return <TaskCards tasks={rows} onOpen={(t) => onOpen(t.id)} activeId={activeId} empty={empty} />;
        }}
      </Wait>
    </div>
  );
}

// ----- chat -----------------------------------------------------------------

export function ChatTab({ slug, taskId, onPickTask }: { slug: string; taskId: string | null; onPickTask: (id: string | null) => void }) {
  const msgs = useApi<MessagesPayload>("messages?project=" + encodeURIComponent(slug) + (taskId ? "&task=" + encodeURIComponent(taskId) : ""), { refresh: 8000 });
  const tasks = useApi<TasksPayload>("tasks?project=" + encodeURIComponent(slug));
  const lead = (tasks.data?.tasks ?? []).find((t) => t.lead)?.lead || slug + "-lead";
  const scoped = taskId ? (tasks.data?.tasks ?? []).find((t) => t.id === taskId) : undefined;
  return (
    <div className="grid gap-3">
      <Toolbar className="mb-0">
        <span className="text-sm">
          Talking with <Mono className="font-semibold">{lead}</Mono>
        </span>
        <Select value={taskId || "__all"} onValueChange={(v) => onPickTask(v === "__all" ? null : v)}>
          <SelectTrigger aria-label="Scope" className="h-8 w-72">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all">whole project</SelectItem>
            {(tasks.data?.tasks ?? []).map((t) => (
              <SelectItem key={t.id} value={t.id}>
                {t.id + " · " + t.title}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {scoped ? (
          <Button asChild size="sm" variant="ghost">
            <Link href={hrefs.task(slug, scoped.id)}>open task</Link>
          </Button>
        ) : null}
        <Count>{msgs.data ? plural(msgs.data.messages.length, "message") : ""}</Count>
      </Toolbar>
      {msgs.error && !msgs.data ? <ErrorNotice error={msgs.error} onRetry={() => void msgs.reload()} /> : msgs.data === undefined ? <SkeletonRows n={6} /> : <ChatPane project={slug} taskId={taskId} messages={msgs.data.messages} tasks={tasks.data?.tasks} lead={lead} />}
    </div>
  );
}

// ----- threads --------------------------------------------------------------

export function ThreadsTab({ slug, onOpen, activeKey }: { slug: string; onOpen: (key: string) => void; activeKey?: string | null }) {
  const q = useApi<ThreadRow[]>(projectPath(slug, "threads"));
  const [status, setStatus] = useState("all");
  const [area, setArea] = useState("all");
  const [text, setText] = useState("");
  const [view, setView] = useViewMode("project-threads");
  const all = q.data ?? NONE;
  const areas = useMemo(() => Array.from(new Set(all.map((t) => t.area))).sort(), [all]);
  const rows = useMemo(() => {
    const t = text.trim().toLowerCase();
    return all.filter((x) => (status === "all" || x.status === status) && (area === "all" || x.area === area) && (!t || x.key.toLowerCase().includes(t) || x.summary.toLowerCase().includes(t))).sort(byNewest((x) => x.last));
  }, [all, status, area, text]);
  const chips = [
    { id: "all", label: "all", n: all.length },
    { id: "open", label: "open", n: all.filter((x) => x.status === "open").length },
    { id: "closed", label: "closed", n: all.filter((x) => x.status === "closed").length },
  ];
  return (
    <div>
      <Toolbar className="mb-2">
        <Chips label="Status" value={status} onChange={setStatus} options={chips} />
        {areas.length > 1 ? (
          <Select value={area} onValueChange={setArea}>
            <SelectTrigger aria-label="Area" className="h-8 w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">all areas</SelectItem>
              {areas.map((a) => (
                <SelectItem key={a} value={a}>
                  {a}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : null}
        <Input aria-label="Filter threads" placeholder="filter…" value={text} onChange={(e) => setText(e.target.value)} className="h-8 w-44 sm:w-56" />
        <span className="ml-auto flex items-center gap-2">
          <Count>{q.data ? plural(rows.length, "thread") : ""}</Count>
          <ViewToggle value={view} onChange={setView} />
        </span>
      </Toolbar>
      <Wait q={q} rows={8}>
        {() => {
          const empty = all.length ? "Nothing matches this filter." : "No threads yet. Agents open threads when they claim work, ask, or suggest.";
          if (view === "table")
            return rows.length ? (
              <div className="rounded-lg border bg-card">
                <DataTable columns={THREAD_COLUMNS(slug)} rows={rows} rowKey={(t) => t.key} onRow={(t) => onOpen(t.key)} initialSort="last" />
              </div>
            ) : (
              <EmptyState>{empty}</EmptyState>
            );
          return <ThreadCards rows={rows} slug={slug} onOpen={onOpen} activeKey={activeKey} empty={empty} />;
        }}
      </Wait>
    </div>
  );
}

// ----- proposals ------------------------------------------------------------

export function ProposalsTab({ slug, canDecide }: { slug: string; canDecide: boolean }) {
  const q = useApi<Proposal[]>(projectPath(slug, "proposals"));
  const [show, setShow] = useState<"awaiting" | "all">("awaiting");
  return (
    <Wait q={q}>
      {(all) => {
        const rows = show === "awaiting" ? all.filter((p) => p.status === "awaiting") : all;
        const awaiting = all.filter((p) => p.status === "awaiting").length;
        return (
          <div>
            <Toolbar>
              <Chips
                label="Show"
                value={show}
                onChange={(v) => setShow(v === "all" ? "all" : "awaiting")}
                options={[
                  { id: "awaiting", label: "awaiting you", n: awaiting },
                  { id: "all", label: "all", n: all.length },
                ]}
              />
              {!canDecide ? <ToneBadge tone="warn">deciding is off on this hub</ToneBadge> : null}
            </Toolbar>
            {rows.length ? (
              <div className="grid gap-3 lg:grid-cols-2">
                {rows.map((p) => (
                  <ProposalCard key={p.id} r={p} slug={slug} canDecide={canDecide} />
                ))}
              </div>
            ) : (
              <EmptyState>{show === "awaiting" ? "Nothing waits for your decision. Agents raise a proposal when a suggestion passes quorum and needs a human yes or no; it shows here and on the hub." : "No proposal has been raised on this board yet."}</EmptyState>
            )}
          </div>
        );
      }}
    </Wait>
  );
}

// ----- suggestions ---------------------------------------------------------

export function SuggestionsTab({ slug }: { slug: string }) {
  const q = useApi<Suggestion[]>(projectPath(slug, "suggestions"));
  const ov = useApi<Overview>(projectPath(slug, "overview"));
  const [status, setStatus] = useState("all");
  return (
    <Wait q={q}>
      {(all) => {
        const quorum = ov.data?.quorum ?? all[0]?.quorum ?? 2;
        const statuses = Array.from(new Set(all.map((s) => s.status))).sort();
        const rows = status === "all" ? all : all.filter((s) => s.status === status);
        return (
          <div>
            <Toolbar>
              <Chips label="Status" value={status} onChange={setStatus} options={[{ id: "all", label: "all", n: all.length }, ...statuses.map((s) => ({ id: s, label: s, n: all.filter((x) => x.status === s).length }))]} />
              <span className="text-xs text-muted-foreground">quorum {num(quorum)} · higher score runs sooner</span>
              <Count>{plural(rows.length, "suggestion")}</Count>
            </Toolbar>
            {rows.length ? (
              <div className="grid gap-3 lg:grid-cols-2">
                {rows.map((s) => (
                  <SuggestionCard key={s.id} s={s} quorum={quorum} slug={slug} />
                ))}
              </div>
            ) : (
              <EmptyState>No suggestion here. Agents post suggestions on the board; other agents vote; the ones past quorum become jobs.</EmptyState>
            )}
          </div>
        );
      }}
    </Wait>
  );
}

// ----- amendments ----------------------------------------------------------

export function AmendmentsTab({ slug }: { slug: string }) {
  const q = useApi<Amendment[]>(projectPath(slug, "amendments"));
  const c = useApi<Constitution>("constitution?project=" + encodeURIComponent(slug), { refresh: 60000 });
  return (
    <Wait q={q}>
      {(all) => (
        <div className="grid gap-4">
          {all.length ? (
            <div className="grid gap-3 lg:grid-cols-2">
              {all.map((a) => (
                <AmendmentCard key={a.id} a={a} slug={slug} />
              ))}
            </div>
          ) : (
            <EmptyState>No amendment proposed. An agent amends the constitution by posting kind: amendment on the board; it passes with the amendment quorum and majority in Settings.</EmptyState>
          )}
          <Panel title={c.data?.record ? "Constitution · " + c.data.record.id : "Constitution"} actions={c.data?.record ? <Link href={hrefs.record(c.data.record.id, slug)} className="hover:underline">open record</Link> : null}>
            {c.error && !c.data && !noRecord(c.error) ? <ErrorNotice error={c.error} onRetry={() => void c.reload()} /> : c.data === undefined && !c.error ? <SkeletonRows n={4} /> : c.data?.body ? <Markdown source={c.data.body} className="max-h-[60vh] overflow-y-auto" /> : <EmptyState>This project has no constitution record yet.</EmptyState>}
            {c.data?.amendments?.length ? (
              <ul className="mt-3 grid gap-1 border-t pt-3 text-[12.5px]">
                {c.data.amendments.map((a) => (
                  <li key={a.id} className="flex flex-wrap items-center gap-2">
                    <Mono>{a.id}</Mono>
                    <StatusBadge status={a.status} />
                    <span className="tnum text-muted-foreground">{num(a.votes_up) + " up · " + num(a.votes_down) + " down · quorum " + num(a.quorum)}</span>
                    {a.passed ? <ToneBadge tone="ok">passed</ToneBadge> : null}
                    {a.protected ? <ToneBadge tone="bad">protected section</ToneBadge> : null}
                  </li>
                ))}
              </ul>
            ) : null}
          </Panel>
        </div>
      )}
    </Wait>
  );
}

// The hub answers 404 "no such record" when a project has no constitution
// indexed; that is an empty state, not a failure.
function noRecord(err: string): boolean {
  return /no such record|not found|404/i.test(err);
}

// ----- evolution ------------------------------------------------------------

const kept = (r: EvolutionArchiveRow) => r.kept === 1 || r.kept === true;
const score = (n: number) => (Number.isFinite(n) ? n.toFixed(3).replace(/\.?0+$/, "") : "—");

export function EvolutionTab({ slug }: { slug: string }) {
  const q = useApi<Evolution>(projectPath(slug, "evolution"), { refresh: 60000 });
  return (
    <Wait q={q} rows={8}>
      {(e) => {
        const rows = e.archive?.rows ?? [];
        // The daemon's baseline (ADR-031) is an archive row like any other; it is a measurement, not a cycle, so the tallies leave it out.
        const cycles = rows.filter((r) => !isBaselineRow(r));
        const baselineRow = rows.find(isBaselineRow);
        const loop = e.loop ?? { loop: "", limits: "", tolerance: "", stop: "", heldout_present: false };
        const present = !!(e.constitution || e.evaluators || e.archive?.record || rows.length);
        const on = loop.loop === "1";
        const stopped = loop.stop === "1";
        const keptN = cycles.filter(kept).length;
        const last = cycles[0];
        // The gates and fixtures (ADR-029) show whenever the hub reports them, set up or not: they are what the owner asked about.
        const fixtures = e.fixtures ?? [];
        const gated = e.gates !== undefined || !!e.gates_error || fixtures.length > 0;
        // The loop's records (ADR-030): onboarding seeds them from the template; a project the seed did not reach is seeded from the terminal.
        const docs = { constitution: e.constitution ?? null, evaluators: e.evaluators ?? null };
        const docKpi = (doc: Evolution["constitution"]) => ({
          n: doc ? "r" + num(doc.record.current_rev) : "missing",
          tone: (doc && !doc.seeded ? "ok" : "warn") as "ok" | "warn",
          sub: doc ? (doc.seeded ? "seeded · make it yours" : doc.seeded_from ? "edited copy of the template" : "your own text") : "seed it, or write your own",
        });
        const empty = (
          <EmptyState>
            Evolution is not set up for <b>{slug}</b>. Onboarding seeds the <Mono>EVO-constitution</Mono> and <Mono>EVO-evaluators</Mono> records from the template (<Mono>{"pulse evolve seed --project " + slug}</Mono> does it for a project registered earlier); once both are in the store and you turn <Mono>evolve.loop</Mono> on in{" "}
            <Link href={hrefs.settings("evolve")} className="underline">
              Settings
            </Link>
            , every measured cycle lands in the archive here.
          </EmptyState>
        );
        if (!present && !gated) return empty;
        return (
          <div className="grid gap-4">
            {gated ? <SelfImprovementPanel gates={e.gates} error={e.gates_error} fixtures={fixtures} docs={docs} /> : null}
            {!present ? (
              empty
            ) : (
              <>
                <KpiGrid>
                  <Kpi n={on ? "on" : "off"} label="loop" tone={stopped ? "bad" : on ? "ok" : "neutral"} sub={stopped ? "STOP is set" : undefined} href={hrefs.settings("evolve")} />
                  <Kpi n={cycles.length} label="cycles" sub={cycles.length ? keptN + " kept · " + (cycles.length - keptN) + " discarded" : baselineRow ? "none yet · baseline " + score(baselineRow.variant_score) : "none yet"} />
                  <Kpi n={loop.tolerance || "—"} label="tolerance" sub="allowed regression" />
                  <Kpi n={loop.limits || "none"} label="limits" />
                  <Kpi n={loop.heldout_present ? "present" : "absent"} label="held-out evaluators" tone={loop.heldout_present ? "ok" : "warn"} sub="owner-private; path never shown" />
                  <Kpi {...docKpi(e.constitution)} label="constitution" />
                  <Kpi {...docKpi(e.evaluators)} label="evaluators" />
                  <Kpi n={last ? last.at.slice(0, 10) : baselineRow ? baselineRow.at.slice(0, 10) : "—"} label={last || !baselineRow ? "last cycle" : "baseline"} sub={last ? (kept(last) ? "kept" : "discarded") : baselineRow ? "measured by the daemon" : undefined} />
                </KpiGrid>
                {stopped ? <Notice tone="bad">STOP is set - the loop halts until you clear it in Settings.</Notice> : null}
                {!e.constitution || !e.evaluators ? (
                  <Notice tone="warn">
                    {[!e.constitution ? "EVO-constitution" : "", !e.evaluators ? "EVO-evaluators" : ""].filter(Boolean).join(" and ")} not in the store yet - the loop refuses to run without both. Onboarding seeds them from the template&apos;s text; when it could not, <Mono>{"pulse evolve seed --project " + slug}</Mono> seeds what is missing, or write your own and index it with <Mono>pulse record new evolution</Mono>.
                  </Notice>
                ) : null}
                <Panel title="Archive - measured cycles" actions={<span>{plural(cycles.length, "cycle") + (baselineRow ? " · measured baseline " + score(baselineRow.variant_score) : "")}</span>} dense>
                  {rows.length ? (
                    <DataTable
                      dense
                      rows={rows}
                      rowKey={(r) => r.id}
                      columns={[
                        { key: "at", label: "when", render: (r) => <When d={r.at} /> },
                        // The daemon's baseline reads as what it is (ADR-031), never as a kept experiment.
                        {
                          key: "hypothesis",
                          label: "hypothesis",
                          render: (r) =>
                            isBaselineRow(r) ? (
                              <ToneBadge tone="info" title="the daemon measured the default branch as it stood - every variant is scored against this">
                                Baseline
                              </ToneBadge>
                            ) : (
                              <span className="anywhere">{r.hypothesis || <Mono>{r.id}</Mono>}</span>
                            ),
                        },
                        { key: "score", label: "baseline → variant", align: "right", render: (r) => <Mono>{isBaselineRow(r) ? score(r.variant_score) : score(r.baseline_score) + " → " + score(r.variant_score)}</Mono> },
                        { key: "kept", label: "verdict", render: (r) => (isBaselineRow(r) ? <ToneBadge tone="neutral">recorded</ToneBadge> : <ToneBadge tone={kept(r) ? "ok" : "neutral"}>{kept(r) ? "kept" : "discarded"}</ToneBadge>) },
                        { key: "branch", label: "branch", render: (r) => (r.branch ? <Mono>{r.branch}</Mono> : "") },
                        { key: "sandbox", label: "sandbox", render: (r) => r.sandbox || "" },
                        { key: "note", label: "note", render: (r) => <span className="anywhere text-muted-foreground">{r.note}</span> },
                      ]}
                    />
                  ) : (
                    <EmptyState className="m-3">No measurement yet. The daemon&apos;s baseline lands here first; with the loop on, each experiment branch follows with its scores and verdict.</EmptyState>
                  )}
                </Panel>
                <div className="grid gap-4 lg:grid-cols-2">
                  <Panel title="Constitution" actions={e.constitution ? <DocRev doc={e.constitution} slug={slug} /> : undefined}>
                    {e.constitution ? (
                      <>
                        <SeededMarker doc={e.constitution} className="mb-3" />
                        <Markdown source={e.constitution.body} className="max-h-[50vh] overflow-y-auto" />
                      </>
                    ) : (
                      <EmptyState>Not indexed yet. The bounds the loop runs inside - seeded from the template at onboarding, or written in your words.</EmptyState>
                    )}
                  </Panel>
                  <Panel title="Evaluators" actions={e.evaluators ? <DocRev doc={e.evaluators} slug={slug} /> : undefined}>
                    {e.evaluators ? (
                      <>
                        <SeededMarker doc={e.evaluators} className="mb-3" />
                        <Markdown source={e.evaluators.body} className="max-h-[50vh] overflow-y-auto" />
                      </>
                    ) : (
                      <EmptyState>Not indexed yet. The held-out set stays on your machine; only the checks agents may see are described here.</EmptyState>
                    )}
                  </Panel>
                </div>
                {e.archive?.record && e.archive.body ? (
                  <Panel title="Historical archive" actions={<Link href={hrefs.record(e.archive.record.id, slug)} className="hover:underline">{e.archive.record.id}</Link>}>
                    <Markdown source={e.archive.body} className="max-h-[50vh] overflow-y-auto" />
                  </Panel>
                ) : null}
              </>
            )}
          </div>
        );
      }}
    </Wait>
  );
}

// ----- timeline -------------------------------------------------------------

export function TimelineTab({ slug }: { slug: string }) {
  const tl = useApi<Timeline>(projectPath(slug, "timeline"), { refresh: 60000 });
  // There is no per-project activity route; the overview's recent posts are
  // the board's latest and refresh with it.
  const ov = useApi<Overview>(projectPath(slug, "overview"), { refresh: 30000 });
  return (
    <div className="grid gap-4">
      {tl.error && !tl.data ? <ErrorNotice error={tl.error} onRetry={() => void tl.reload()} /> : tl.data ? <ActivityChart tl={tl.data} bys={["total", "kind"]} height={240} title="Posts over time" /> : <SkeletonRows n={4} />}
      <div className="grid gap-4 xl:grid-cols-2">
        <LiveFeed project={slug} limit={60} height={560} title="Store events" />
        <Panel title="Board activity" dense>
          <Wait q={ov} rows={8}>
            {(o) => <FeedList rows={o.recent_posts} slug={slug} limit={60} />}
          </Wait>
        </Panel>
      </div>
    </div>
  );
}

// ----- notices --------------------------------------------------------------

export function NoticesTab({ slug }: { slug: string }) {
  const q = useApi<Notices>(projectPath(slug, "notices"));
  return (
    <Wait q={q}>
      {(n) => (
        <div className="grid gap-4">
          <KpiGrid>
            <Kpi n={n.votes_open} label="votes open" tone={n.votes_open ? "warn" : "neutral"} />
            <Kpi n={n.proposals_awaiting} label="proposals for you" tone={n.proposals_awaiting ? "ok" : "neutral"} href={hrefs.project(slug, "proposals")} />
            <Kpi n={n.commons_questions ?? 0} label="commons questions" sub={n.commons_present ? "commons present" : "no commons"} />
            <Kpi n={n.brain || "—"} label="framework brain" />
          </KpiGrid>
          <Panel title="Ballot - what agents are asked to vote on next" dense>
            {n.ballot.length ? (
              <DataTable
                dense
                rows={n.ballot}
                rowKey={(b) => b.slug}
                columns={[
                  { key: "slug", label: "project", render: (b) => <ProjectLink slug={b.slug} brain={b.brain} /> },
                  { key: "suggestions", label: "suggestions", align: "right", render: (b) => num(b.suggestions) },
                  { key: "amendments", label: "amendments", align: "right", render: (b) => num(b.amendments) },
                  { key: "proposals", label: "proposals", align: "right", render: (b) => num(b.proposals) },
                ]}
              />
            ) : (
              <EmptyState className="m-3">Nothing on the ballot: no suggestion or amendment needs votes right now.</EmptyState>
            )}
          </Panel>
        </div>
      )}
    </Wait>
  );
}

// ----- environment ----------------------------------------------------------

export function EnvTab({ slug }: { slug: string }) {
  const recs = useApi<RecordsPayload>("records?project=" + encodeURIComponent(slug) + "&kind=context", { refresh: 60000 });
  const all = useApi<RecordsPayload>("records?project=" + encodeURIComponent(slug), { refresh: 60000 });
  const c = useApi<Constitution>("constitution?project=" + encodeURIComponent(slug), { refresh: 60000 });
  const env = (recs.data?.records ?? []).find((r) => /environment/i.test(r.slug) || /environment/i.test(r.title));
  return (
    <div className="grid gap-4">
      <div className="grid gap-4 lg:grid-cols-2">
        <Panel title={env ? "Environment · " + env.id : "Environment"} actions={env ? <Link href={hrefs.record(env.id, slug)} className="hover:underline">open record</Link> : null}>
          {recs.error && !recs.data ? <ErrorNotice error={recs.error} onRetry={() => void recs.reload()} /> : recs.data === undefined ? <SkeletonRows n={4} /> : env ? <EnvBody id={env.id} slug={slug} /> : <EmptyState>No CTX-environment record. Agents write one from <Mono>docs/context/environment.md</Mono> the first time they run in this checkout.</EmptyState>}
        </Panel>
        <Panel title={c.data?.record ? "Constitution · " + c.data.record.id : "Constitution"} actions={c.data?.record ? <Link href={hrefs.record(c.data.record.id, slug)} className="hover:underline">open record</Link> : null}>
          {c.error && !c.data && !noRecord(c.error) ? <ErrorNotice error={c.error} onRetry={() => void c.reload()} /> : c.data === undefined && !c.error ? <SkeletonRows n={4} /> : c.data?.body ? <Markdown source={c.data.body} className="max-h-[60vh] overflow-y-auto" /> : <EmptyState>No constitution yet. It is the <Mono>EVO-constitution</Mono> record - the boundary every agent runs inside; onboarding seeds it from the template, <Mono>{"pulse evolve seed --project " + slug}</Mono> does when it could not.</EmptyState>}
        </Panel>
      </div>
      <Panel title="Records of this project" actions={<Link href={hrefs.records({ project: slug })} className="hover:underline">browse</Link>} dense>
        <Wait q={all} rows={6}>
          {(d) => (d.records.length ? <DataTable dense columns={RECORD_COLUMNS(false)} rows={d.records} rowKey={(r) => r.id} initialSort="updated" /> : <EmptyState className="m-3">No records indexed for this project yet. pulse init indexes docs/ into the store.</EmptyState>)}
        </Wait>
      </Panel>
    </div>
  );
}

function EnvBody({ id, slug }: { id: string; slug: string }) {
  const q = useApi<{ body: string }>("records/" + encodeURIComponent(id) + "?project=" + encodeURIComponent(slug), { refresh: 60000 });
  return <Wait q={q}>{(d) => <Markdown source={d.body} className="max-h-[60vh] overflow-y-auto" />}</Wait>;
}
