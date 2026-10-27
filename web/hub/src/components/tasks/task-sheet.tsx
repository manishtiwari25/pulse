"use client";

import Link from "next/link";
import { useState } from "react";
import { ArrowRightIcon, ArrowUpIcon, BanIcon, MoreHorizontalIcon, PencilIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ChatPane } from "@/components/chat/chat-pane";
import { type Column } from "@/components/pulse/data-table";
import { SectionHeading } from "@/components/pulse/item-card";
import { Markdown } from "@/components/pulse/markdown";
import { Disclosure, EmptyState, ErrorNotice, Kv, SkeletonRows } from "@/components/pulse/panel";
import { Avatar, Mono, ProjectLink, StatusBadge, ToneBadge, When, Who } from "@/components/pulse/who";
import { sentence } from "@/lib/format";
import { cn } from "@/lib/utils";
import { priorityWord, statusWord } from "@/lib/words";
import { hrefs } from "@/lib/nav";
import { enc, taskProgress, type Task, type TaskDetail, type TaskJob } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { CancelTaskDialog, TaskNeedsYou, TaskProgressBar } from "./task-card";
import { AmendTaskDialog } from "./task-dialogs";
import { Ago, idleLine, TaskWorkerInline, TaskWorkingLine } from "./task-work";

export const TASK_COLUMNS = (showProject: boolean): Column<Task>[] => [
  {
    key: "title",
    label: "Task",
    sort: (t) => t.title,
    render: (t) => (
      <span className="block min-w-0">
        <span className="block font-medium anywhere">{t.title}</span>
        <span className="block text-xs text-muted-foreground">{t.id}</span>
      </span>
    ),
  },
  ...(showProject ? [{ key: "project", label: "Project", sort: (t: Task) => t.project, render: (t: Task) => <ProjectLink slug={t.project} className="font-normal" /> } as Column<Task>] : []),
  { key: "status", label: "Status", nowrap: true, sort: (t) => t.status, render: (t) => <StatusBadge status={t.status} /> },
  { key: "priority", label: "Priority", sort: (t) => t.priority, hide: "sm", render: (t) => <span title={"Priority " + t.priority}>{priorityWord(t.priority)}</span> },
  {
    key: "working",
    label: "Working on it",
    hide: "md",
    render: (t) => {
      const workers = taskProgress(t).working;
      if (t.status === "done" || t.status === "cancelled") return <span className="text-muted-foreground">{"\u2014"}</span>;
      if (workers.length) return <span className="inline-flex flex-wrap items-center gap-1.5">{workers.map((w, i) => <Who key={w.job + i} row={{ agent: w.agent }} compact />)}</span>;
      return t.lead ? <span className="text-muted-foreground" title={"Lead agent: " + t.lead}>Nobody right now</span> : <span className="text-muted-foreground">No lead agent yet</span>;
    },
  },
  {
    key: "jobs",
    label: "Steps done",
    hide: "lg",
    sort: (t) => taskProgress(t).total,
    render: (t) => {
      const p = taskProgress(t);
      return p.total ? <span className="tnum whitespace-nowrap">{p.done + " of " + p.total}{p.failed ? <span className="text-bad">{", " + p.failed + " failed"}</span> : null}</span> : <span className="text-muted-foreground">{t.status === "done" || t.status === "cancelled" ? "\u2014" : "None yet"}</span>;
    },
  },
  { key: "updated", label: "Updated", nowrap: true, sort: (t) => t.updated_at, render: (t) => <When d={t.updated_at} /> },
];

export function TaskSheet({ id, slug, onClose }: { id: string | null; slug?: string; onClose: () => void }) {
  return (
    <Sheet open={!!id} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="flex flex-col gap-0 overflow-hidden rounded-l-xl p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-3xl">
        {id ? <TaskBody id={id} slug={slug} /> : null}
      </SheetContent>
    </Sheet>
  );
}

// A job's moment in plain words: when it finished, started or was added.
function jobWhen(j: TaskJob): [string, string | undefined] {
  if (j.finished_at) return [j.status === "failed" ? "Stopped" : "Finished", j.finished_at];
  if (j.leased_at) return ["Started", j.leased_at];
  if (j.created_at) return ["Added", j.created_at];
  return ["Updated", j.updated_at];
}

