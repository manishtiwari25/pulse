"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { ActivityIcon, ArrowRightIcon, BellIcon, BotIcon, CheckCircle2Icon, FileDiffIcon, FolderGit2Icon, FolderIcon, MessageSquareIcon, MessagesSquareIcon, RadioIcon, SearchIcon, SettingsIcon, VoteIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { CodeDeliveredKpi } from "@/components/code-delivery/report";
import { ActivityChart } from "@/components/pulse/charts";
import { FeedList, ProposalCard, QuestionCard, WorkingNow, workingRows, workingSummary } from "@/components/pulse/cards";
import { AmendmentCard, SuggestionCard } from "@/components/board/vote-cards";
import { HygienePanel } from "@/components/pulse/hygiene";
import { DataTable } from "@/components/pulse/data-table";
import { DocRev, GateCard, SelfImprovementPanel } from "@/components/pulse/evolve";
import { useInbox } from "@/components/pulse/inbox";
import { ComputeInboxCard } from "@/components/compute/compute";
import { ProposedRecordCard } from "@/components/records/decide-record";
import { DraftCard, RecordDraftPanel } from "@/components/records/draft-panel";
import { LiveFeed } from "@/components/pulse/live-feed";
import { FailedJobsSection } from "@/components/pulse/job-repair-section";
import { Markdown } from "@/components/pulse/markdown";
import { ProjectGoalPanel } from "@/components/project/goal";
import { LeadsPanel } from "@/components/project/leads";
import { ToolkitPanel } from "@/components/project/toolkit";
import { Count, Disclosure, EmptyState, ErrorNotice, Kpi, KpiGrid, Kv, Notice, Panel, SkeletonRows, Toolbar } from "@/components/pulse/panel";
import { useViewMode, ViewToggle } from "@/components/pulse/view-toggle";
import { Avatar, Mono, PostLink, ProjectLink, StatusBadge, ThreadLink, ToneBadge, When, Who } from "@/components/pulse/who";
import { THREAD_COLUMNS, ThreadCards, ThreadParticipants, ThreadSignals, threadTitle } from "@/components/board/sheets";
import { ChatPane } from "@/components/chat/chat-pane";
import { CardGrid, SectionHeading } from "@/components/pulse/item-card";
import { TaskCards } from "@/components/tasks/task-card";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { SuggestedWorkPanel, WorkSuggestionCard } from "@/components/tasks/suggested-work";
import { TASK_COLUMNS } from "@/components/tasks/task-sheet";
import { kindLabel, RECORD_COLUMNS, RecordCards, RecordSheet } from "@/components/records/records";
import { hrefs } from "@/lib/nav";
import { cn } from "@/lib/utils";
import { num, plural, rel, sentence } from "@/lib/format";
import { statusWord } from "@/lib/words";
import { codeDeliveryKpiState } from "@/lib/code-deliveries";
import { codeDeliveriesPath, type CodeDeliveriesPayload, isBaselineRow, projectPath, RECORD_KINDS, TASK_STATUSES, type Amendment, type Compute, type Constitution, type Evolution, type EvolutionArchiveRow, type MessagesPayload, type Notices, type Overview, type OwnerQuestion, type ProjectInfo, type Proposal, type RecordRow, type RecordsPayload, type Suggestion, type Task, type TasksPayload, type ThreadRow, type Timeline } from "@/lib/api";
import { useApi, type Loaded } from "@/lib/use-api";

function Wait<T>({ q, rows = 5, children }: { q: Loaded<T>; rows?: number; children: (d: T) => React.ReactNode }) {
  if (q.error && !q.data) return <ErrorNotice error={q.error} onRetry={() => void q.reload()} />;
  if (q.data === undefined) return <SkeletonRows n={rows} />;
  return <>{children(q.data)}</>;
}

// A one-of filter as the segmented control /tasks uses: every choice is
// visible and one tap away, with its count inside. On a phone the row
// scrolls sideways. `you` marks a count that waits for the owner.
function Segments({ label, value, onChange, options, className }: { label: string; value: string; onChange: (v: string) => void; options: { id: string; label: string; n?: number; you?: boolean }[]; className?: string }) {
  return (
    <Tabs value={value} onValueChange={onChange} className={cn("min-w-0 max-w-full", className)}>
      <div className="scroll-strip min-w-0 max-w-full">
        <TabsList aria-label={label}>
          {options.map((o) => (
            <TabsTrigger key={o.id} value={o.id} className="flex-none px-2.5">
              {o.label}
              {o.n !== undefined ? <span className={cn("tnum min-w-4 rounded-full px-1.5 text-xs font-normal leading-5", o.you && o.n ? "bg-you-soft text-you-ink" : "bg-background/70 text-muted-foreground")}>{num(o.n)}</span> : null}
            </TabsTrigger>
          ))}
        </TabsList>
      </div>
    </Tabs>
  );
}

// A search box with its magnifier, like the one on /tasks.
function Search({ label, value, onChange, className }: { label: string; value: string; onChange: (v: string) => void; className?: string }) {
  return (
    <span className={cn("relative min-w-0", className)}>
      <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden />
      <Input type="search" aria-label={label} placeholder={label} value={value} onChange={(e) => onChange(e.target.value)} className="h-9 pl-8" />
    </span>
  );
}

// A quiet "See all" link for a panel's title row.
function SeeAll({ href, children = "See all" }: { href: string; children?: React.ReactNode }) {
  return (
    <Link href={href} className="inline-flex items-center gap-1 font-medium text-primary underline-offset-2 hover:underline">
      {children}
      <ArrowRightIcon className="size-3.5" aria-hidden />
    </Link>
  );
}

// "Prefer the terminal?" - the CLI way, folded under the plain instruction.
function Terminal({ children }: { children: React.ReactNode }) {
  return (
    <Disclosure summary="Prefer the terminal?" className="mt-2 text-left" contentClassName="mt-1.5 text-[13px] leading-relaxed text-muted-foreground">
      {children}
    </Disclosure>
  );
}

