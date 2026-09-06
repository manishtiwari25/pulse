"use client";

import { Suspense, useCallback, useMemo, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { DataTable } from "@/components/pulse/data-table";
import { Count, EmptyState, ErrorNotice, Kpi, KpiGrid, PageTitle, SkeletonPage, SkeletonRows, Toolbar } from "@/components/pulse/panel";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { TASK_COLUMNS, TaskSheet } from "@/components/tasks/task-sheet";
import { allProjects, TASK_STATUSES, type Org, type TasksPayload } from "@/lib/api";
import { plural } from "@/lib/format";
import { useApi } from "@/lib/use-api";

export default function TasksPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={3} />}>
      <TasksScreen />
    </Suspense>
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
  const projects = useMemo(() => (org.data ? allProjects(org.data).map((p) => p.slug) : Array.from(new Set((q.data?.tasks ?? []).map((t) => t.project)))), [org.data, q.data]);
  const all = useMemo(() => q.data?.tasks ?? [], [q.data]);
  const rows = useMemo(() => {
    const t = text.trim().toLowerCase();
    return all.filter((x) => (!project || x.project === project) && (status === "all" ? true : status === "active" ? x.status !== "done" && x.status !== "cancelled" : x.status === status) && (!t || x.title.toLowerCase().includes(t) || x.id.toLowerCase().includes(t) || (x.brief || "").toLowerCase().includes(t)));
  }, [all, project, status, text]);
  const by = (s: string) => all.filter((x) => x.status === s).length;
  return (
    <>
      <PageTitle sub="What you asked for, across every project. A task goes to the project's lead, who plans it into jobs and talks with you in its conversation." actions={<NewTaskButton project={project || undefined} onCreated={(t) => setParams({ task: t.id })} />}>
        Tasks
      </PageTitle>
      <KpiGrid>
        <Kpi n={by("open")} label="open" sub="waiting for a lead" tone={by("open") ? "info" : "neutral"} />
        <Kpi n={by("planned")} label="planned" sub="jobs written" />
        <Kpi n={by("in_progress")} label="in progress" tone={by("in_progress") ? "ok" : "neutral"} sub="jobs running" />
        <Kpi n={by("review")} label="review" tone={by("review") ? "warn" : "neutral"} sub="lead reports back" />
        <Kpi n={by("done")} label="done" />
        <Kpi n={by("cancelled")} label="cancelled" />
        <Kpi n={all.filter((x) => !x.lead && x.status !== "done" && x.status !== "cancelled").length} label="no lead yet" sub="runner wakes one" />
        <Kpi n={all.length} label="all time" />
      </KpiGrid>
      <Toolbar>
        <Select value={project || "__all"} onValueChange={(v) => setParams({ project: v === "__all" ? null : v })}>
          <SelectTrigger aria-label="Project" className="h-8 w-44">
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
        <Select value={status} onValueChange={(v) => setParams({ status: v === "active" ? null : v })}>
          <SelectTrigger aria-label="Status" className="h-8 w-40">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="active">active</SelectItem>
            <SelectItem value="all">all</SelectItem>
            {TASK_STATUSES.map((s) => (
              <SelectItem key={s} value={s}>
                {s.replace("_", " ")}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Input aria-label="Filter tasks" placeholder="filter…" value={text} onChange={(e) => setText(e.target.value)} className="h-8 w-56" />
        <Count>{q.data ? plural(rows.length, "task") : ""}</Count>
      </Toolbar>
      {q.error && !q.data ? (
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      ) : !q.data ? (
        <SkeletonRows n={8} />
      ) : rows.length ? (
        <div className="rounded-lg border bg-card">
          <DataTable columns={TASK_COLUMNS(true)} rows={rows} rowKey={(t) => t.id} onRow={(t) => setParams({ task: t.id })} initialSort="updated" caption="Tasks" />
        </div>
      ) : (
        <EmptyState>{all.length ? "Nothing matches these filters." : <>No task yet. Click <b>New task</b>, pick the project, write the brief in Markdown - the runner wakes that project&apos;s lead, which plans jobs and answers in the task&apos;s conversation.</>}</EmptyState>
      )}
      <TaskSheet id={task} onClose={() => setParams({ task: null })} />
    </>
  );
}
