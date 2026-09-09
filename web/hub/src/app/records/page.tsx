"use client";

import Link from "next/link";
import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { BookOpenIcon, InboxIcon } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { DataTable } from "@/components/pulse/data-table";
import { Count, EmptyState, ErrorNotice, PageTitle, SkeletonPage, SkeletonRows } from "@/components/pulse/panel";
import { useViewMode, ViewToggle } from "@/components/pulse/view-toggle";
import { Mono } from "@/components/pulse/who";
import { RECORD_COLUMNS, RecordCards, RecordSheet, kindLabel } from "@/components/records/records";
import { registeredProjects, RECORD_KINDS, type Org, type RecordsPayload } from "@/lib/api";
import { plural } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

export default function RecordsPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={3} />}>
      <RecordsScreen />
    </Suspense>
  );
}

function RecordsScreen() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const kind = sp.get("kind") || "";
  const status = sp.get("status") || "";
  const project = sp.get("project") || "";
  const id = sp.get("id");
  // The framework's own docs (workflows, skills, templates…) are indexed too;
  // they stay behind one toggle so the shelf opens on the records that are yours.
  const showFramework = sp.get("fw") === "1" || (!!kind && !OWNER_KINDS.includes(kind));
  const [text, setText] = useState("");
  const [area, setArea] = useState("");
  const [view, setView] = useViewMode("records");
  const chipsRef = useRef<HTMLDivElement>(null);

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

  const q = useApi<RecordsPayload>("records" + (project ? "?project=" + encodeURIComponent(project) : ""), { refresh: 60000 });
  const org = useApi<Org>("org", { refresh: 60000 });
  // The decisions that wait for the owner (ADR-028); the shell asks for the same path, so this is the cached answer.
  const proposed = useApi<RecordsPayload>("records?kind=adr&status=Proposed", { refresh: 60000 });
  const waiting = (proposed.data?.records ?? []).filter((r) => r.kind === "adr" && r.status === "Proposed").length;
  const showingWaiting = kind === "adr" && status === "Proposed" && !project;
  const projects = useMemo(() => (org.data ? registeredProjects(org.data).map((p) => p.slug) : project ? [project] : []), [org.data, project]);
  const all = useMemo(() => q.data?.records ?? [], [q.data]);
  const scope = useMemo(() => (showFramework ? all : all.filter((r) => OWNER_KINDS.includes(r.kind))), [all, showFramework]);
  const statuses = useMemo(() => Array.from(new Set(scope.map((r) => r.status).filter(Boolean))).sort(), [scope]);
  const areas = useMemo(() => Array.from(new Set(scope.flatMap((r) => r.areas || []))).sort(), [scope]);
  // Every kind the store holds, in the canonical order, plus the one in the
  // URL so a deep link to an empty shelf still shows which chip is selected.
  const kinds = useMemo(() => {
    const present = new Set<string>(all.map((r) => r.kind));
    if (kind) present.add(kind);
    const known: string[] = RECORD_KINDS;
    return known.filter((k) => present.has(k)).concat(Array.from(present).filter((k) => !known.includes(k)));
  }, [all, kind]);
  const ownerKinds = useMemo(() => kinds.filter((k) => OWNER_KINDS.includes(k)), [kinds]);
  const frameworkKinds = useMemo(() => kinds.filter((k) => !OWNER_KINDS.includes(k)), [kinds]);
  // Everything but the kind filter, so each chip can count what it would show.
  const baseAll = useMemo(() => {
    const t = text.trim().toLowerCase();
    return all.filter((r) => (!status || r.status === status) && (!area || (r.areas || []).includes(area)) && (!t || r.id.toLowerCase().includes(t) || r.title.toLowerCase().includes(t) || (r.tags || []).some((x) => x.toLowerCase().includes(t))));
  }, [all, status, area, text]);
  const base = useMemo(() => (showFramework ? baseAll : baseAll.filter((r) => OWNER_KINDS.includes(r.kind))), [baseAll, showFramework]);
  const rows = useMemo(() => (kind ? base.filter((r) => r.kind === kind) : base), [base, kind]);
  const perKind = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of baseAll) m.set(r.kind, (m.get(r.kind) || 0) + 1);
    return m;
  }, [baseAll]);
  const frameworkCount = useMemo(() => baseAll.filter((r) => !OWNER_KINDS.includes(r.kind)).length, [baseAll]);
  const filtered = !!(kind || status || project || area || text || showFramework);

  // Keep the selected chip in view when the row scrolls (phones, deep links).
  useEffect(() => {
    chipsRef.current?.querySelector<HTMLElement>('[aria-pressed="true"]')?.scrollIntoView({ inline: "nearest", block: "nearest" });
  }, [kind, kinds]);

  return (
    <>
      <PageTitle
        icon={BookOpenIcon}
        sub="The decisions, plans, work units, features, memory and evolution records of your projects, with their revision history. The framework's own docs (workflows, skills, guides, templates…) are one toggle away."
        actions={
          <>
            <Input aria-label="Search records" placeholder="id, title or tag…" value={text} onChange={(e) => setText(e.target.value)} className="h-8 w-36 sm:w-56" />
            <ViewToggle value={view} onChange={setView} />
          </>
        }
        bar={
          <div className="grid gap-1">
            <BarRow ref={chipsRef} role="group" aria-label="Kind" className="w-fit gap-0.5 rounded-lg border bg-muted/40 p-0.5">
              <KindChip active={!kind} n={q.data ? base.length : undefined} onClick={() => setParams({ kind: null })}>
                All
              </KindChip>
              {ownerKinds.map((k) => (
                <KindChip key={k} active={kind === k} n={q.data ? perKind.get(k) || 0 : undefined} onClick={() => setParams({ kind: kind === k ? null : k })}>
                  {kindLabel(k)}
                </KindChip>
              ))}
              {frameworkKinds.length ? (
                <>
                  <span aria-hidden className="mx-0.5 h-4 w-px shrink-0 bg-border" />
                  <KindChip
                    active={showFramework}
                    n={q.data ? frameworkCount : undefined}
                    title={showFramework ? "Hide the framework's own docs" : "Show the framework's own docs: workflows, skills, guides, templates, prompts…"}
                    onClick={() => setParams(showFramework ? { fw: null, kind: kind && !OWNER_KINDS.includes(kind) ? null : kind } : { fw: "1" })}
                  >
                    Framework docs
                  </KindChip>
                  {showFramework
                    ? frameworkKinds.map((k) => (
                        <KindChip key={k} active={kind === k} n={q.data ? perKind.get(k) || 0 : undefined} onClick={() => setParams({ kind: kind === k ? null : k })}>
                          {kindLabel(k)}
                        </KindChip>
                      ))
                    : null}
                </>
              ) : null}
            </BarRow>
            <BarRow>
              <Select value={project || "__all"} onValueChange={(v) => setParams({ project: v === "__all" ? null : v })}>
                <SelectTrigger aria-label="Project" className={SELECT}>
                  <SelectValue placeholder="project" />
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
              <Select value={status || "__all"} onValueChange={(v) => setParams({ status: v === "__all" ? null : v })}>
                <SelectTrigger aria-label="Status" className={SELECT}>
                  <SelectValue placeholder="status" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="__all">all statuses</SelectItem>
                  {statuses.map((s) => (
                    <SelectItem key={s} value={s}>
                      {s}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {areas.length ? (
                <Select value={area || "__all"} onValueChange={(v) => setArea(v === "__all" ? "" : v)}>
                  <SelectTrigger aria-label="Area" className={SELECT}>
                    <SelectValue placeholder="area" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="__all">all areas</SelectItem>
                    {areas.map((a) => (
                      <SelectItem key={a} value={a}>
                        {a}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : null}
              {filtered ? (
                <Button
                  size="sm"
                  variant="ghost"
                  className="shrink-0"
                  onClick={() => {
                    setText("");
                    setArea("");
                    setParams({ kind: null, status: null, project: null, fw: null });
                  }}
                >
                  clear
                </Button>
              ) : null}
              <span className="ml-auto shrink-0 whitespace-nowrap">
                <Count>{q.data ? plural(rows.length, "record") : ""}</Count>
              </span>
            </BarRow>
          </div>
        }
      >
        Records
      </PageTitle>
      {waiting && !showingWaiting ? (
        <div className="surface-tint mb-3 flex flex-wrap items-center gap-3 rounded-lg border border-primary/30 px-3.5 py-2.5 text-sm">
          <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary" aria-hidden>
            <InboxIcon className="size-4" />
          </span>
          <span className="min-w-0 flex-1 anywhere">{plural(waiting, "proposed decision") + (waiting === 1 ? " waits" : " wait") + " for you."}</span>
          <Button size="xs" onClick={() => setParams({ kind: "adr", status: "Proposed", project: null })}>
            Show them here
          </Button>
          <Button asChild size="xs" variant="outline">
            <Link href={hrefs.home() + "#for-you"}>Decide on the hub</Link>
          </Button>
        </div>
      ) : null}
      {q.error && !q.data ? (
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      ) : !q.data ? (
        <SkeletonRows n={10} />
      ) : !rows.length ? (
        <EmptyState
          title={all.length ? "No matching records" : "No records indexed yet"}
          action={
            filtered ? (
              <Button
                size="xs"
                variant="outline"
                onClick={() => {
                  setText("");
                  setArea("");
                  setParams({ kind: null, status: null, project: null, fw: null });
                }}
              >
                clear filters
              </Button>
            ) : undefined
          }
        >
          {all.length ? "Nothing matches these filters." : <>Run <Mono>pulse init</Mono> to index docs/ (ADRs, plans, work units, memory…); agents add records as they work.</>}
        </EmptyState>
      ) : view === "table" ? (
        <div className="overflow-hidden rounded-lg border bg-card">
          <DataTable columns={RECORD_COLUMNS(!project)} rows={rows} rowKey={(r) => r.id} onRow={(r) => setParams({ id: r.id })} initialSort="updated" caption="Records" />
        </div>
      ) : (
        <RecordCards rows={rows} showProject={!project} onOpen={(r) => setParams({ id: r.id })} activeId={id} cols={3} />
      )}
      <RecordSheet id={id} project={project || undefined} onClose={() => setParams({ id: null })} />
    </>
  );
}

const SELECT = "h-8 w-32 shrink-0 sm:w-36";

// The kinds an owner writes or decides on; every other kind is the framework's own documentation.
const OWNER_KINDS: string[] = ["adr", "plan", "wu", "feature", "memory", "evolution"];

// One row of the sticky filter bar: scrolls sideways on its own (hidden
// scrollbar) so reaching the last filter never drags the other row along.
function BarRow({ className, children, ...rest }: React.ComponentProps<"div">) {
  return (
    <div className={cn("-mx-3 flex flex-wrap items-center gap-2 overflow-x-auto px-3 py-0.5 [scrollbar-width:none] sm:-mx-4 sm:px-4 [&::-webkit-scrollbar]:hidden", className)} {...rest}>
      {children}
    </div>
  );
}

// A shelf of the library: one kind and how many records it holds under the
// other filters. The selected chip is filled; an empty shelf is dimmed.
function KindChip({ active, n, title, onClick, children }: { active: boolean; n?: number; title?: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={active}
      title={title}
      onClick={onClick}
      className={cn(
        "inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2.5 text-xs font-medium whitespace-nowrap transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset",
        active ? "bg-card text-foreground shadow-card" : "text-muted-foreground hover:text-foreground",
        !active && n === 0 && "opacity-50",
      )}
    >
      {children}
      {n !== undefined ? <span className={cn("tnum rounded-full bg-muted px-1.5 text-[10.5px]", active ? "bg-primary/10 text-primary" : "text-muted-foreground")}>{n}</span> : null}
    </button>
  );
}