const byNewest = <T,>(d: (x: T) => string) => (a: T, b: T) => (d(a) < d(b) ? 1 : d(a) > d(b) ? -1 : 0);
const isActiveTask = (x: Task) => x.status !== "done" && x.status !== "cancelled";
const NONE: never[] = [];

// ----- overview -----------------------------------------------------------

// A project's home: what waits for you first, then the goal and the work in
// motion; the repository, the agents and the activity sit in a side column.
// Cleanup and the full event log live under History.
export function OverviewTab({ slug, canDecide = true }: { slug: string; canDecide?: boolean }) {
  const ov = useApi<Overview>(projectPath(slug, "overview"));
  const info = useApi<ProjectInfo>(projectPath(slug, "project"), { refresh: 60000 });
  const tasks = useApi<TasksPayload>("tasks?project=" + encodeURIComponent(slug));
  const tl = useApi<Timeline>(projectPath(slug, "timeline"), { refresh: 60000 });
  const compute = useApi<Compute>("compute");
  const inbox = useInbox();
  const [openRecord, setOpenRecord] = useState<RecordRow | null>(null);
  const commons = slug === "commons";
  return (
    <Wait q={ov} rows={8}>
      {(o) => {
        const open = (tasks.data?.tasks ?? []).filter(isActiveTask);
        const mine = inbox.forProject(slug);
        // The first asks, in the inbox's order; the For you tab holds them all.
        const asks = [
          ...mine.decisions.map((r) => <ProposedRecordCard key={"d" + r.id} r={r} onOpen={setOpenRecord} />),
          ...mine.proposals.map((p) => <ProposalCard key={"p" + p.id} r={p} slug={slug} canDecide={canDecide} />),
          ...mine.questions.map((x) => <QuestionCard key={"q" + x.id} q={x} slug={slug} canDecide={canDecide} />),
          ...mine.compute.map((cr) => <ComputeInboxCard key={"c" + cr.id} r={cr} caps={compute.data?.caps} />),
          ...mine.work.map((s) => <WorkSuggestionCard key={"w" + s.id} s={s} slug={slug} />),
          ...mine.drafts.map(({ r }) => <DraftCard key={"r" + r.id} slug={slug} r={r} />),
          ...mine.steps.map((st) => <GateCard key={"s" + st.kind} step={st} />),
        ];
        const working = workingRows(o.active_claims, open, slug);
        return (
          <div className="grid gap-8 xl:grid-cols-[minmax(0,1fr)_360px]">
            <div className="grid min-w-0 content-start gap-8">
              {!o.board_present ? (
                <Notice tone="warn">
                  This project has no board yet, so agents can&apos;t post or pick up work here. Re-add the project from Home, or run <Mono>pulse init</Mono> in its folder.
                </Notice>
              ) : null}
              {asks.length ? (
                <section aria-label="For you">
                  <SectionHeading
                    count={mine.total}
                    note="These wait for your answer. Nothing here moves until you decide."
                    action={
                      asks.length > 2 ? (
                        <Button asChild size="sm" variant="outline">
                          <Link href={hrefs.project(slug, "for-you")}>
                            See all {num(mine.total)} <ArrowRightIcon />
                          </Link>
                        </Button>
                      ) : undefined
                    }
                  >
                    For you
                  </SectionHeading>
                  <div className="grid gap-3">{asks.slice(0, 2)}</div>
                </section>
              ) : null}
              {commons ? null : <ProjectGoalPanel key={"goal-" + slug} slug={slug} />}
              <Panel title="Working now" icon={RadioIcon} actions={working.length ? <span>{workingSummary(working, o.active_claims.length)}</span> : undefined}>
                {working.length ? (
                  <WorkingNow claims={o.active_claims} slug={slug} tasks={open} />
                ) : (
                  <p className="text-sm leading-relaxed text-muted-foreground">Nobody is working on {commons ? "the shared board" : slug} right now. When an agent starts a job or picks up a thread, it shows up here.</p>
                )}
                {o.stale_claims.length ? <StaleClaims claims={o.stale_claims} slug={slug} /> : null}
              </Panel>
              {commons ? null : <LeadsPanel key={"leads-" + slug} slug={slug} canDecide={canDecide} />}
              {commons ? null : <ToolkitPanel key={"toolkit-" + slug} slug={slug} canDecide={canDecide} />}
              <Panel title="Open threads" icon={MessagesSquareIcon} actions={o.open_threads.length ? <SeeAll href={hrefs.project(slug, "board", { sub: "threads" })}>{"See all " + num(o.threads_open)}</SeeAll> : undefined} dense>
                {o.open_threads.length ? (
                  <OpenThreads rows={o.open_threads} slug={slug} limit={5} />
                ) : (
                  <p className="px-4 py-4 text-sm leading-relaxed text-muted-foreground sm:px-5">No open threads. A thread starts when an agent picks up work, asks another agent something or makes a suggestion.</p>
                )}
              </Panel>
              <Panel title="Recent posts" icon={MessageSquareIcon} actions={o.recent_posts.length ? <SeeAll href={hrefs.project(slug, "history", { sub: "timeline" })} /> : undefined}>
                <FeedList rows={o.recent_posts} slug={slug} limit={8} />
              </Panel>
              <FailedJobsSection key={"repairs-" + slug} project={slug} />
            </div>
            <aside className="grid min-w-0 content-start gap-4" aria-label={commons ? "About the shared board" : "About this project"}>
              <RepositoryPanel q={info} commons={commons} />
              <OverviewStats o={o} slug={slug} commons={commons} />
              <AgentsPanel agents={o.agents} />
              {tl.data ? <ActivityChart tl={tl.data} bys={["total"]} height={140} title="Posts per day" /> : null}
            </aside>
            <RecordSheet id={openRecord?.id ?? null} project={openRecord?.project} onClose={() => setOpenRecord(null)} />
          </div>
        );
      }}
    </Wait>
  );
}