function JobRow({ j, t, working }: { j: TaskJob; t: Task; working?: ReturnType<typeof taskProgress>["working"][number] }) {
  const [label, at] = jobWhen(j);
  const tip = ["Job " + j.id, j.kind ? "kind: " + j.kind : "", j.branch ? "branch " + j.branch : "", j.executor ? "machine " + j.executor : ""].filter(Boolean).join(", ");
  return (
    <li className="grid min-w-0 gap-1.5 px-4 py-3" title={tip}>
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-3 gap-y-1">
        <p className="min-w-0 flex-1 basis-60 text-sm font-medium leading-snug text-foreground anywhere">{sentence(j.title) || "A step"}</p>
        <StatusBadge status={j.status} />
      </div>
      {working ? (
        <p className="text-[13px] leading-relaxed text-muted-foreground">
          <TaskWorkerInline t={t} w={working} bare />
        </p>
      ) : (
        <p className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1 text-[13px] text-muted-foreground">
          {j.agent ? (
            <span className="inline-flex items-center gap-1.5">
              <Avatar seed={j.agent} size={18} />
              {j.agent}
            </span>
          ) : !j.finished_at ? (
            <span>No agent on it yet</span>
          ) : null}
          {at ? (
            <span>
              {label + " "}
              <Ago d={at} />
            </span>
          ) : null}
          {j.harness_session ? (
            <Link href={hrefs.harness({ id: j.harness_session })} title={"Run " + j.harness_session} className="font-medium text-primary underline-offset-2 hover:underline">
              See the run
            </Link>
          ) : null}
        </p>
      )}
      <Disclosure summary="Technical details" contentClassName="mt-1.5">
        <Kv
          className="text-[13px]"
          rows={[
            ["Job", <Mono key="id">{j.id}</Mono>],
            ...(j.kind ? [["Kind", <Mono key="kind">{j.kind}</Mono>] as [string, React.ReactNode]] : []),
            ...(j.branch ? [["Branch", <Mono key="branch">{j.branch}</Mono>] as [string, React.ReactNode]] : []),
            ...(j.executor ? [["Machine", <Mono key="machine">{j.executor}</Mono>] as [string, React.ReactNode]] : []),
          ]}
        />
      </Disclosure>
    </li>
  );
}

