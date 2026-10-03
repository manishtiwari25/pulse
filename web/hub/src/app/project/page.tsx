"use client";

import Link from "next/link";
import { Suspense, useCallback, useEffect } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageTitle, EmptyState, SkeletonPage } from "@/components/pulse/panel";
import { CardGrid, ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle, SectionHeading } from "@/components/pulse/item-card";
import { KindBadge, Mono, ToneBadge, When } from "@/components/pulse/who";
import { CommonsLine } from "@/components/pulse/cards";
import { PostSheet, ThreadSheet } from "@/components/board/sheets";
import { TaskSheet } from "@/components/tasks/task-sheet";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { AddProjectButton, RemoveMenuButton, RestoreButton } from "@/components/project/project-dialogs";
import { AmendmentsTab, ChatTab, EnvTab, EvolutionTab, NoticesTab, OverviewTab, ProposalsTab, SuggestionsTab, TasksTab, ThreadsTab, TimelineTab } from "@/components/project/tabs";
import { hrefs, PROJECT_TABS, resolveProjectTab, type ProjectCanonicalTab } from "@/lib/nav";
import { allProjects, projectPath, registeredProjects, type Org, type ProjectCardData, type ProjectGoalPayload, type ProjectsPayload, type RegisteredProject, type TasksPayload } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { useInbox } from "@/components/pulse/inbox";
import { hashHue, initials, num, plural } from "@/lib/format";
import { cn } from "@/lib/utils";
import { ActivityIcon, BookOpenIcon, ClockIcon, InboxIcon, ListTodoIcon, MessagesSquareIcon, SparklesIcon } from "lucide-react";

type BoardSub = "threads" | "suggestions" | "amendments" | "notices";
type HistorySub = "timeline" | "env";

const BOARD_SUBS: { id: BoardSub; label: string }[] = [
  { id: "threads", label: "Threads" },
  { id: "suggestions", label: "Suggestions" },
  { id: "amendments", label: "Amendments" },
  { id: "notices", label: "Notices" },
];

const HISTORY_SUBS: { id: HistorySub; label: string }[] = [
  { id: "timeline", label: "Timeline" },
  { id: "env", label: "Environment" },
];

const boardSub = (v: string | null | undefined): BoardSub => (v === "suggestions" || v === "amendments" || v === "notices" ? v : "threads");
const historySub = (v: string | null | undefined): HistorySub => (v === "env" ? "env" : "timeline");

export default function ProjectPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={4} />}>
      <ProjectScreen />
    </Suspense>
  );
}