// The side column's numbers, each only when it is not zero: a big 0 says
// nothing the owner needs.
function OverviewStats({ o, slug, commons }: { o: Overview; slug: string; commons: boolean }) {
  // The same request the Code delivered tile makes (it shares the cache), read here only to know whether there is anything to show.
  const cd = useApi<CodeDeliveriesPayload>(commons ? null : codeDeliveriesPath({ project: slug, limit: 100 }), { refresh: 30000 });
  const votes = o.suggestions_needing_votes + o.amendments_open;
  const code = commons ? null : codeDeliveryKpiState(cd.data, cd.loading, cd.error).n;
  const showCode = !commons && code !== 0;
  if (!votes && !showCode) return null;
  return (
    <KpiGrid className={cn("mb-0", votes && showCode ? "md:grid-cols-2" : "grid-cols-1 md:grid-cols-1")}>
      {votes ? <Kpi icon={VoteIcon} n={votes} label="Open votes" sub={voteLine(o)} href={hrefs.project(slug, "board", { sub: o.amendments_open && !o.suggestions_needing_votes ? "amendments" : "suggestions" })} /> : null}
      {showCode ? <CodeDeliveredKpi project={slug} /> : null}
    </KpiGrid>
  );
}

function voteLine(o: Overview): string {
  return [o.suggestions_needing_votes ? plural(o.suggestions_needing_votes, "suggestion") : "", o.amendments_open ? plural(o.amendments_open, "rule change") : ""].filter(Boolean).join(", ");
}

// Things an agent picked up and then went quiet on: worth a look, folded.
function StaleClaims({ claims, slug }: { claims: Overview["stale_claims"]; slug: string }) {
  return (
    <Disclosure className="mt-4 border-t pt-3 text-sm" summary={(claims.length === 1 ? "1 thing was" : plural(claims.length, "thing") + " were") + " picked up and then went quiet"}>
      <ul className="grid gap-2">
        {claims.map((c) => (
          <li key={c.id} className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[13px] text-muted-foreground">
            <Who row={c} compact />
            <span>on</span>
            <ThreadLink slug={slug} k={c.thread_key} />
            <span>
              , ran out <When d={c.expires} />
            </span>
          </li>
        ))}
      </ul>
    </Disclosure>
  );
}

// A folder path that wraps only after a slash, never inside a name
// ("/home/owner/src/" then "demo-app"), with the whole path in the tooltip.
function FolderPath({ path }: { path: string }) {
  const parts = path.split("/");
  return (
    <span className="rounded-md bg-muted px-1.5 py-0.5 font-mono text-[12.5px] text-foreground/85" title={path}>
      {parts.map((p, i) => (
        <span key={i}>
          <span className="whitespace-nowrap">{p + (i < parts.length - 1 ? "/" : "")}</span>
          {i < parts.length - 1 ? <wbr /> : null}
        </span>
      ))}
    </span>
  );
}

// The repository in words: where it lives, which branch, how votes work.
function RepositoryPanel({ q, commons }: { q: Loaded<ProjectInfo>; commons: boolean }) {
  const i = q.data;
  const bp = i?.board_params;
  const rows: [React.ReactNode, React.ReactNode][] = i
    ? [
        ...(commons
          ? []
          : ([
              ["Folder", <FolderPath key="p" path={i.repo} />],
              [
                "Branch",
                <span key="b" title={i.head ? "At commit " + i.head : undefined}>
                  {i.branch || "—"}
                  {i.dirty === "1" ? <span className="text-warn">, with uncommitted changes</span> : null}
                </span>,
              ],
              ["Remote", <span key="r" className="anywhere">{i.remote || "None"}</span>],
            ] as [React.ReactNode, React.ReactNode][])),
        ["Board", i.board_present === "1" ? "Set up" : <span key="bd" className="text-warn">Not set up</span>],
        ...(commons ? [] : ([["Self-improvement", i.evolution_present === "1" ? "Set up" : "Not set up"]] as [React.ReactNode, React.ReactNode][])),
        ["Votes needed", bp ? <span key="q" title="How many agents must vote before a suggestion is decided (quorum)">{plural(bp.quorum ?? 0, "vote") + " per suggestion"}</span> : "…"],
        ["Rule changes", bp ? <span key="a" title="Votes and majority an amendment to the constitution needs">{"Need " + plural(bp.amend_quorum ?? 0, "vote") + (bp.amend_majority ? " and a " + bp.amend_majority + " majority" : "")}</span> : "…"],
        ["Time limits", bp ? <span key="t">{"Suggestions close after " + plural(bp.expiry_days ?? 0, "day") + "; threads go quiet after " + plural(bp.stale_days ?? 0, "day")}</span> : "…"],
        ["Last checked", <When key="s" d={i.built_at} />],
      ]
    : [];
  return (
    <Panel title={commons ? "About the shared board" : "Repository"} icon={FolderGit2Icon}>
      {q.error && !i ? <ErrorNotice error={q.error} onRetry={() => void q.reload()} /> : i ? <Kv rows={rows} className="grid-cols-[minmax(6.5rem,auto)_1fr] gap-y-2" /> : <SkeletonRows n={4} />}
      {i?.last_error ? (
        <Notice tone="bad" className="mt-3 text-[13px]">
          {"The last check failed: " + i.last_error}
        </Notice>
      ) : null}
    </Panel>
  );
}

