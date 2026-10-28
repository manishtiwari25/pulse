"use client";

import Link from "next/link";
import { Suspense, useCallback, useEffect, useRef, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { MoreHorizontalIcon, PencilLineIcon, Trash2Icon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PageTitle, EmptyState, SkeletonPage } from "@/components/pulse/panel";
import { CardGrid, ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle, SectionHeading } from "@/components/pulse/item-card";
import { Mono, When } from "@/components/pulse/who";
import { CommonsLine, ProjectCard, workingRows } from "@/components/pulse/cards";
import { PostSheet, ThreadSheet } from "@/components/board/sheets";
import { TaskSheet } from "@/components/tasks/task-sheet";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { AddProjectButton, RemoveDialog, RenameDialog, RestoreButton } from "@/components/project/project-dialogs";
import { AmendmentsTab, ChatTab, CleanupTab, EnvTab, EvolutionTab, NoticesTab, OverviewTab, ProposalsTab, SuggestionsTab, TasksTab, ThreadsTab, TimelineTab } from "@/components/project/tabs";
import { hrefs, PROJECT_TABS, resolveProjectTab, type ProjectCanonicalTab } from "@/lib/nav";
import { allProjects, projectPath, registeredProjects, type Org, type ProjectCardData, type ProjectGoalPayload, type ProjectsPayload, type RegisteredProject, type Task, type TasksPayload } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { useInbox } from "@/components/pulse/inbox";
import { ProjectMascot } from "@/components/pulse/mascots";
import { num, plural } from "@/lib/format";
import { cn } from "@/lib/utils";
import { ArchiveIcon, FolderOpenIcon, UsersIcon } from "lucide-react";

type BoardSub = "threads" | "suggestions" | "amendments" | "notices";
type HistorySub = "timeline" | "env" | "cleanup";

const BOARD_SUBS: { id: BoardSub; label: string; note: string }[] = [
  { id: "threads", label: "Threads", note: "Where agents pick up work, ask each other questions and report what they found." },
  { id: "suggestions", label: "Suggestions", note: "Ideas the agents vote on. A suggestion with enough votes becomes a job." },
  { id: "amendments", label: "Rule changes", note: "Proposed changes to the rules that self-improvement follows (the constitution)." },
  { id: "notices", label: "Notices", note: "What waits for votes across the boards, and what waits for you." },
];

const HISTORY_SUBS: { id: HistorySub; label: string; note: string }[] = [
  { id: "timeline", label: "Timeline", note: "Everything that happened here, newest first." },
  { id: "env", label: "Setup and records", note: "How agents build and run the project, its rules, and the records they keep." },
  { id: "cleanup", label: "Cleanup", note: "" },
];

const boardSub = (v: string | null | undefined): BoardSub => (v === "suggestions" || v === "amendments" || v === "notices" ? v : "threads");
const historySub = (v: string | null | undefined): HistorySub => (v === "env" ? "env" : v === "cleanup" ? "cleanup" : "timeline");

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
  const [removing, setRemoving] = useState(false);
  const [renaming, setRenaming] = useState(false);

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
        <PageTitle icon={FolderOpenIcon} sub="Every project PULSE works on. Open one to give its lead agent a task, answer what waits for you and follow its board." actions={<AddProjectButton onRegistered={(r) => setParams({ slug: r.project.slug })} />}>
          Projects
        </PageTitle>
        {!org.data && org.loading ? (
          <SkeletonPage rows={2} />
        ) : (
          <div className="grid gap-10">
            {projects.length ? (
              <CardGrid cols={3}>
                {projects.map((p) => (
                  <ProjectCard key={p.slug} p={p} gate={inbox.gateOf(p)} forYou={inbox.forProject(p.slug).total} />
                ))}
              </CardGrid>
            ) : (
              <EmptyState title="Add your first project" action={<AddProjectButton size="sm" onRegistered={(r) => setParams({ slug: r.project.slug })} />}>
                Pick the folder of a git checkout on this machine. Its lead agent starts on the tasks you give it.
              </EmptyState>
            )}
            {commons ? <CommonsLine c={commons} /> : null}
            {inactive.length ? (
              <section aria-label="Removed projects">
                <SectionHeading count={inactive.length} note="PULSE no longer works on these. Their posts and history are kept, and you can bring one back.">
                  Removed projects
                </SectionHeading>
                <CardGrid cols={3}>
                  {inactive.map((p) => (
                    <InactivePick key={p.slug} p={p} onRestored={(slug) => setParams({ slug })} />
                  ))}
                </CardGrid>
              </section>
            ) : null}
          </div>
        )}
      </>
    );
  }

  const card = allProjects(org.data).find((p) => p.slug === slug);
  const isCommons = card?.kind === "commons" || slug === "commons";
  const canDecide = org.data?.can_decide ?? true;
  const mine = card ? inbox.forProject(card.slug) : null;
  const forYou = mine?.total ?? 0;
  const openList = (taskSummary.data?.tasks ?? []).filter((t) => t.status !== "done" && t.status !== "cancelled");
  const openTasks = openList.length;
  // The project's own name leads; the slug (its id in every link) stands
  // beside it once the two differ, so a renamed project is still findable.
  const name = (card?.name || "").trim();
  const title = isCommons ? "Shared board" : name && name !== slug ? name : slug;
  const goalText = (card?.goal_preview || goal.data?.goal?.text || "").trim();
  const sub = isCommons
    ? "The shared board is for work that touches more than one project. Agents from every project post, ask and vote here."
    : card?.brain
      ? goalText || "PULSE's own source code. Agents here improve PULSE itself."
      : goalText || (card?.path ? "Software project at " + card.path : card?.name && card.name !== card.slug ? card.name : "A software project PULSE works on.");
  const counts: Partial<Record<ProjectCanonicalTab, number>> = card ? { "for-you": forYou, work: openTasks, board: card.threads_open } : {};
  const removed = () => setParams({ slug: null, tab: null, sub: null, thread: null, post: null, task: null });

  return (
    <Tabs value={tab} onValueChange={(v) => setParams({ tab: v === "overview" ? null : v, sub: null, thread: null, post: null })} className="gap-0">
      <PageTitle
        className="mb-8"
        badge={
          card && !card.present ? (
            <span className="rounded-full bg-bad/10 px-2.5 py-0.5 text-[13px] font-medium text-bad">Missing on disk</span>
          ) : card?.brain ? (
            <span className="rounded-full bg-muted px-2.5 py-0.5 text-[13px] font-medium text-muted-foreground" title="PULSE's own source code">PULSE&apos;s own source</span>
          ) : undefined
        }
        sub={
          card || !org.loading ? (
            <>
              <p className="line-clamp-2" title={goalText && !isCommons ? "Project goal: " + goalText : undefined}>
                {card ? sub : org.error ? "This project didn't load: " + org.error : "PULSE doesn't know a project called " + slug + "."}
              </p>
              {card ? <StatusLine card={card} commons={isCommons} forYou={forYou} openTasks={taskSummary.data ? openList : null} slug={slug} /> : null}
            </>
          ) : (
            "Loading the project…"
          )
        }
        actions={
          isCommons || !card ? null : (
            <>
              {card.brain ? null : (
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="outline" size="icon" aria-label={"More actions for " + slug}>
                      <MoreHorizontalIcon />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem onSelect={() => setRenaming(true)}>
                      <PencilLineIcon /> Rename project…
                    </DropdownMenuItem>
                    <DropdownMenuItem variant="destructive" onSelect={() => setRemoving(true)}>
                      <Trash2Icon /> Remove project…
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              )}
              <NewTaskButton size="default" project={slug} onCreated={(t) => setParams({ tab: "work", task: t.id })} />
            </>
          )
        }
        bar={<SectionTabs tab={tab} counts={counts} />}
      >
        <span className="inline-flex min-w-0 items-center gap-3">
          {isCommons ? (
            <span className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-muted text-muted-foreground" aria-hidden>
              <UsersIcon className="size-5" />
            </span>
          ) : (
            <ProjectAvatar slug={slug} />
          )}
          <span className="min-w-0 anywhere">
            {title}
            {!isCommons && title !== slug ? <span className="ml-2 align-middle font-mono text-[13px] font-normal text-muted-foreground">{slug}</span> : null}
          </span>
        </span>
      </PageTitle>

      <div role="tabpanel" aria-label={PROJECT_TABS.find((t) => t.id === tab)?.label ?? tab}>
        {tab === "overview" ? <OverviewTab slug={slug} canDecide={canDecide} /> : null}
        {tab === "for-you" ? <ProposalsTab slug={slug} canDecide={canDecide} /> : null}
        {tab === "work" ? <WorkTab slug={slug} task={task} setParams={setParams} /> : null}
        {tab === "board" ? <BoardTab slug={slug} value={board} onValueChange={(next) => setParams({ tab: "board", sub: next, thread: null, post: null })} thread={thread} setParams={setParams} /> : null}
        {tab === "evolution" ? <EvolutionTab slug={slug} canDecide={canDecide} /> : null}
        {tab === "history" ? <HistoryTab slug={slug} value={history} onValueChange={(next) => setParams({ tab: "history", sub: next })} /> : null}
      </div>

      <ThreadSheet slug={slug} threadKey={thread} onClose={() => setParams({ thread: null })} />
      <PostSheet slug={slug} postId={post} onClose={() => setParams({ post: null })} />
      <TaskSheet id={tab === "work" ? null : task} slug={slug} onClose={() => setParams({ task: null })} />
      {card && !card.brain && !isCommons ? <RemoveDialog open={removing} onOpenChange={setRemoving} kind="project" name={slug} onApplied={removed} /> : null}
      {card && !card.brain && !isCommons ? <RenameDialog open={renaming} onOpenChange={setRenaming} slug={slug} name={card.name || slug} /> : null}
    </Tabs>
  );
}