function ProjectScreen() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const slug = sp.get("slug") || "";
  const tabParam = sp.get("tab");
  const resolved = resolveProjectTab(tabParam);
  const tab: ProjectCanonicalTab = resolved.tab;
  const subParam = sp.get("sub") || resolved.sub;
  const board = boardSub(subParam);
  const history = historySub(subParam);
  const thread = sp.get("thread");
  const post = sp.get("post");
  const task = sp.get("task");
  const org = useApi<Org>("org");
  const goal = useApi<ProjectGoalPayload>(slug && slug !== "commons" ? projectPath(slug, "goal") : null, { refresh: 60000 });
  const taskSummary = useApi<TasksPayload>(slug ? "tasks?project=" + encodeURIComponent(slug) : null, { refresh: 30000 });
  // The registry (inactive rows included) only matters on the picker.
  const reg = useApi<ProjectsPayload>(!slug ? "projects" : null, { refresh: false });
  // The one "for you" count, shared with the home page and the sidebar.
  const inbox = useInbox();

  const setParams = useCallback(
    (patch: Record<string, string | null | undefined>) => {
      const q = new URLSearchParams(sp.toString());
      for (const [k, v] of Object.entries(patch)) {
        if (v === null || v === undefined || v === "") q.delete(k);
        else q.set(k, v);
      }
      const s = q.toString();
      router.push(pathname + (s ? "?" + s : ""), { scroll: false });
    },
    [sp, router, pathname],
  );

  useEffect(() => {
    if (!slug) return;
    const wanted = tab === "overview" ? null : tab;
    const wantedSub = tab === "board" ? board : tab === "history" ? history : null;
    if ((tabParam || null) === wanted && (sp.get("sub") || null) === wantedSub) return;
    const q = new URLSearchParams(sp.toString());
    if (wanted) q.set("tab", wanted);
    else q.delete("tab");
    if (wantedSub) q.set("sub", wantedSub);
    else q.delete("sub");
    router.replace(pathname + (q.toString() ? "?" + q.toString() : ""), { scroll: false });
  }, [slug, tab, board, history, tabParam, sp, router, pathname]);

  if (!slug) {
    const projects = registeredProjects(org.data);
    const commons = org.data?.commons;
    const inactive = (reg.data?.projects ?? []).filter((p) => p.status !== "active");
    return (
      <>
        <PageTitle
          crumb={
            <>
              <Link href={hrefs.home()} className="hover:underline">
                Home
              </Link>{" "}
              / project
            </>
          }
          sub="Every checkout the hub knows about. Open one to see its board, tasks and the conversation with its lead."
          actions={<AddProjectButton size="sm" onRegistered={(r) => setParams({ slug: r.project.slug })} />}
        >
          Pick a project
        </PageTitle>
        {!org.data && org.loading ? (
          <SkeletonPage rows={2} />
        ) : (
          <>
            {projects.length ? (
              <CardGrid cols={3}>
                {projects.map((p) => (
                  <ProjectPick key={p.slug} p={p} forYou={inbox.forProject(p.slug).total} />
                ))}
              </CardGrid>
            ) : (
              <EmptyState>
                No project registered yet. Use <span className="font-medium text-foreground">Add project</span> above, or run <Mono>pulse init</Mono> in a checkout; the hub lists it here.
              </EmptyState>
            )}
            {inactive.length ? (
              <section aria-label="Inactive projects" className="mt-4">
                <SectionHeading count={inactive.length} action={<span>removed from the hub; the rows and history stay</span>}>
                  Inactive
                </SectionHeading>
                <CardGrid cols={3}>
                  {inactive.map((p) => (
                    <InactivePick key={p.slug} p={p} onRestored={(slug) => setParams({ slug })} />
                  ))}
                </CardGrid>
              </section>
            ) : null}
            {commons ? <CommonsLine c={commons} className="mt-6" /> : null}
          </>
        )}
      </>
    );
  }

  const card = allProjects(org.data).find((p) => p.slug === slug);
  const isCommons = card?.kind === "commons";
  const canDecide = org.data?.can_decide ?? true;
  const forYou = card ? inbox.forProject(card.slug).total : 0;
  const loopLabel = card ? (card.stop_present ? "Loop stopped" : card.loop === "1" ? "Loop running" : "Loop waiting") : "";
  const goalLine = isCommons ? "The shared board across all projects." : card?.brain ? "The PULSE framework itself." : goal.data?.goal?.text || (card?.name && card.name !== card.slug ? card.name : "Agents work toward the goal you set for this project.");
  const openTasks = (taskSummary.data?.tasks ?? []).filter((t) => t.status !== "done" && t.status !== "cancelled").length;
  const counts: Partial<Record<ProjectCanonicalTab, number>> = card
    ? {
        "for-you": forYou,
        work: openTasks,
        board: card.threads_open,
      }
    : {};

  return (
    <Tabs value={tab} onValueChange={(v) => setParams({ tab: v === "overview" ? null : v, sub: null, thread: null, post: null })} className="gap-0">
      <section className="surface-hero glossy ring-soft mb-6 rounded-3xl p-4 shadow-pop sm:p-5" aria-labelledby="project-title">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0 flex-1">
            <div className="mb-2 text-xs text-muted-foreground">
              <Link href={hrefs.home()} className="hover:underline">Home</Link> / project
            </div>
            <div className="flex min-w-0 items-center gap-3">
              <ProjectAvatar slug={isCommons ? "Commons" : slug} />
              <h1 id="project-title" className="min-w-0 truncate text-3xl font-semibold tracking-tight sm:text-4xl" title={isCommons ? "Commons" : slug}>
                {isCommons ? "Commons" : slug}
              </h1>
              <span className="inline-flex flex-wrap items-center gap-1.5">
                {card?.brain ? <Badge variant="secondary" className="rounded-full">Framework</Badge> : null}
                {isCommons ? <Badge variant="outline" className="rounded-full">Shared board</Badge> : null}
                {card?.stop_present ? <ToneBadge tone="bad">Stop</ToneBadge> : null}
                {card && !card.present ? <Badge variant="destructive" className="rounded-full">Missing on disk</Badge> : null}
              </span>
            </div>
            <p className="mt-2 max-w-3xl text-sm leading-relaxed text-muted-foreground anywhere">{card ? goalLine : org.error || "Loading project..."}</p>
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <ToneBadge tone={forYou ? "warn" : "neutral"} className="h-7 px-3" title={num(forYou) + " for you"}>
                <InboxIcon className="size-3.5" aria-hidden /> {num(forYou)} for you
              </ToneBadge>
              {loopLabel ? (
                <ToneBadge tone={card?.stop_present ? "bad" : card?.loop === "1" ? "ok" : "neutral"} className="h-7 px-3">
                  <ActivityIcon className="size-3.5" aria-hidden /> {loopLabel}
                </ToneBadge>
              ) : null}
              <ToneBadge tone="neutral" className="h-7 px-3">
                <ListTodoIcon className="size-3.5" aria-hidden /> {plural(openTasks, "open task")}
              </ToneBadge>
            </div>
          </div>
          {isCommons ? null : (
            <div className="flex items-center gap-2">
              {card?.brain ? null : <RemoveMenuButton kind="project" name={slug} onApplied={() => setParams({ slug: null, tab: null, sub: null, thread: null, post: null, task: null })} />}
              <NewTaskButton project={slug} onCreated={(t) => setParams({ tab: "work", task: t.id })} />
            </div>
          )}
        </div>
        <TabsList className="mt-5 w-full justify-start overflow-x-auto rounded-full p-1 sm:w-fit" aria-label="Project sections">
          {PROJECT_TABS.map((t) => {
            const active = tab === t.id;
            return (
              <TabsTrigger key={t.id} value={t.id} className="h-9 flex-none rounded-full px-3">
                <ProjectTabIcon id={t.id as ProjectCanonicalTab} />
                {t.label}
                {counts[t.id as ProjectCanonicalTab] ? (
                  <span aria-label={num(counts[t.id as ProjectCanonicalTab] || 0) + " " + t.label.toLowerCase()} className={cn("tnum rounded-full px-1.5 text-[10.5px]", active ? "bg-primary/10 text-primary" : "bg-muted text-muted-foreground")}>
                    {counts[t.id as ProjectCanonicalTab]}
                  </span>
                ) : null}
              </TabsTrigger>
            );
          })}
        </TabsList>
      </section>

      <div role="tabpanel" aria-label={tab}>
        {tab === "overview" ? <OverviewTab slug={slug} /> : null}
        {tab === "for-you" ? <ProposalsTab slug={slug} canDecide={canDecide} /> : null}
        {tab === "work" ? <WorkTab slug={slug} task={task} setParams={setParams} /> : null}
        {tab === "board" ? <BoardTab slug={slug} value={board} onValueChange={(next) => setParams({ tab: "board", sub: next, thread: null, post: null })} thread={thread} setParams={setParams} /> : null}
        {tab === "evolution" ? <EvolutionTab slug={slug} canDecide={canDecide} /> : null}
        {tab === "history" ? <HistoryTab slug={slug} value={history} onValueChange={(next) => setParams({ tab: "history", sub: next })} /> : null}
      </div>

      <ThreadSheet slug={slug} threadKey={thread} onClose={() => setParams({ thread: null })} />
      <PostSheet slug={slug} postId={post} onClose={() => setParams({ post: null })} />
      <TaskSheet id={tab === "work" ? null : task} slug={slug} onClose={() => setParams({ task: null })} />
    </Tabs>
  );
}

