"use client";

import Link from "next/link";
import { useState } from "react";
import { BanIcon, BriefcaseBusinessIcon, FileTextIcon, HistoryIcon, MessagesSquareIcon, PencilIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ChatPane } from "@/components/chat/chat-pane";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { type Column } from "@/components/pulse/data-table";
import { SectionHeading } from "@/components/pulse/item-card";
import { Markdown } from "@/components/pulse/markdown";
import { EmptyState, ErrorNotice, Kv, SkeletonRows } from "@/components/pulse/panel";
import { Mono, ProjectLink, StatusBadge, ToneBadge, When, Who } from "@/components/pulse/who";
import { prioLabel } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { enc, taskProgress, writes, type Task, type TaskDetail } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { AmendTaskDialog } from "./task-dialogs";
import { TaskWorkerInline, TaskWorkingLine } from "./task-work";

export const TASK_COLUMNS = (showProject: boolean): Column<Task>[] => [
  { key: "id", label: "task", nowrap: true, sort: (t) => t.id, render: (t) => <Mono className="font-medium">{t.id}</Mono> },
  ...(showProject ? [{ key: "project", label: "project", sort: (t: Task) => t.project, render: (t: Task) => <ProjectLink slug={t.project} /> } as Column<Task>] : []),
  { key: "title", label: "title", render: (t) => <span className="font-medium anywhere">{t.title}</span> },
  { key: "status", label: "status", sort: (t) => t.status, render: (t) => <StatusBadge status={t.status} /> },
  { key: "priority", label: "priority", align: "right", sort: (t) => t.priority, hide: "sm", render: (t) => <span title={String(t.priority)}>{prioLabel(t.priority)}</span> },
  { key: "lead", label: "lead", hide: "md", render: (t) => (t.lead ? <Who row={{ agent: t.lead }} compact /> : <span className="text-muted-foreground">not yet</span>) },
  { key: "working", label: "working", hide: "md", render: (t) => {
    const workers = taskProgress(t).working;
    return workers.length ? <span className="inline-flex flex-wrap items-center gap-1.5">{workers.map((w, i) => <span key={w.job + i} className="inline-flex items-center gap-1">{i ? <span className="text-muted-foreground">,</span> : null}<Who row={{ agent: w.agent }} compact /></span>)}</span> : <span className="text-muted-foreground">—</span>;
  } },
  { key: "jobs", label: "jobs", align: "right", hide: "lg", sort: (t) => taskProgress(t).total, render: (t) => {
    const p = taskProgress(t);
    return p.total ? <span className="tnum">{p.done + "/" + p.total}{p.failed ? <span className="text-bad">{" · " + p.failed + " failed"}</span> : null}</span> : <span className="text-muted-foreground">—</span>;
  } },
  { key: "rev", label: "rev", align: "right", hide: "lg", render: (t) => t.current_rev },
  { key: "updated", label: "updated", nowrap: true, sort: (t) => t.updated_at, render: (t) => <When d={t.updated_at} /> },
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

function TaskBody({ id, slug }: { id: string; slug?: string }) {
  const { data, error, loading, reload } = useApi<TaskDetail>("tasks/" + enc(id), { refresh: 10000 });
  const [amend, setAmend] = useState(false);
  const [cancel, setCancel] = useState(false);
  if (error && !data)
    return (
      <div className="p-4">
        <SheetHeader className="px-0">
          <SheetTitle>{id}</SheetTitle>
          <SheetDescription>Task</SheetDescription>
        </SheetHeader>
        <ErrorNotice error={error} onRetry={() => void reload()} />
      </div>
    );
  if (!data || (loading && !data))
    return (
      <div className="p-4">
        <SheetHeader className="px-0">
          <SheetTitle>{id}</SheetTitle>
          <SheetDescription>Loading…</SheetDescription>
        </SheetHeader>
        <SkeletonRows n={6} />
      </div>
    );
  const t = data.task;
  const progress = taskProgress(t);
  const project = t.project || slug || "";
  const closed = t.status === "done" || t.status === "cancelled";
  return (
    <>
      <SheetHeader className="border-b bg-card px-4 py-3">
        <div className="flex flex-wrap items-center gap-1.5 pr-8 text-xs">
          <Mono className="font-semibold">{t.id}</Mono>
          <StatusBadge status={t.status} />
          <ToneBadge tone="neutral">{prioLabel(t.priority) + " priority"}</ToneBadge>
          <ToneBadge tone="neutral">{"rev " + t.current_rev}</ToneBadge>
          {!slug ? <ProjectLink slug={project} className="text-xs" /> : null}
        </div>
        <SheetTitle className="text-lg font-semibold leading-snug anywhere">{t.title}</SheetTitle>
        <SheetDescription className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <span className="inline-flex items-center gap-1.5">
            lead: {t.lead ? <Who row={{ agent: t.lead }} compact /> : <span>none yet - the runner keeps one lead per active project and wakes it for this task</span>}
          </span>
          <span>
            opened <When d={t.created_at} /> by {t.created_by === "human" ? "you" : t.created_by}
          </span>
          <span>
            updated <When d={t.updated_at} />
          </span>
          {t.job_id ? (
            <span>
              job <Mono>{t.job_id}</Mono>
            </span>
          ) : null}
          <TaskWorkingLine t={t} className="basis-full" />
          {progress.total ? (
            <span className="inline-flex basis-full flex-wrap items-center gap-1.5">
              <ToneBadge tone="neutral">{progress.total + " jobs"}</ToneBadge>
              {progress.done ? <ToneBadge tone="ok">{progress.done + " done"}</ToneBadge> : null}
              {progress.running ? <ToneBadge tone="info">{progress.running + " running"}</ToneBadge> : null}
              {progress.failed ? <ToneBadge tone="bad">{progress.failed + " failed"}</ToneBadge> : null}
            </span>
          ) : null}
        </SheetDescription>
        <div className="mt-1 flex flex-wrap gap-2">
          <Button size="sm" asChild>
            <Link href={hrefs.chat(project, t.id)}>
              <MessagesSquareIcon /> Chat
            </Link>
          </Button>
          <Button size="sm" variant="outline" onClick={() => setAmend(true)} disabled={closed} title={closed ? "a " + t.status + " task cannot be amended - open a new one" : undefined}>
            <PencilIcon /> Amend
          </Button>
          <Button size="sm" variant="destructive" onClick={() => setCancel(true)} disabled={closed}>
            <BanIcon /> Cancel task
          </Button>
        </div>
      </SheetHeader>
      <Tabs defaultValue="brief" className="flex min-h-0 flex-1 flex-col gap-0">
        <TabsList variant="line" className="mx-4 mt-2 w-fit">
          <TabsTrigger value="brief">Brief</TabsTrigger>
          <TabsTrigger value="chat">
            Conversation <span className="tnum text-muted-foreground">{data.messages.length}</span>
          </TabsTrigger>
          <TabsTrigger value="jobs">
            Jobs <span className="tnum text-muted-foreground">{data.jobs.length}</span>
          </TabsTrigger>
          <TabsTrigger value="history">
            <HistoryIcon /> Revisions <span className="tnum text-muted-foreground">{data.revisions.length}</span>
          </TabsTrigger>
        </TabsList>
        <TabsContent value="brief" className="scroll-thin min-h-0 flex-1 overflow-y-auto p-4">
          <SectionHeading icon={FileTextIcon}>Brief</SectionHeading>
          <Markdown source={t.brief} />
        </TabsContent>
        <TabsContent value="chat" className="min-h-0 flex-1 overflow-hidden">
          <div className="border-b px-4 pt-3">
            <SectionHeading icon={MessagesSquareIcon} count={data.messages.length}>
              Conversation
            </SectionHeading>
          </div>
          <ChatPane project={project} taskId={t.id} messages={data.messages} lead={t.lead} embedded />
        </TabsContent>
        <TabsContent value="jobs" className="scroll-thin min-h-0 flex-1 overflow-y-auto p-4">
          <SectionHeading icon={BriefcaseBusinessIcon} count={data.jobs.length}>
            Jobs
          </SectionHeading>
          {data.jobs.length ? (
            <ul className="divide-y rounded-lg border">
              {data.jobs.map((j) => {
                const worker = progress.working.find((w) => w.job === j.id);
                return (
                <li key={j.id} className="grid gap-1 px-3 py-2.5">
                  <div className="flex flex-wrap items-center gap-1.5 text-xs">
                    <Mono className="font-semibold">{j.id}</Mono>
                    <ToneBadge tone="neutral">{j.kind}</ToneBadge>
                    <StatusBadge status={j.status} />
                    {j.agent ? <Who row={{ agent: j.agent }} compact className="ml-auto" /> : <span className="ml-auto text-muted-foreground">no agent yet</span>}
                  </div>
                  <div className="text-[13px] leading-snug anywhere">{j.title}</div>
                  {j.harness_session ? (
                    <Link href={hrefs.harness({ id: j.harness_session })} className="w-fit text-xs font-medium underline-offset-2 hover:underline">
                      session <Mono>{j.harness_session}</Mono>
                    </Link>
                  ) : null}
                  {worker ? (
                    <div className="text-xs text-muted-foreground">
                      {"· "}
                      <TaskWorkerInline t={t} w={worker} compact />
                    </div>
                  ) : null}
                  {j.branch ? <Mono className="text-muted-foreground">{j.branch}</Mono> : null}
                </li>
                );
              })}
            </ul>
          ) : (
            <EmptyState title="No job yet">No job yet. The lead breaks the task into jobs with the CLI (pulse job add --source task:{t.id}); they appear here with their branch and status.</EmptyState>
          )}
        </TabsContent>
        <TabsContent value="history" className="scroll-thin min-h-0 flex-1 overflow-y-auto p-4">
          <SectionHeading icon={HistoryIcon} count={data.revisions.length}>
            Revisions
          </SectionHeading>
          <ol className="grid gap-3">
            {[...data.revisions].sort((a, b) => b.rev - a.rev).map((r) => (
              <li key={r.rev} className="rounded-lg border bg-card p-3 shadow-card">
                <Kv
                  rows={[
                    ["revision", <span key="r">{r.rev + (r.rev === t.current_rev ? " (current)" : "")}</span>],
                    ["by", r.author === "human" ? "you" : r.author],
                    ["at", <When key="at" d={r.at} exact />],
                    ...(r.note ? ([["note", r.note]] as [React.ReactNode, React.ReactNode][]) : []),
                    ["title", r.title],
                  ]}
                />
                {r.rev !== t.current_rev ? (
                  <details className="mt-2 text-xs">
                    <summary className="cursor-pointer text-muted-foreground hover:text-foreground">brief at this revision</summary>
                    <Separator className="my-2" />
                    <Markdown source={r.brief} />
                  </details>
                ) : null}
              </li>
            ))}
          </ol>
        </TabsContent>
      </Tabs>
      {amend ? <AmendTaskDialog task={t} open={amend} onOpenChange={setAmend} /> : null}
      <ConfirmDialog
        open={cancel}
        onOpenChange={setCancel}
        title={"Cancel " + t.id + "?"}
        description="The task is marked cancelled and its queued job is withdrawn. A job already running finishes and the lead is told. This cannot be undone - open a new task instead."
        action="Cancel the task"
        destructive
        words={{ label: "Reason", placeholder: "e.g. superseded by TASK-012", required: true }}
        success={t.id + " cancelled."}
        onConfirm={(reason) => writes.cancelTask(t.id, reason)}
        invalidates={(p) => p.startsWith("tasks") || p.startsWith("events")}
      />
    </>
  );
}