// The project's sections as underlined tabs. On a phone the six don't fit:
// the strip scrolls sideways with a fading edge, and it scrolls itself (not
// the page) so the open section is always in view, also when a link opened it.
function SectionTabs({ tab, counts }: { tab: ProjectCanonicalTab; counts: Partial<Record<ProjectCanonicalTab, number>> }) {
  const strip = useRef<HTMLDivElement>(null);
  const countKey = PROJECT_TABS.map((t) => counts[t.id as ProjectCanonicalTab] || 0).join(",");
  useEffect(() => {
    const c = strip.current;
    const el = c?.querySelector<HTMLElement>('[role="tab"][data-state="active"]');
    if (!c || !el || c.scrollWidth <= c.clientWidth) return;
    const box = c.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    if (r.left < box.left) c.scrollLeft += r.left - box.left - 16;
    else if (r.right > box.right - 32) c.scrollLeft += r.right - box.right + 40;
    // The counts arrive after the first paint and widen the tabs, so it checks again then.
  }, [tab, countKey]);
  return (
    <div ref={strip} className="scroll-strip min-w-0 max-w-full">
      <TabsList variant="line" className="w-full min-w-max justify-start pr-8 sm:pr-0" aria-label="Project sections">
        {PROJECT_TABS.map((t) => {
          const id = t.id as ProjectCanonicalTab;
          const n = counts[id] || 0;
          return (
            <TabsTrigger key={t.id} value={t.id} className="flex-none px-2 text-sm sm:px-3">
              {t.label}
              {n ? (
                <>
                  <span aria-hidden className={cn("tnum inline-flex h-5 min-w-5 items-center justify-center rounded-full px-1.5 text-xs font-semibold", id === "for-you" ? "bg-you text-[oklch(0.3_0.06_60)]" : "bg-muted text-muted-foreground")}>
                    {num(n)}
                  </span>
                  <span className="sr-only">{", " + countLabel(id, n)}</span>
                </>
              ) : null}
            </TabsTrigger>
          );
        })}
      </TabsList>
    </div>
  );
}