function ProjectTabIcon({ id }: { id: ProjectCanonicalTab }) {
  const cls = "size-4";
  if (id === "for-you") return <InboxIcon className={cls} aria-hidden />;
  if (id === "work") return <ListTodoIcon className={cls} aria-hidden />;
  if (id === "board") return <MessagesSquareIcon className={cls} aria-hidden />;
  if (id === "evolution") return <SparklesIcon className={cls} aria-hidden />;
  if (id === "history") return <ClockIcon className={cls} aria-hidden />;
  return <BookOpenIcon className={cls} aria-hidden />;
}

function WorkTab({ slug, task, setParams }: { slug: string; task: string | null; setParams: (patch: Record<string, string | null | undefined>) => void }) {
  return (
    <div className="grid gap-4 xl:grid-cols-[minmax(0,0.95fr)_minmax(360px,1.05fr)]">
      <section className="surface-raised glossy rounded-2xl p-3">
        <SectionHeading icon={ListTodoIcon}>Tasks</SectionHeading>
        <TasksTab slug={slug} onOpen={(id) => setParams({ task: id })} activeId={task} />
      </section>
      <section className="surface-raised glossy min-h-[520px] rounded-2xl p-3">
        <SectionHeading icon={MessagesSquareIcon}>Chat</SectionHeading>
        <ChatTab slug={slug} taskId={task} onPickTask={(id) => setParams({ task: id })} />
      </section>
    </div>
  );
}

function BoardTab({ slug, value, onValueChange, thread, setParams }: { slug: string; value: BoardSub; onValueChange: (next: BoardSub) => void; thread: string | null; setParams: (patch: Record<string, string | null | undefined>) => void }) {
  return (
    <Tabs value={value} onValueChange={(next) => onValueChange(boardSub(next))} className="gap-4">
      <TabsList className="rounded-full" aria-label="Board sections">
        {BOARD_SUBS.map((s) => (
          <TabsTrigger key={s.id} value={s.id} className="rounded-full">
            {s.label}
          </TabsTrigger>
        ))}
      </TabsList>
      {value === "threads" ? <ThreadsTab slug={slug} onOpen={(k) => setParams({ thread: k })} activeKey={thread} /> : null}
      {value === "suggestions" ? <SuggestionsTab slug={slug} /> : null}
      {value === "amendments" ? <AmendmentsTab slug={slug} /> : null}
      {value === "notices" ? <NoticesTab slug={slug} /> : null}
    </Tabs>
  );
}

