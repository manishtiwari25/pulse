"use client";

import { Suspense, useCallback, useMemo, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { ListTodoIcon, SearchIcon } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DataTable } from "@/components/pulse/data-table";
import { Count, EmptyState, ErrorNotice, PageTitle, SkeletonPage, SkeletonRows } from "@/components/pulse/panel";
import { useViewMode, ViewToggle } from "@/components/pulse/view-toggle";
import { TaskCards } from "@/components/tasks/task-card";
import { NewTaskButton } from "@/components/tasks/task-dialogs";
import { TASK_COLUMNS, TaskSheet } from "@/components/tasks/task-sheet";
import { registeredProjects, taskProgress, TASK_STATUSES, type Org, type Task, type TasksPayload } from "@/lib/api";
import { andList, plural } from "@/lib/format";
import { statusWord } from "@/lib/words";
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

// "active" (shown as "Open", the word the summary line and the project
// Work tab use) is the default and means no `status` param, so links from the
// palette and other pages (hrefs.tasks) keep working unchanged. What waits
// for the owner ("review", read "Needs you") comes right after it.
const STATUS_FILTERS: [string, string][] = [
  ["active", "Open"],
  ["review", statusWord("review")],
  ...TASK_STATUSES.filter((s) => s !== "review").map((s): [string, string] => [s, statusWord(s)]),
  ["all", "All"],
];

// The status filter as a segmented control with the count in each trigger.
// A status with no task in it stays out of the way unless it is the one
// selected; Open and All are always there.
function StatusFilter({ value, count, onChange }: { value: string; count: (s: string) => number; onChange: (s: string) => void }) {
  return (
    <Tabs value={value} onValueChange={onChange} className="min-w-0 max-w-full">
      <div className="scroll-strip min-w-0 max-w-full">
        <TabsList aria-label="Status">
          {STATUS_FILTERS.filter(([v]) => v === "active" || v === "all" || v === value || count(v) > 0).map(([v, label]) => (
            <TabsTrigger key={v} value={v} className="flex-none px-2.5">
              {label}
              <span className={cn("tnum min-w-4 rounded-full px-1.5 text-xs font-normal leading-5", v === "review" && count(v) ? "bg-you-soft text-you-ink" : "bg-background/70 text-muted-foreground")}>{count(v)}</span>
            </TabsTrigger>
          ))}
        </TabsList>
      </div>
    </Tabs>
  );
}

// One sentence on the list in view: how many are open and what needs you.
function summary(scope: Task[]): string {
  const active = scope.filter(isActive);
  if (!scope.length) return "";
  if (!active.length) return "No open task right now. Everything you gave the agents is done or cancelled.";
  const review = active.filter((t) => t.status === "review").length;
  const working = active.filter((t) => taskProgress(t).working.length > 0).length;
  const bits = [review ? (review === 1 ? "1 waits for your decision" : review + " wait for your decision") : "", working ? (working === 1 ? "agents are working on 1 right now" : "agents are working on " + working + " right now") : ""].filter(Boolean);
  return plural(active.length, "open task") + (bits.length ? ": " + andList(bits) + "." : ".");
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
  // Counts in the filter and the summary follow the project filter, so they
  // describe the list the owner is looking at rather than the whole org.
  const scope = useMemo(() => (project ? all.filter((x) => x.project === project) : all), [all, project]);
  const rows = useMemo(() => {
    const t = text.trim().toLowerCase();
    return scope.filter((x) => (status === "all" ? true : status === "active" ? isActive(x) : x.status === status) && (!t || x.title.toLowerCase().includes(t) || x.id.toLowerCase().includes(t) || (x.brief || "").toLowerCase().includes(t)));
  }, [scope, status, text]);
  const by = (s: string) => (s === "all" ? scope.length : s === "active" ? scope.filter(isActive).length : scope.filter((x) => x.status === s).length);
  const filtered = !!text.trim() || status !== "active";
  const empty = all.length ? (
    text.trim() ? (
      <>No task matches &ldquo;{text.trim()}&rdquo;. Try fewer words, or look in All.</>
    ) : (
      <>No task here with this filter. Pick another status, or All, to see the rest.</>
    )
  ) : (
    <>Nothing yet. Give the agents a task: pick the project and say what you want done. The project&apos;s lead agent splits it into steps and reports back in its chat.</>
  );
  const emptyTitle = all.length ? (filtered ? "No task matches" : "No open task") : "No tasks yet";
  const emptyAction = all.length && filtered ? undefined : <NewTaskButton project={project || undefined} onCreated={(t) => setParams({ task: t.id })} />;
  const line = q.data ? summary(scope) : "";
  return (
    <>
      <PageTitle
        icon={ListTodoIcon}
        sub="The work you gave your agents. Each task's lead agent splits it into steps and reports back in its chat."
        actions={<NewTaskButton size="default" project={project || undefined} onCreated={(t) => setParams({ task: t.id })} />}
        bar={
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <Select value={project || "__all"} onValueChange={(v) => setParams({ project: v === "__all" ? null : v })}>
              <SelectTrigger aria-label="Project" className="w-full sm:w-44">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__all">All projects</SelectItem>
                {projects.map((p) => (
                  <SelectItem key={p} value={p}>
                    {p}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <StatusFilter value={status} count={by} onChange={(v) => setParams({ status: v === "active" ? null : v })} />
            <span className="flex min-w-0 flex-1 basis-56 items-center gap-2">
              <span className="relative min-w-0 flex-1 lg:max-w-64">
                <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden />
                <Input type="search" aria-label="Search tasks" placeholder="Search tasks" value={text} onChange={(e) => setText(e.target.value)} className="pl-8" />
              </span>
              {/* How many match, once a search or a status other than Open narrows the list. */}
              {q.data && filtered ? (
                <span className="ml-auto flex shrink-0" aria-live="polite">
                  <Count>{plural(rows.length, "task")}</Count>
                </span>
              ) : null}
              <ViewToggle value={view} onChange={setView} className={cn("shrink-0", !(q.data && filtered) && "ml-auto")} />
            </span>
          </div>
        }
      >
        Tasks
      </PageTitle>
      {line ? (
        <p className="-mt-1 mb-4 max-w-3xl text-sm leading-relaxed text-muted-foreground" aria-live="polite">
          {line}
        </p>
      ) : null}
      {q.error && !q.data ? (
        <ErrorNotice error={q.error} onRetry={() => void q.reload()}>
          {"The task list didn't load (" + q.error + "). Check that PULSE is running, then try again."}
        </ErrorNotice>
      ) : !q.data ? (
        <SkeletonRows n={8} />
      ) : view === "table" ? (
        rows.length ? (
          <div className="overflow-hidden rounded-2xl border bg-card">
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