// What a tab's count means, for screen readers ("3 things wait for you").
function countLabel(id: ProjectCanonicalTab, n: number): string {
  if (id === "for-you") return n === 1 ? "1 thing waits for you" : plural(n, "thing") + " wait for you";
  return plural(n, id === "work" ? "open task" : "open thread");
}

// The project's state in a few words, each with its dot: what waits for
// you (marigold), the work in motion (teal) and self-improvement (violet).
function StatusLine({ card, commons, forYou, openTasks: tasks, slug }: { card: ProjectCardData; commons: boolean; forYou: number; openTasks: Task[] | null; slug: string }) {
  // The same "who is on what" the Working now panel shows: task workers and live claims, one per agent.
  const agents = workingRows(card.active_claims ?? [], tasks ?? [], slug).length || (card.claims_active ? 1 : 0);
  const openTasks = tasks ? tasks.length : null;
  const loopOn = card.loop === "1" || card.loop === "on";
  const items: { text: React.ReactNode; dot: string; strong?: boolean }[] = [];
  if (forYou)
    items.push({
      text: (
        <Link href={hrefs.project(slug, "for-you")} className="underline-offset-2 hover:underline">
          {forYou === 1 ? "1 thing waits for you" : plural(forYou, "thing") + " wait for you"}
        </Link>
      ),
      dot: "bg-you",
      strong: true,
    });
  else items.push({ text: "Nothing waits for you", dot: "bg-ok" });
  if (!commons && openTasks !== null) items.push({ text: openTasks ? plural(openTasks, "open task") : "No open tasks", dot: openTasks ? "bg-primary" : "bg-muted-foreground/40" });
  items.push({ text: agents ? (agents === 1 ? "1 agent working" : plural(agents, "agent") + " working") : "No agents working", dot: agents ? "bg-primary" : "bg-muted-foreground/40" });
  if (commons) {
    const q = card.questions_open + (card.owner_questions ?? 0);
    items.push({ text: q ? plural(q, "open question") : "No open questions", dot: q ? "bg-warn" : "bg-muted-foreground/40" });
  } else if (card.stop_present) items.push({ text: "Self-improvement stopped", dot: "bg-bad" });
  else items.push({ text: loopOn ? "Self-improvement on" : "Self-improvement off", dot: loopOn ? "bg-accent-2" : "bg-muted-foreground/40" });
  return (
    <ul className="mt-3 flex flex-wrap items-center gap-x-5 gap-y-1.5 text-sm" aria-label="Project status">
      {items.map((it, i) => (
        <li key={i} className={cn("inline-flex items-center gap-2", it.strong ? "font-medium text-foreground" : "text-muted-foreground")}>
          <span className={cn("size-2 shrink-0 rounded-full", it.dot)} aria-hidden />
          {it.text}
        </li>
      ))}
    </ul>
  );
}