function TaskBody({ id, slug }: { id: string; slug?: string }) {
  const { data, error, loading, reload } = useApi<TaskDetail>("tasks/" + enc(id), { refresh: 10000 });
  const [amend, setAmend] = useState(false);
  const [cancel, setCancel] = useState(false);
  const [tab, setTab] = useState<string | null>(null);
  if (error && !data)
    return (
      <div className="p-5">
        <SheetHeader className="px-0">
          <SheetTitle>{id}</SheetTitle>
          <SheetDescription>This task didn&apos;t load.</SheetDescription>
        </SheetHeader>
        <ErrorNotice error={error} onRetry={() => void reload()} />
      </div>
    );
  if (!data || (loading && !data))
    return (
      <div className="p-5">
        <SheetHeader className="px-0">
          <SheetTitle>{id}</SheetTitle>
          <SheetDescription>Loading the task…</SheetDescription>
        </SheetHeader>
        <SkeletonRows n={6} />
      </div>
    );
  const t = data.task;
  const progress = taskProgress(t);
  const project = t.project || slug || "";
  const closed = t.status === "done" || t.status === "cancelled";
  const review = t.status === "review";
  const idle = idleLine(t);
  const revisions = [...data.revisions].sort((a, b) => b.rev - a.rev);
  const by = (a: string) => (a === "human" || a === "owner" ? "you" : a);
  const current = tab ?? (review ? "chat" : "brief");
  const chat = current === "chat";
  return (
    <>
      {/* On a phone the conversation needs the room: on that tab the header
          folds to the title and a "More actions" menu (the facts stay for
          screen readers). */}
      <SheetHeader className={cn("gap-2 border-b bg-card px-5 pt-5 pb-4", chat && "max-sm:gap-0 max-sm:px-4 max-sm:py-3")}>
        <div className={cn("flex flex-wrap items-center gap-x-3 gap-y-1.5 pr-8 text-[13px] text-muted-foreground", chat && "max-sm:hidden")}>
          <StatusBadge status={t.status} />
          {!slug ? <ProjectLink slug={project} className="font-normal text-muted-foreground" /> : null}
          {t.priority >= 30 ? <ToneBadge tone={t.priority >= 50 ? "bad" : "neutral"}><ArrowUpIcon className="size-3" aria-hidden />{priorityWord(t.priority) + " priority"}</ToneBadge> : <span>{priorityWord(t.priority) + " priority"}</span>}
          <span title={t.job_id ? "Planning job " + t.job_id : undefined}>{t.id}</span>
        </div>
        <div className={cn("flex min-w-0 items-center gap-2", chat && "max-sm:pr-8")}>
          <SheetTitle className={cn("min-w-0 flex-1 text-xl font-semibold leading-snug tracking-tight anywhere", chat && "max-sm:line-clamp-1 max-sm:text-base")} title={chat ? t.title : undefined}>
            {t.title}
          </SheetTitle>
          {chat ? (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button size="icon-sm" variant="ghost" className="shrink-0 sm:hidden" aria-label={"More actions for " + t.title}>
                  <MoreHorizontalIcon />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-48">
                <DropdownMenuItem disabled={closed} onSelect={() => setAmend(true)}>
                  <PencilIcon /> Edit the task
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem variant="destructive" disabled={closed} onSelect={() => setCancel(true)}>
                  <BanIcon /> Cancel the task
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          ) : null}
        </div>
        <SheetDescription className={cn("flex flex-wrap items-center gap-x-4 gap-y-1 text-[13px]", chat && "max-sm:sr-only")}>
          <span className="inline-flex items-center gap-1.5">
            {t.lead ? (
              <>
                Lead agent <Who row={{ agent: t.lead }} compact />
              </>
            ) : closed ? (
              <span>No lead agent worked on it</span>
            ) : (
              <span title="The runner keeps one lead agent per active project and wakes it for this task.">No lead agent yet. One is woken for this task soon.</span>
            )}
          </span>
          <span>
            {"Given by " + by(t.created_by) + " "}
            <Ago d={t.created_at} />
          </span>
          <span>
            {"Updated "}
            <Ago d={t.updated_at} />
          </span>
        </SheetDescription>
        <div className={cn("mt-1 flex flex-wrap gap-2", chat && "max-sm:hidden")}>
          <Button size="sm" variant="outline" onClick={() => setAmend(true)} disabled={closed} title={closed ? "A " + statusWord(t.status).toLowerCase() + " task can't be changed. Give a new one instead." : undefined}>
            <PencilIcon /> Edit the task
          </Button>
          <Button size="sm" variant="ghost" className="text-bad hover:bg-bad/10 hover:text-bad" onClick={() => setCancel(true)} disabled={closed}>
            <BanIcon /> Cancel the task
          </Button>
        </div>
      </SheetHeader>
      <Tabs value={current} onValueChange={setTab} className="flex min-h-0 flex-1 flex-col gap-0">
        <TabsList variant="line" className="w-full justify-start px-4">
          <TabsTrigger value="brief" className="flex-none">
            Overview
          </TabsTrigger>
          <TabsTrigger value="chat" className="flex-none">
            Conversation <span className="tnum rounded-full bg-muted px-1.5 text-xs font-normal leading-5 text-muted-foreground">{data.messages.length}</span>
          </TabsTrigger>
        </TabsList>
        <TabsContent value="brief" className="scroll-thin min-h-0 flex-1 overflow-y-auto px-5 py-5">
          <div className="grid gap-8">
            {review ? <TaskNeedsYou t={t} onChat={() => setTab("chat")} /> : null}
            <section aria-labelledby="task-asked">
              <SectionHeading className="mb-2">
                <span id="task-asked">What you asked</span>
              </SectionHeading>
              <div className="rounded-2xl border bg-card px-4 py-3 text-[15px]">
                <Markdown source={t.brief} />
              </div>
            </section>
            <section aria-labelledby="task-progress">
              <SectionHeading className="mb-2" count={data.jobs.length || undefined} note={data.jobs.length ? "The steps the lead agent split this task into." : undefined}>
                <span id="task-progress">Progress</span>
              </SectionHeading>
              <div className="grid gap-3">
                <TaskProgressBar t={t} />
                {progress.working.length && !data.jobs.length ? <TaskWorkingLine t={t} /> : null}
                {idle ? <p className="text-[13px] leading-relaxed text-muted-foreground">{idle}</p> : null}
                {data.jobs.length ? (
                  <ul className="divide-y overflow-hidden rounded-2xl border bg-card">
                    {data.jobs.map((j) => (
                      <JobRow key={j.id} j={j} t={t} working={progress.working.find((w) => w.job === j.id)} />
                    ))}
                  </ul>
                ) : (
                  <EmptyState title={closed ? "No steps" : "No steps yet"} className="py-6">
                    {t.status === "cancelled" ? "This task was cancelled before the lead agent split it into steps." : closed ? "This task was finished without separate steps." : "When the lead agent splits the task into steps, each one shows up here with who works on it and how it went."}
                  </EmptyState>
                )}
              </div>
            </section>
            <section aria-labelledby="task-history">
              <SectionHeading className="mb-2" note={revisions.length > 1 ? "Every time the task was changed, the agents got a new version." : undefined}>
                <span id="task-history">History</span>
              </SectionHeading>
              <div className="rounded-2xl border bg-card px-3 py-1">
                <Disclosure variant="row" summary={revisions.length === 1 ? "1 version" : revisions.length + " versions"} hint={revisions.length > 1 ? "Latest: version " + t.current_rev : "Not changed since you gave it"} contentClassName="mt-1">
                  <ol className="-mx-3 grid gap-0 divide-y border-t">
                    {revisions.map((r) => (
                      <li key={r.rev} className="grid gap-1 px-4 py-3 text-sm">
                        <p className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
                          <span className="font-medium text-foreground">{"Version " + r.rev + (r.rev === t.current_rev ? " (current)" : "")}</span>
                          <span className="text-[13px] text-muted-foreground">
                            {"by " + by(r.author) + ", "}
                            <Ago d={r.at} />
                          </span>
                        </p>
                        {/* The note and title are the owner's own words: shown as typed. */}
                        {r.note ? <p className="leading-relaxed text-foreground anywhere">{r.note}</p> : null}
                        {r.title !== t.title ? <p className="text-[13px] text-muted-foreground anywhere">{"Titled: " + r.title}</p> : null}
                        {r.rev !== t.current_rev ? (
                          <Disclosure summary="What you asked in this version" className="mt-1">
                            <div className="rounded-xl bg-muted/50 px-3 py-2 text-[13px]">
                              <Markdown source={r.brief} />
                            </div>
                          </Disclosure>
                        ) : null}
                      </li>
                    ))}
                  </ol>
                </Disclosure>
              </div>
            </section>
          </div>
        </TabsContent>
        <TabsContent value="chat" className="flex min-h-0 flex-1 flex-col overflow-hidden">
          <div className="flex flex-wrap items-center justify-between gap-2 border-b px-5 py-2.5 text-[13px] text-muted-foreground">
            <span>{t.lead ? "Your conversation with " + t.lead + ", the lead agent." : "Your conversation with the project's lead agent."}</span>
            <Link href={hrefs.chat(project, t.id)} className="inline-flex items-center gap-1 font-medium text-foreground underline-offset-2 hover:underline">
              Open in the project <ArrowRightIcon className="size-3.5" aria-hidden />
            </Link>
          </div>
          <div className="min-h-0 flex-1">
            <ChatPane project={project} taskId={t.id} task={t} messages={data.messages} lead={t.lead} embedded />
          </div>
        </TabsContent>
      </Tabs>
      {amend ? <AmendTaskDialog task={t} open={amend} onOpenChange={setAmend} /> : null}
      <CancelTaskDialog t={t} open={cancel} onOpenChange={setCancel} />
    </>
  );
}