// Who posts on this board: full names that wrap rather than clip, their
// post count and when they were last here.
function AgentsPanel({ agents }: { agents: Overview["agents"] }) {
  const [all, setAll] = useState(false);
  const LIMIT = 8;
  // One row per name: an agent that ran under several runners or models posts as several identities.
  const list = useMemo(() => {
    const by = new Map<string, { name: string; posts: number; last: string; meta: Set<string> }>();
    for (const a of agents) {
      const name = a.agent || a.name || a.session || "An agent";
      const r = by.get(name) ?? { name, posts: 0, last: "", meta: new Set<string>() };
      r.posts += a.posts || 0;
      if ((a.last || "") > r.last) r.last = a.last || "";
      const m = [a.runner, a.model].filter(Boolean).join(", ");
      if (m) r.meta.add(m);
      by.set(name, r);
    }
    return [...by.values()].sort((a, b) => (a.last < b.last ? 1 : a.last > b.last ? -1 : 0));
  }, [agents]);
  const shown = all ? list : list.slice(0, LIMIT);
  return (
    <Panel title="Who posts here" icon={BotIcon} actions={list.length ? <span className="tnum">{num(list.length)}</span> : undefined}>
      {list.length ? (
        <>
          <ul className="grid gap-3">
            {shown.map((a) => {
              const human = a.name === "human" || a.name === "owner";
              const meta = [...a.meta].join("; ");
              return (
                <li key={a.name} className="flex min-w-0 items-start gap-2.5" title={meta ? a.name + " (" + meta + ")" : a.name}>
                  <Avatar seed={a.name} size={28} human={human} />
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-medium leading-snug anywhere">{human ? "You" : a.name}</div>
                    <div className="text-[13px] leading-snug text-muted-foreground">
                      {plural(a.posts, "post")}
                      {a.last ? ", last " + rel(a.last) : ""}
                    </div>
                  </div>
                </li>
              );
            })}
          </ul>
          {list.length > LIMIT ? (
            <button type="button" aria-expanded={all} className="mt-3 text-[13px] font-medium text-primary underline-offset-2 hover:underline" onClick={() => setAll((v) => !v)}>
              {all ? "Show fewer" : "Show all " + plural(list.length, "agent")}
            </button>
          ) : null}
        </>
      ) : (
        <p className="text-sm leading-relaxed text-muted-foreground">No agent has posted here yet. They appear once they start working on this board.</p>
      )}
    </Panel>
  );
}

