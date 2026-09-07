"use client";

import Link from "next/link";
import { useState } from "react";
import { BanIcon, HistoryIcon, MessagesSquareIcon, PencilIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ChatPane } from "@/components/chat/chat-pane";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { DataTable, type Column } from "@/components/pulse/data-table";
import { Markdown } from "@/components/pulse/markdown";
import { EmptyState, ErrorNotice, Kv, SkeletonRows } from "@/components/pulse/panel";
import { Mono, ProjectLink, StatusBadge, ToneBadge, When, Who } from "@/components/pulse/who";
import { prioLabel } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { enc, writes, type Task, type TaskDetail, type TaskJob } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { AmendTaskDialog } from "./task-dialogs";

export const TASK_COLUMNS = (showProject: boolean): Column<Task>[] => [
  { key: "id", label: "task", nowrap: true, sort: (t) => t.id, render: (t) => <Mono className="font-medium">{t.id}</Mono> },
  ...(showProject ? [{ key: "project", label: "project", sort: (t: Task) => t.project, render: (t: Task) => <ProjectLink slug={t.project} /> } as Column<Task>] : []),
  { key: "title", label: "title", render: (t) => <span className="font-medium anywhere">{t.title}</span> },
  { key: "status", label: "status", sort: (t) => t.status, render: (t) => <StatusBadge status={t.status} /> },
  { key: "priority", label: "priority", align: "right", sort: (t) => t.priority, hide: "sm", render: (t) => <span title={String(t.priority)}>{prioLabel(t.priority)}</span> },
  { key: "lead", label: "lead", hide: "md", render: (t) => (t.lead ? <Who row={{ agent: t.lead }} compact /> : <span className="text-muted-foreground">not yet</span>) },
  { key: "rev", label: "rev", align: "right", hide: "lg", render: (t) => t.current_rev },
  { key: "updated", label: "updated", nowrap: true, sort: (t) => t.updated_at, render: (t) => <When d={t.updated_at} /> },
];

const JOB_COLUMNS: Column<TaskJob>[] = [
  { key: "id", label: "job", nowrap: true, render: (j) => <Mono>{j.id}</Mono> },
  { key: "kind", label: "kind", render: (j) => <ToneBadge tone="neutral">{j.kind}</ToneBadge> },
  { key: "title", label: "title", render: (j) => <span className="anywhere">{j.title}</span> },
  { key: "status", label: "status", render: (j) => <StatusBadge status={j.status} /> },
  { key: "agent", label: "agent", render: (j) => (j.agent ? <Who row={{ agent: j.agent }} compact /> : <span className="text-muted-foreground">{"—"}</span>) },
  { key: "branch", label: "branch", render: (j) => (j.branch ? <Mono>{j.branch}</Mono> : null) },
];

export function TaskSheet({ id, slug, onClose }: { id: string | null; slug?: string; onClose: () => void }) {
  return (
    <Sheet open={!!id} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="flex flex-col gap-0 overflow-hidden p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-3xl">
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
  if (!data || loading && !data)
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
  const project = t.project || slug || "";
  const closed = t.status === "done" || t.status === "cancelled";
  return (
    <>
      <SheetHeader className="border-b px-4 py-3">
        <div className="flex flex-wrap items-center gap-1.5 pr-8 text-xs">
          <Mono className="font-semibold">{t.id}</Mono>
          <StatusBadge status={t.status} />
          <ToneBadge tone="neutral">{prioLabel(t.priority) + " priority"}</ToneBadge>
          <ToneBadge tone="neutral">{"rev " + t.current_rev}</ToneBadge>
          {!slug ? <ProjectLink slug={project} className="text-xs" /> : null}
        </div>
        <SheetTitle className="text-base leading-snug anywhere">{t.title}</SheetTitle>
        <SheetDescription className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <span>
            opened <When d={t.created_at} /> by {t.created_by === "human" ? "you" : t.created_by}
          </span>
          <span>
            updated <When d={t.updated_at} />
          </span>
          <span>lead: {t.lead ? <b className="text-foreground">{t.lead}</b> : <span>none yet - the runner keeps one lead per active project and wakes it for this task</span>}</span>
          {t.job_id ? (
            <span>
              job <Mono>{t.job_id}</Mono>
            </span>
          ) : null}
        </SheetDescription>
        <div className="mt-1 flex flex-wrap gap-2">
          <Button size="sm" variant="outline" onClick={() => setAmend(true)} disabled={closed} title={closed ? "a " + t.status + " task cannot be amended - open a new one" : undefined}>
            <PencilIcon /> Amend
          </Button>
          <Button size="sm" variant="outline" className="text-bad" onClick={() => setCancel(true)} disabled={closed}>
            <BanIcon /> Cancel task
          </Button>
          <Button size="sm" variant="ghost" asChild>
            <Link href={hrefs.chat(project, t.id)}>
              <MessagesSquareIcon /> Open in chat
            </Link>
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
        <TabsContent value="brief" className="min-h-0 flex-1 overflow-y-auto p-4">
          <Markdown source={t.brief} />
        </TabsContent>
        <TabsContent value="chat" className="min-h-0 flex-1 overflow-hidden">
          <ChatPane project={project} taskId={t.id} messages={data.messages} lead={t.lead} embedded />
        </TabsContent>
        <TabsContent value="jobs" className="min-h-0 flex-1 overflow-y-auto p-4">
          {data.jobs.length ? <DataTable columns={JOB_COLUMNS} rows={data.jobs} rowKey={(j) => j.id} dense /> : <EmptyState>No job yet. The lead breaks the task into jobs with the CLI (pulse job add --source task:{t.id}); they appear here with their branch and status.</EmptyState>}
        </TabsContent>
        <TabsContent value="history" className="min-h-0 flex-1 overflow-y-auto p-4">
          <ol className="grid gap-3">
            {[...data.revisions].sort((a, b) => b.rev - a.rev).map((r) => (
              <li key={r.rev} className="rounded-md border p-3">
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