function HistoryTab({ slug, value, onValueChange }: { slug: string; value: HistorySub; onValueChange: (next: HistorySub) => void }) {
  return (
    <Tabs value={value} onValueChange={(next) => onValueChange(historySub(next))} className="gap-4">
      <TabsList className="rounded-full" aria-label="History sections">
        {HISTORY_SUBS.map((s) => (
          <TabsTrigger key={s.id} value={s.id} className="rounded-full">
            {s.label}
          </TabsTrigger>
        ))}
      </TabsList>
      {value === "timeline" ? <TimelineTab slug={slug} /> : null}
      {value === "env" ? <EnvTab slug={slug} /> : null}
    </Tabs>
  );
}

function ProjectAvatar({ slug }: { slug: string }) {
  return (
    <span className="flex size-8 shrink-0 items-center justify-center rounded-lg text-xs font-semibold text-white shadow-card" style={{ background: `oklch(0.62 0.12 ${hashHue(slug)})` }} aria-hidden>
      {initials(slug)}
    </span>
  );
}

// One inactive row: removed from the hub, its history kept, Restore in reach.
function InactivePick({ p, onRestored }: { p: RegisteredProject; onRestored: (slug: string) => void }) {
  return (
    <ItemCard className="opacity-80" ariaLabel={p.slug + " (inactive)"}>
      <ItemCardHeader right={p.last_seen ? <When d={p.last_seen} /> : null}>
        <Badge variant="outline">inactive</Badge>
        {p.brain ? <Badge variant="secondary">framework</Badge> : null}
        {!p.present ? <Badge variant="destructive">missing on disk</Badge> : null}
        {p.default_branch ? <Mono className="text-muted-foreground">{p.default_branch}</Mono> : null}
      </ItemCardHeader>
      <ItemCardTitle>{p.slug}</ItemCardTitle>
      {p.name && p.name !== p.slug ? <ItemCardBody clamp={1}>{p.name}</ItemCardBody> : null}
      <ItemCardMeta>
        <Mono className="text-muted-foreground">{p.path}</Mono>
      </ItemCardMeta>
      <ItemCardActions>
        <RestoreButton slug={p.slug} onRestored={(r) => onRestored(r.project.slug)} />
        <span className="text-xs text-muted-foreground">{p.present ? "registers it again at its stored path" : "the hub refuses while the folder is missing"}</span>
      </ItemCardActions>
    </ItemCard>
  );
}

// One registered checkout on the picker: what it is, what waits in it.
function ProjectPick({ p, forYou }: { p: ProjectCardData; forYou: number }) {
  const attention = forYou > 0;
  return (
    <ItemCard href={hrefs.project(p.slug)} highlight={attention} ariaLabel={p.slug}>
      <ItemCardHeader right={p.last_post ? <When d={p.last_post.date} /> : null}>
        {p.brain ? <Badge variant="secondary">framework</Badge> : null}
        {p.stop_present ? <ToneBadge tone="bad">STOP</ToneBadge> : null}
        {!p.present ? <Badge variant="destructive">missing on disk</Badge> : null}
        {p.branch ? <Mono className="text-muted-foreground">{p.branch}</Mono> : null}
        {p.dirty ? <span className="text-warn">dirty</span> : null}
      </ItemCardHeader>
      <ItemCardTitle chevron>{p.slug}</ItemCardTitle>
      {p.name && p.name !== p.slug ? <ItemCardBody clamp={1}>{p.name}</ItemCardBody> : null}
      <ItemCardMeta>
        <span className={attention ? "font-medium text-ok" : undefined}>{num(forYou) + " for you"}</span>
        <span>{plural(p.claims_active, "claim")}</span>
        <span>{plural(p.threads_open, "open thread")}</span>
        <span>{num(p.posts_24h) + " posts/24h"}</span>
      </ItemCardMeta>
      {p.last_post ? (
        <ItemCardBody clamp={1} className="flex items-center gap-1.5">
          <KindBadge kind={p.last_post.kind} />
          <span className="truncate">{p.last_post.summary}</span>
        </ItemCardBody>
      ) : (
        <ItemCardBody clamp={1}>{p.board_present ? "No posts yet." : "No board yet - pulse init registers this checkout."}</ItemCardBody>
      )}
    </ItemCard>
  );
}
