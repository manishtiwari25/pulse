"use client";

import { Suspense, useCallback, useMemo, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { BanIcon, BotOffIcon, CheckCircle2Icon, CircleDashedIcon, EyeIcon, LayersIcon, ListChecksIcon, ListTodoIcon, LoaderIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { DataTable } from "@/components/pulse/data-table";
import { Count, EmptyState, ErrorNotice, Kpi, KpiGrid, PageTitle, SkeletonPage, SkeletonRows } from "@/components/pulse/panel";
import { useViewMode, ViewToggle } from "@/components/pulse/view-toggle";
import { TaskCards } from "@/components/tasks/task-card";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { TASK_COLUMNS, TaskSheet } from "@/components/tasks/task-sheet";
import { registeredProjects, TASK_STATUSES, type Org, type Task, type TasksPayload } from "@/lib/api";
import { plural } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

export default function TasksPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={3} />}>
      <TasksScreen />
    </Suspense>
  );
}

const isActive = (t: Task) => t.status !== "done" && t.status !== "cancelled";

// "active" is the default and means no `status` param, so links from the
// palette and other pages (hrefs.tasks) keep working unchanged.
const STATUS_CHIPS: [string, string][] = [["active", "active"], ["all", "all"], ...TASK_STATUSES.map((s): [string, string] => [s, s.replace("_", " ")])];

// The status filter as a row of chips instead of a Select: the whole
// pipeline and its counts are visible at a glance and one tap switches.
function StatusChips({ value, count, onChange }: { value: string; count: (s: string) => number; onChange: (s: string) => void }) {
  return (
    <div role="group" aria-label="Status" className="inline-flex shrink-0 items-center gap-0.5 rounded-lg border bg-muted/40 p-0.5">
      {STATUS_CHIPS.map(([v, label]) => {
        const on = value === v;
        return (
          <Button key={v} size="xs" variant="ghost" aria-pressed={on} onClick={() => onChange(v)} className={cn("h-7 gap-1 rounded-md px-2.5 text-xs font-medium hover:bg-muted", on ? "bg-card text-foreground shadow-card hover:bg-card" : "text-muted-foreground hover:text-foreground")}>
            {label}
            <span className={cn("tnum rounded-full bg-muted px-1.5 text-[10.5px] leading-none text-muted-foreground", on && "bg-primary/10 text-primary")}>{count(v)}</span>
          </Button>
        );
      })}
    </div>
  );
}

function TasksScreen() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const project = sp.get("project") || "";
  const status = sp.get("status") || "active";
  const task = sp.get("task");
  const [text, setText] = useState("");
  const [view, setView] = useViewMode("tasks");
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
  const q = useApi<TasksPayload>("tasks", { refresh: 10000 });
  const org = useApi<Org>("org", { refresh: 60000 });
  const projects = useMemo(() => (org.data ? registeredProjects(org.data).map((p) => p.slug) : Array.from(new Set((q.data?.tasks ?? []).map((t) => t.project)))), [org.data, q.data]);
  const all = useMemo(() => q.data?.tasks ?? [], [q.data]);
  // Counts in the chips and the strip follow the project filter, so they
  // describe the list the owner is looking at rather than the whole org.
  const scope = useMemo(() => (project ? all.filter((x) => x.project === project) : all), [all, project]);
  const rows = useMemo(() => {
    const t = text.trim().toLowerCase();
    return scope.filter((x) => (status === "all" ? true : status === "active" ? isActive(x) : x.status === status) && (!t || x.title.toLowerCase().includes(t) || x.id.toLowerCase().includes(t) || (x.brief || "").toLowerCase().includes(t)));
  }, [scope, status, text]);
  const by = (s: string) => (s === "all" ? scope.length : s === "active" ? scope.filter(isActive).length : scope.filter((x) => x.status === s).length);
  const link = (s: string) => hrefs.tasks({ project: project || undefined, status: s });
  const empty = all.length ? (
    "Nothing matches these filters."
  ) : (
    <>
      No task yet. Click <b>New task</b>, pick the project, write the brief in Markdown - the runner wakes that project&apos;s lead, which plans jobs and answers in the task&apos;s conversation.
    </>
  );
  const emptyTitle = all.length ? "Nothing matches these filters." : "No task yet";
  const emptyAction = all.length ? undefined : <NewTaskButton project={project || undefined} onCreated={(t) => setParams({ task: t.id })} />;
  return (
    <>
      <PageTitle
        icon={ListTodoIcon}
        sub="What you asked for, across every project. The project's lead plans each task into jobs and talks with you in its conversation."
        actions={<NewTaskButton project={project || undefined} onCreated={(t) => setParams({ task: t.id })} />}
        bar={
          <div className="flex w-max items-center gap-2 md:w-auto md:flex-wrap">
            <Select value={project || "__all"} onValueChange={(v) => setParams({ project: v === "__all" ? null : v })}>
              <SelectTrigger size="sm" aria-label="Project" className="w-40">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__all">all projects</SelectItem>
                {projects.map((p) => (
                  <SelectItem key={p} value={p}>
                    {p}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <StatusChips value={status} count={by} onChange={(v) => setParams({ status: v === "active" ? null : v })} />
            <Input aria-label="Filter tasks" placeholder="filter…" value={text} onChange={(e) => setText(e.target.value)} className="h-7 w-32 xl:w-40" />
            <span className="ml-auto flex shrink-0 items-center gap-2">
              <Count>{q.data ? plural(rows.length, "task") : ""}</Count>
              <ViewToggle value={view} onChange={setView} />
            </span>
          </div>
        }
      >
        Tasks
      </PageTitle>
      <KpiGrid>
        <Kpi icon={CircleDashedIcon} n={by("open")} label="open" sub="waiting for a lead" tone={by("open") ? "info" : "neutral"} href={link("open")} />
        <Kpi icon={ListChecksIcon} n={by("planned")} label="planned" sub="jobs written" href={link("planned")} />
        <Kpi icon={LoaderIcon} n={by("in_progress")} label="in progress" tone={by("in_progress") ? "ok" : "neutral"} sub="jobs running" href={link("in_progress")} />
        <Kpi icon={EyeIcon} n={by("review")} label="review" tone={by("review") ? "warn" : "neutral"} sub="the lead needs you" href={link("review")} />
        <Kpi icon={CheckCircle2Icon} n={by("done")} label="done" href={link("done")} />
        <Kpi icon={BanIcon} n={by("cancelled")} label="cancelled" href={link("cancelled")} />
        <Kpi icon={BotOffIcon} n={scope.filter((x) => !x.lead && isActive(x)).length} label="no lead yet" sub="runner wakes one" />
        <Kpi icon={LayersIcon} n={scope.length} label="all time" href={link("all")} />
      </KpiGrid>
      {q.error && !q.data ? (
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      ) : !q.data ? (
        <SkeletonRows n={8} />
      ) : view === "table" ? (
        rows.length ? (
          <div className="overflow-hidden rounded-lg border bg-card">
            <DataTable columns={TASK_COLUMNS(true)} rows={rows} rowKey={(t) => t.id} onRow={(t) => setParams({ task: t.id })} initialSort="updated" caption="Tasks" />
          </div>
        ) : (
          <EmptyState title={emptyTitle} action={emptyAction}>
            {empty}
          </EmptyState>
        )
      ) : (
        <TaskCards tasks={rows} showProject onOpen={(t) => setParams({ task: t.id })} activeId={task} cols={3} empty={empty} emptyTitle={emptyTitle} emptyAction={emptyAction} />
      )}
      <TaskSheet id={task} onClose={() => setParams({ task: null })} />
    </>
  );
}