function WorkTab({ slug, task, setParams }: { slug: string; task: string | null; setParams: (patch: Record<string, string | null | undefined>) => void }) {
  return (
    <div className="grid gap-8 xl:grid-cols-[minmax(0,1fr)_minmax(380px,1fr)]">
      <section aria-label="Tasks" className="min-w-0">
        <SectionHeading note="What you asked for, and how far each task has come.">Tasks</SectionHeading>
        <TasksTab slug={slug} onOpen={(id) => setParams({ task: id })} activeId={task} />
      </section>
      <section aria-label="Chat" className="min-h-[520px] min-w-0">
        <SectionHeading note="Talk with the lead agent about the project or one task.">Chat</SectionHeading>
        <ChatTab slug={slug} taskId={task} onPickTask={(id) => setParams({ task: id })} />
      </section>
    </div>
  );
}

function BoardTab({ slug, value, onValueChange, thread, setParams }: { slug: string; value: BoardSub; onValueChange: (next: BoardSub) => void; thread: string | null; setParams: (patch: Record<string, string | null | undefined>) => void }) {
  const note = BOARD_SUBS.find((s) => s.id === value)?.note;
  return (
    <Tabs value={value} onValueChange={(next) => onValueChange(boardSub(next))} className="gap-0">
      <div className="mb-5 grid gap-2">
        <div className="scroll-strip min-w-0 max-w-full">
          <TabsList aria-label="Board sections">
            {BOARD_SUBS.map((s) => (
              <TabsTrigger key={s.id} value={s.id} className="px-3">
                {s.label}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>
        {note ? <p className="text-sm text-muted-foreground">{note}</p> : null}
      </div>
      {value === "threads" ? <ThreadsTab slug={slug} onOpen={(k) => setParams({ thread: k })} activeKey={thread} /> : null}
      {value === "suggestions" ? <SuggestionsTab slug={slug} /> : null}
      {value === "amendments" ? <AmendmentsTab slug={slug} /> : null}
      {value === "notices" ? <NoticesTab slug={slug} /> : null}
    </Tabs>
  );
}

function HistoryTab({ slug, value, onValueChange }: { slug: string; value: HistorySub; onValueChange: (next: HistorySub) => void }) {
  const note = HISTORY_SUBS.find((s) => s.id === value)?.note;
  return (
    <Tabs value={value} onValueChange={(next) => onValueChange(historySub(next))} className="gap-0">
      <div className="mb-5 grid gap-2">
        <div className="scroll-strip min-w-0 max-w-full">
          <TabsList aria-label="History sections">
            {HISTORY_SUBS.map((s) => (
              <TabsTrigger key={s.id} value={s.id} className="px-3">
                {s.label}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>
        {note ? <p className="text-sm text-muted-foreground">{note}</p> : null}
      </div>
      {value === "timeline" ? <TimelineTab slug={slug} /> : null}
      {value === "env" ? <EnvTab slug={slug} /> : null}
      {value === "cleanup" ? <CleanupTab slug={slug} /> : null}
    </Tabs>
  );
}

function ProjectAvatar({ slug }: { slug: string }) {
  return <ProjectMascot slug={slug} size={40} className="pop" />;
}

// One removed project: PULSE no longer works on it, its history is kept,
// and Restore brings it back.
function InactivePick({ p, onRestored }: { p: RegisteredProject; onRestored: (slug: string) => void }) {
  return (
    <ItemCard ariaLabel={p.slug + " (removed)"}>
      <ItemCardHeader right={p.last_seen ? <span>Last seen <When d={p.last_seen} /></span> : null}>
        <ItemCardKind icon={ArchiveIcon}>Removed</ItemCardKind>
        {!p.present ? <span className="text-bad">Folder is missing</span> : null}
      </ItemCardHeader>
      <ItemCardTitle>{p.slug}</ItemCardTitle>
      {p.name && p.name !== p.slug ? <ItemCardBody clamp={1}>{p.name}</ItemCardBody> : null}
      <ItemCardMeta>
        <Mono className="text-muted-foreground">{p.path}</Mono>
      </ItemCardMeta>
      <ItemCardActions>
        <span className="text-[13px] text-muted-foreground">{p.present ? "Restoring adds it back from the same folder." : "Put the folder back first; PULSE can't restore it without it."}</span>
        <span className="sm:ml-auto">
          <RestoreButton slug={p.slug} onRestored={(r) => onRestored(r.project.slug)} />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}