// The overview's digest of open threads: one row each, newest first, the
// whole row a link into the thread. Cards live on the Board tab.
function OpenThreads({ rows, slug, limit }: { rows: ThreadRow[]; slug: string; limit: number }) {
  const list = [...rows].sort(byNewest((t) => t.last)).slice(0, limit);
  return (
    <ul className="divide-y divide-border/70">
      {list.map((t) => (
        <li key={t.key}>
          <Link href={hrefs.thread(slug, t.key)} title={"Thread " + t.key} className="block px-4 py-3 transition-colors hover:bg-muted/40 focus-visible:bg-muted/40 focus-visible:outline-none sm:px-5">
            <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
              <StatusBadge status={t.status} />
              <span className="min-w-0 flex-1 text-sm font-medium leading-snug text-foreground anywhere">{threadTitle(t)}</span>
              <When d={t.last} className="text-xs text-muted-foreground" />
            </div>
            {t.summary ? <p className="mt-1 line-clamp-1 text-[13px] text-muted-foreground anywhere">{sentence(t.summary)}</p> : null}
            <div className="mt-1.5 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 text-[13px] text-muted-foreground">
              <ThreadParticipants t={t} />
              <span className="tnum">{plural(t.posts, "post")}</span>
              <ThreadSignals t={t} />
            </div>
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
    { id: "active", label: "Open", n: all.filter(isActiveTask).length },
    { id: "all", label: "All", n: all.length },
    ...TASK_STATUSES.map((s) => ({ id: s, label: statusWord(s), n: all.filter((x) => x.status === s).length, you: s === "review" })).filter((c) => c.n || c.id === status),
  ];
  return (
    <div>
      <SuggestedWorkPanel slug={slug} onOpenTask={onOpen} className="mb-6" />
      <Toolbar className="mb-4">
        <Segments label="Status" value={status} onChange={setStatus} options={chips} />
        <span className="flex min-w-0 flex-1 basis-56 items-center gap-2">
          <Search label="Search tasks" value={text} onChange={setText} className="flex-1 lg:max-w-64" />
          <ViewToggle value={view} onChange={setView} className="ml-auto shrink-0" />
        </span>
      </Toolbar>
      <Wait q={q}>
        {() => {
          const empty =
            status === "active" && !text ? (
              <>
                {slug} has no open task. Press <b>New task</b>, say what you want, and the lead agent starts on it. Its questions and progress show up in the chat.
              </>
            ) : (
              "No task matches. Try another word or status."
            );
          if (view === "table")
            return rows.length ? (
              <div className="overflow-x-auto rounded-2xl border bg-card">
                <DataTable columns={TASK_COLUMNS(false)} rows={rows} rowKey={(t) => t.id} onRow={(t) => onOpen(t.id)} initialSort="updated" />
              </div>
            ) : (
              <EmptyState title="No tasks" action={status === "active" && !text ? <NewTaskButton project={slug} onCreated={(t: Task) => onOpen(t.id)} /> : undefined}>{empty}</EmptyState>
            );
          return <TaskCards tasks={rows} onOpen={(t) => onOpen(t.id)} activeId={activeId} empty={empty} emptyTitle={status === "active" && !text ? "No open task" : undefined} emptyAction={status === "active" && !text ? <NewTaskButton project={slug} onCreated={(t: Task) => onOpen(t.id)} /> : undefined} />;
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
        <span className="inline-flex min-w-0 items-center gap-2 text-sm">
          <Avatar seed={lead} size={24} />
          <span className="text-muted-foreground">Talking with</span>
          <span className="font-medium anywhere">{lead}</span>
        </span>
        <Select value={taskId || "__all"} onValueChange={(v) => onPickTask(v === "__all" ? null : v)}>
          <SelectTrigger aria-label="What the chat is about" className="h-9 w-full sm:w-72">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all">Whole project</SelectItem>
            {(tasks.data?.tasks ?? []).map((t) => (
              <SelectItem key={t.id} value={t.id}>
                {t.title + " (" + t.id + ")"}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {scoped ? (
          <Button asChild size="sm" variant="ghost">
            <Link href={hrefs.task(slug, scoped.id)}>Open the task</Link>
          </Button>
        ) : null}
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
    { id: "all", label: "All", n: all.length },
    { id: "open", label: "Open", n: all.filter((x) => x.status === "open").length },
    { id: "closed", label: "Closed", n: all.filter((x) => x.status === "closed").length },
  ];
  return (
    <div>
      <Toolbar className="mb-4">
        <Segments label="Status" value={status} onChange={setStatus} options={chips} />
        {areas.length > 1 ? (
          <Select value={area} onValueChange={setArea}>
            <SelectTrigger aria-label="Area" className="h-9 w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All areas</SelectItem>
              {areas.map((a) => (
                <SelectItem key={a} value={a}>
                  {sentence(a)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : null}
        <Search label="Search threads" value={text} onChange={setText} className="w-full sm:w-56" />
        <span className="ml-auto flex items-center gap-2">
          {text.trim() ? <Count>{plural(rows.length, "thread")}</Count> : null}
          <ViewToggle value={view} onChange={setView} />
        </span>
      </Toolbar>
      <Wait q={q} rows={8}>
        {() => {
          const empty = all.length ? "No thread matches. Try another word, status or area." : "No threads yet. A thread starts when an agent picks up work, asks another agent something or makes a suggestion.";
          if (view === "table")
            return rows.length ? (
              <div className="overflow-x-auto rounded-2xl border bg-card">
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

// The project's inbox: every card the "N for you" count counts - proposed
// decisions, proposals, questions, compute requests, drafts and the gate
// step - each with its yes or no on it. "All" adds the proposals already
// decided, for the record.
export function ProposalsTab({ slug, canDecide }: { slug: string; canDecide: boolean }) {
  const q = useApi<Proposal[]>(projectPath(slug, "proposals"));
  // Questions addressed to the owner and the evolution loop's stop questions wait for a yes or no like a proposal does; an older hub has no such route.
  const qs = useApi<OwnerQuestion[]>(projectPath(slug, "questions"), { refresh: 60000 });
  const compute = useApi<Compute>("compute");
  const inbox = useInbox();
  const [show, setShow] = useState<"awaiting" | "all">("awaiting");
  const [openRecord, setOpenRecord] = useState<RecordRow | null>(null);
  return (
    <Wait q={q}>
      {(all) => {
        const mine = inbox.forProject(slug);
        // The project's own routes are the source for proposals and questions; the shared inbox adds the rest.
        const questions = qs.data ?? mine.questions;
        const awaiting = all.filter((p) => p.status === "awaiting");
        const decided = all.filter((p) => p.status !== "awaiting");
        const waiting = mine.decisions.length + awaiting.length + questions.length + mine.compute.length + mine.work.length + mine.drafts.length + mine.steps.length;
        const rows = show === "awaiting" ? awaiting : all;
        return (
          <div>
            <Toolbar className="mb-4">
              <Segments
                label="Show"
                value={show}
                onChange={(v) => setShow(v === "all" ? "all" : "awaiting")}
                options={[
                  { id: "awaiting", label: "Waiting for you", n: waiting, you: true },
                  { id: "all", label: "All, with answered ones", n: waiting + decided.length },
                ]}
              />
              {!canDecide ? <span className="text-[13px] text-muted-foreground">To answer, open PULSE on the machine that runs it.</span> : null}
            </Toolbar>
            {waiting || rows.length ? (
              <CardGrid cols={2}>
                {mine.decisions.map((r) => (
                  <ProposedRecordCard key={r.id} r={r} onOpen={setOpenRecord} />
                ))}
                {rows.map((p) => (
                  <ProposalCard key={p.id} r={p} slug={slug} canDecide={canDecide} />
                ))}
                {questions.map((x) => (
                  <QuestionCard key={x.id} q={x} slug={slug} canDecide={canDecide} />
                ))}
                {mine.compute.map((cr) => (
                  <ComputeInboxCard key={cr.id} r={cr} caps={compute.data?.caps} />
                ))}
                {mine.work.map((s) => (
                  <WorkSuggestionCard key={s.id} s={s} slug={slug} />
                ))}
                {mine.drafts.map(({ r }) => (
                  <DraftCard key={r.id} slug={slug} r={r} />
                ))}
                {mine.steps.map((st) => (
                  <GateCard key={st.kind} step={st} />
                ))}
              </CardGrid>
            ) : (
              <EmptyState icon={<CheckCircle2Icon className="size-[18px]" />} title={show === "awaiting" ? "Nothing waits for you" : "No proposals yet"}>
                {show === "awaiting" ? "You're all caught up on " + slug + ". When an agent needs your yes or no, its card shows up here." : "No agent has asked for a decision on this board yet. When one does, its card shows up here."}
              </EmptyState>
            )}
            <RecordSheet id={openRecord?.id ?? null} project={openRecord?.project} onClose={() => setOpenRecord(null)} />
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
            <Toolbar className="mb-4">
              <Segments label="Status" value={status} onChange={setStatus} options={[{ id: "all", label: "All", n: all.length }, ...statuses.map((s) => ({ id: s, label: statusWord(s), n: all.filter((x) => x.status === s).length }))]} />
            </Toolbar>
            {rows.length ? (
              <CardGrid cols={2}>
                {rows.map((s) => (
                  <SuggestionCard key={s.id} s={s} quorum={quorum} slug={slug} />
                ))}
              </CardGrid>
            ) : (
              <EmptyState icon={<VoteIcon className="size-[18px]" />} title="No suggestions">
                Agents post ideas here and vote on each other&apos;s. Once a suggestion has enough votes and passes the checks, it becomes a job.
              </EmptyState>
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
        <div className="grid gap-6">
          {all.length ? (
            <CardGrid cols={2}>
              {all.map((a) => (
                <AmendmentCard key={a.id} a={a} slug={slug} />
              ))}
            </CardGrid>
          ) : (
            <EmptyState icon={<FileDiffIcon className="size-[18px]" />} title="No rule changes">
              Nobody has proposed a change to the rules. An agent proposes one on the board; it passes when enough agents vote for it.
            </EmptyState>
          )}
          <ConstitutionPanel c={c} slug={slug} />
        </div>
      )}
    </Wait>
  );
}

// The rules the project's self-improvement follows, with the rule changes made so far.
function ConstitutionPanel({ c, slug }: { c: Loaded<Constitution>; slug: string }) {
  return (
    <Panel title="The rules (constitution)" icon={FileDiffIcon} actions={c.data?.record ? <SeeAll href={hrefs.record(c.data.record.id, slug)}>Open the record</SeeAll> : null}>
      {c.error && !c.data && !noRecord(c.error) ? (
        <ErrorNotice error={c.error} onRetry={() => void c.reload()} />
      ) : c.data === undefined && !c.error ? (
        <SkeletonRows n={4} />
      ) : c.data?.body ? (
        <Markdown source={c.data.body} className="max-h-[60vh] overflow-y-auto" />
      ) : (
        <p className="text-sm leading-relaxed text-muted-foreground">
          {slug} has no constitution yet. It holds the limits every agent works inside; PULSE writes one from its template when the project is set up.
        </p>
      )}
      {c.data?.amendments?.length ? (
        <div className="mt-4 border-t pt-3">
          <div className="mb-2 text-[13px] font-semibold">Rule changes so far</div>
          <ul className="grid gap-2 text-[13px]">
            {c.data.amendments.map((a) => (
              <li key={a.id} className="flex flex-wrap items-center gap-2">
                <PostLink slug={slug} id={a.id} label={"Rule change by " + (a.id.replace(/^\d{8}T\d{6}Z-?/, "") || "an agent")} className="font-medium text-foreground" />
                <StatusBadge status={a.status} />
                <span className="tnum text-muted-foreground">{num(a.votes_up) + " for, " + num(a.votes_down) + " against, " + num(a.quorum) + " needed"}</span>
                {a.passed ? <ToneBadge tone="ok">Passed</ToneBadge> : null}
                {a.protected ? <ToneBadge tone="bad" title="This part of the rules only you can change">Protected section</ToneBadge> : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </Panel>
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

export function EvolutionTab({ slug, canDecide = true }: { slug: string; canDecide?: boolean }) {
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
        const stopped = loop.stop === "1";
        const keptN = cycles.filter(kept).length;
        // The gates and fixtures (ADR-029) show whenever the hub reports them, set up or not: they are what the owner asked about.
        const fixtures = e.fixtures ?? [];
        const gated = e.gates !== undefined || !!e.gates_error || fixtures.length > 0;
        // The loop's records (ADR-030): onboarding seeds them from the template; agents draft the project's own text and the owner adopts it (ADR-046).
        const docs = { constitution: e.constitution ?? null, evaluators: e.evaluators ?? null };
        const docWord = (doc: Evolution["constitution"]) =>
          !doc ? "Missing" : !doc.seeded ? (doc.draft ? "Your own text, with a new draft to review" : doc.seeded_from ? "Your edited copy of the template" : "Your own text") : doc.draft ? "Template text, with a draft to review" : doc.draft_job ? "Template text; an agent is drafting your own" : "Template text";
        const empty = (
          <EmptyState icon={<ActivityIcon className="size-[18px]" />} title="Self-improvement isn't set up">
            Self-improvement lets agents try changes to {slug} on their own and keep only the ones that score better. It needs the project&apos;s rules and scoring rules. PULSE adds both when it sets a project up; for an older project, add them from a terminal. Then turn it on in{" "}
            <Link href={hrefs.settings("evolve")} className="font-medium text-foreground underline underline-offset-2">
              Settings
            </Link>
            .
            <Terminal>
              For a project added before this existed, run <Mono>{"pulse evolve seed --project " + slug}</Mono>.
            </Terminal>
          </EmptyState>
        );
        // The empty state is the whole tab only when there is nothing else: with the setup step showing, that step is the one call to action.
        if (!present && !gated) return empty;
        return (
          <div className="grid gap-6">
            {gated ? <SelfImprovementPanel gates={e.gates} error={e.gates_error} fixtures={fixtures} docs={docs} canDecide={canDecide} /> : null}
            {!present ? null : (
              <>
                {stopped ? <Notice tone="bad">Self-improvement is stopped. It stays stopped until you clear the stop in Settings.</Notice> : null}
                {!e.constitution || !e.evaluators ? (
                  <Notice tone="warn">
                    {"Self-improvement can't run yet: " + [!e.constitution ? "the rules (constitution)" : "", !e.evaluators ? "the scoring rules" : ""].filter(Boolean).join(" and ") + (!e.constitution && !e.evaluators ? " are" : " is") + " missing. " + (gated ? "Add them with the setup step above." : "PULSE adds them when it sets a project up.")}
                    <Terminal>
                      Run <Mono>{"pulse evolve seed --project " + slug}</Mono> to add what is missing, or write your own and add it with <Mono>pulse record new evolution</Mono>.
                    </Terminal>
                  </Notice>
                ) : null}
                <Panel title="Experiments so far" icon={ActivityIcon} actions={<span>{plural(cycles.length, "experiment") + (cycles.length ? ", " + keptN + " kept" : "") + (baselineRow ? ", starting score " + score(baselineRow.variant_score) : "")}</span>} dense>
                  {rows.length ? (
                    <DataTable
                      dense
                      rows={rows}
                      rowKey={(r) => r.id}
                      columns={[
                        { key: "at", label: "When", render: (r) => <When d={r.at} /> },
                        // The daemon's baseline reads as what it is (ADR-031), never as a kept experiment.
                        {
                          key: "hypothesis",
                          label: "What it tried",
                          render: (r) =>
                            isBaselineRow(r) ? (
                              <ToneBadge tone="info" title="The runner measured the project as it stood; every experiment is scored against this">
                                Starting score
                              </ToneBadge>
                            ) : (
                              <span className="anywhere" title={r.id}>{r.hypothesis || "An experiment"}</span>
                            ),
                        },
                        { key: "score", label: "Score", align: "right", render: (r) => <span className="tnum">{isBaselineRow(r) ? score(r.variant_score) : score(r.baseline_score) + " → " + score(r.variant_score)}</span> },
                        { key: "kept", label: "Result", render: (r) => (isBaselineRow(r) ? <ToneBadge tone="neutral">Measured</ToneBadge> : <ToneBadge tone={kept(r) ? "ok" : "neutral"}>{kept(r) ? "Kept" : "Dropped"}</ToneBadge>) },
                        { key: "branch", label: "Branch", render: (r) => (r.branch ? <Mono>{r.branch}</Mono> : "") },
                        { key: "sandbox", label: "Where it ran", render: (r) => r.sandbox || "" },
                        { key: "note", label: "Note", render: (r) => <span className="anywhere text-muted-foreground">{r.note}</span> },
                      ]}
                    />
                  ) : (
                    <EmptyState className="m-4">No experiments yet. The runner first measures a starting score; with self-improvement on, every experiment follows here with its score and whether it was kept.</EmptyState>
                  )}
                </Panel>
                <div className="grid gap-4 lg:grid-cols-2">
                  <Panel title="The rules (constitution)" icon={FileDiffIcon} actions={e.constitution ? <DocRev doc={e.constitution} slug={slug} /> : undefined} footer={<span>{docWord(e.constitution)}</span>}>
                    {e.constitution ? (
                      <>
                        <RecordDraftPanel r={{ id: e.constitution.record.id, rev: e.constitution.record.current_rev, seeded: e.constitution.seeded === true, seeded_from: e.constitution.seeded_from ?? "", draft: e.constitution.draft, draft_job: e.constitution.draft_job }} body={e.constitution.draft?.body} className="mb-3" />
                        <Markdown source={e.constitution.body} className="max-h-[50vh] overflow-y-auto" />
                      </>
                    ) : (
                      <p className="text-sm leading-relaxed text-muted-foreground">Not written yet. These are the limits experiments must stay inside: from the template, or in your own words.</p>
                    )}
                  </Panel>
                  <Panel title="Scoring rules" icon={CheckCircle2Icon} actions={e.evaluators ? <DocRev doc={e.evaluators} slug={slug} /> : undefined} footer={<span>{docWord(e.evaluators)}</span>}>
                    {e.evaluators ? (
                      <>
                        <RecordDraftPanel r={{ id: e.evaluators.record.id, rev: e.evaluators.record.current_rev, seeded: e.evaluators.seeded === true, seeded_from: e.evaluators.seeded_from ?? "", draft: e.evaluators.draft, draft_job: e.evaluators.draft_job }} body={e.evaluators.draft?.body} className="mb-3" />
                        <Markdown source={e.evaluators.body} className="max-h-[50vh] overflow-y-auto" />
                      </>
                    ) : (
                      <p className="text-sm leading-relaxed text-muted-foreground">Not written yet. It describes the checks agents may see; the private tests stay on your machine.</p>
                    )}
                  </Panel>
                </div>
                <Disclosure variant="row" summary="Limits and older history" className="rounded-2xl border bg-card px-3 py-1.5 text-sm sm:px-4" contentClassName="pb-3">
                  <Kv
                    rows={[
                      ["Allowed drop in score", loop.tolerance || "—"],
                      ["Limits", loop.limits || "None"],
                    ]}
                  />
                  {e.archive?.record && e.archive.body ? (
                    <div className="mt-4 border-t pt-3">
                      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
                        <span className="text-[13px] font-semibold">Older history</span>
                        <SeeAll href={hrefs.record(e.archive.record.id, slug)}>Open the record</SeeAll>
                      </div>
                      <Markdown source={e.archive.body} className="max-h-[50vh] overflow-y-auto" />
                    </div>
                  ) : null}
                </Disclosure>
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
      {tl.error && !tl.data ? <ErrorNotice error={tl.error} onRetry={() => void tl.reload()} /> : tl.data ? <ActivityChart tl={tl.data} bys={["total", "kind"]} height={220} title="Posts per day" /> : <SkeletonRows n={4} />}
      <div className="grid gap-4 xl:grid-cols-2">
        <Panel title="Board posts" icon={MessageSquareIcon}>
          <Wait q={ov} rows={8}>
            {(o) => <FeedList rows={o.recent_posts} slug={slug} limit={60} />}
          </Wait>
        </Panel>
        <LiveFeed project={slug} limit={60} height={640} title="Everything that happened" />
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
          <NoticeStats n={n} slug={slug} />
          <Panel title="What agents vote on next" icon={BellIcon} dense>
            {n.ballot.length ? (
              <DataTable
                dense
                rows={n.ballot}
                rowKey={(b) => b.slug}
                columns={[
                  { key: "slug", label: "Project", render: (b) => <ProjectLink slug={b.slug} brain={b.brain} /> },
                  { key: "suggestions", label: "Suggestions", align: "right", render: (b) => num(b.suggestions) },
                  { key: "amendments", label: "Rule changes", align: "right", render: (b) => num(b.amendments) },
                  { key: "proposals", label: "Proposals", align: "right", render: (b) => num(b.proposals) },
                ]}
              />
            ) : (
              <EmptyState className="m-4">Nothing to vote on: no suggestion or rule change needs votes right now.</EmptyState>
            )}
          </Panel>
        </div>
      )}
    </Wait>
  );
}

// The notice numbers that are not zero; when all are, one sentence says so.
function NoticeStats({ n, slug }: { n: Notices; slug: string }) {
  const tiles = [
    n.votes_open ? <Kpi key="v" icon={VoteIcon} n={n.votes_open} label="Open votes" sub="Agents are voting" /> : null,
    n.proposals_awaiting ? <Kpi key="p" icon={MessagesSquareIcon} n={n.proposals_awaiting} label="Proposals for you" sub="Waiting for your answer" href={hrefs.project(slug, "for-you")} tone="you" /> : null,
    n.commons_questions ? <Kpi key="q" icon={MessageSquareIcon} n={n.commons_questions} label="Questions on the shared board" sub="Across all projects" href={hrefs.project("commons")} /> : null,
  ].filter(Boolean);
  if (!tiles.length) return <p className="text-sm text-muted-foreground">Nothing waits right now: no open votes, no proposals for you and no questions on the shared board.</p>;
  return <KpiGrid className={cn("mb-0", tiles.length === 3 ? "md:grid-cols-3" : tiles.length === 2 ? "md:grid-cols-2" : "grid-cols-1 md:grid-cols-1")}>{tiles}</KpiGrid>;
}

// ----- cleanup ----------------------------------------------------------------

// Routine housekeeping (the runner's sweep), kept off the Overview.
export function CleanupTab({ slug }: { slug: string }) {
  return (
    <div className="grid gap-4">
      <p className="max-w-3xl text-sm leading-relaxed text-muted-foreground">PULSE tidies up after agents on its own: old branches, finished worktrees and threads that went quiet. This is what it found and did.</p>
      <HygienePanel project={slug} limit={30} />
    </div>
  );
}

// ----- environment ----------------------------------------------------------

export function EnvTab({ slug }: { slug: string }) {
  const recs = useApi<RecordsPayload>("records?project=" + encodeURIComponent(slug) + "&kind=context", { refresh: 60000 });
  const all = useApi<RecordsPayload>("records?project=" + encodeURIComponent(slug), { refresh: 60000 });
  const c = useApi<Constitution>("constitution?project=" + encodeURIComponent(slug), { refresh: 60000 });
  const env = (recs.data?.records ?? []).find((r) => /environment/i.test(r.slug) || /environment/i.test(r.title));
  return (
    <div className="grid gap-8">
      <div className="grid gap-4 lg:grid-cols-2">
        <Panel title="How to build and run it" icon={SettingsIcon} actions={env ? <SeeAll href={hrefs.record(env.id, slug)}>Open the record</SeeAll> : null}>
          {recs.error && !recs.data ? (
            <ErrorNotice error={recs.error} onRetry={() => void recs.reload()} />
          ) : recs.data === undefined ? (
            <SkeletonRows n={4} />
          ) : env ? (
            <EnvBody id={env.id} slug={slug} />
          ) : (
            <p className="text-sm leading-relaxed text-muted-foreground">
              No notes yet. Agents write them from <Mono>docs/context/environment.md</Mono> the first time they work in this folder.
            </p>
          )}
        </Panel>
        <ConstitutionPanel c={c} slug={slug} />
      </div>
      <ProjectRecords slug={slug} q={all} />
    </div>
  );
}

// The project's shelf of the library: its decisions, plans, work units,
// features, memory and evolution records as cards, the dense table one click
// away, narrowed by kind or text, each opening in place. Every filter the
// library knows lives on the Records page; "browse" leads there pre-filtered.
function ProjectRecords({ slug, q }: { slug: string; q: Loaded<RecordsPayload> }) {
  const [kind, setKind] = useState("all");
  const [text, setText] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [view, setView] = useViewMode("project-records");
  const all = q.data?.records ?? NONE;
  // The kinds this project holds, in the canonical order, unknown ones last.
  const kinds = useMemo(() => {
    const present = new Set<string>(all.map((r) => r.kind));
    const known: string[] = RECORD_KINDS;
    return known.filter((k) => present.has(k)).concat(Array.from(present).filter((k) => !known.includes(k)));
  }, [all]);
  const rows = useMemo(() => {
    const t = text.trim().toLowerCase();
    return all
      .filter((r) => (kind === "all" || r.kind === kind) && (!t || r.id.toLowerCase().includes(t) || r.title.toLowerCase().includes(t) || (r.tags || []).some((x) => x.toLowerCase().includes(t))))
      .sort(byNewest((r) => r.updated_at || r.date));
  }, [all, kind, text]);
  const chips = [{ id: "all", label: "All", n: all.length }, ...kinds.map((k) => ({ id: k, label: kindLabel(k), n: all.filter((r) => r.kind === k).length }))];
  const clear = () => {
    setKind("all");
    setText("");
  };
  return (
    <section aria-label="Records of this project">
      <SectionHeading icon={FolderIcon} count={q.data ? all.length : undefined} note="Decisions, plans and notes the agents keep about this project." action={<SeeAll href={hrefs.records({ project: slug })}>Open in Records</SeeAll>}>
        Records
      </SectionHeading>
      {all.length ? (
        <Toolbar className="mb-4">
          <Segments label="Kind" value={kind} onChange={setKind} options={chips} />
          <Search label="Search records" value={text} onChange={setText} className="w-full sm:w-56" />
          <span className="ml-auto flex items-center gap-2">
            {text.trim() ? <Count>{plural(rows.length, "record")}</Count> : null}
            <ViewToggle value={view} onChange={setView} />
          </span>
        </Toolbar>
      ) : null}
      <Wait q={q} rows={3}>
        {() => {
          if (!all.length)
            return (
              <EmptyState title="No records yet">
                Nothing is saved for {slug} yet. When PULSE sets a project up it reads the decisions, plans and notes in its docs folder; agents add more as they work.
              </EmptyState>
            );
          if (!rows.length)
            return (
              <EmptyState
                title="No matching records"
                action={
                  <Button size="sm" variant="outline" onClick={clear}>
                    Clear filters
                  </Button>
                }
              >
                No record matches. Try another word or kind.
              </EmptyState>
            );
          if (view === "table")
            return (
              <div className="overflow-x-auto rounded-2xl border bg-card">
                <DataTable columns={RECORD_COLUMNS(false)} rows={rows} rowKey={(r) => r.id} onRow={(r) => setOpen(r.id)} initialSort="updated" caption={"Records of " + slug} />
              </div>
            );
          return <RecordCards rows={rows} onOpen={(r) => setOpen(r.id)} activeId={open} cols={3} />;
        }}
      </Wait>
      <RecordSheet id={open} project={slug} onClose={() => setOpen(null)} />
    </section>
  );
}

function EnvBody({ id, slug }: { id: string; slug: string }) {
  const q = useApi<{ body: string }>("records/" + encodeURIComponent(id) + "?project=" + encodeURIComponent(slug), { refresh: 60000 });
  return <Wait q={q}>{(d) => <Markdown source={d.body} className="max-h-[60vh] overflow-y-auto" />}</Wait>;
}
