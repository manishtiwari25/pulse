"use client";

import Link from "next/link";
import { Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { BookOpenIcon, ScaleIcon, SearchIcon, XIcon } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { DataTable } from "@/components/pulse/data-table";
import { EmptyState, ErrorNotice, PageTitle, SkeletonPage, SkeletonRows } from "@/components/pulse/panel";
import { useViewMode, ViewToggle } from "@/components/pulse/view-toggle";
import { RECORD_COLUMNS, RecordCards, RecordSheet, TIER_KINDS, kindLabel } from "@/components/records/records";
import { registeredProjects, RECORD_KINDS, type Org, type RecordsPayload } from "@/lib/api";
import { plural, sentence } from "@/lib/format";
import { statusWord } from "@/lib/words";
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
  const statusKey = status.toLowerCase();
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
  // One entry per status word: the store may hold "Active" and "active".
  const statuses = useMemo(() => {
    const m = new Map<string, string>();
    for (const r of scope) if (r.status && !m.has(r.status.toLowerCase())) m.set(r.status.toLowerCase(), r.status);
    return Array.from(m.values()).sort((a, b) => statusWord(a).localeCompare(statusWord(b)));
  }, [scope]);
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
    return all.filter((r) => (!statusKey || (r.status || "").toLowerCase() === statusKey) && (!area || (r.areas || []).includes(area)) && (!t || r.id.toLowerCase().includes(t) || r.title.toLowerCase().includes(t) || (r.tags || []).some((x) => x.toLowerCase().includes(t))));
  }, [all, statusKey, area, text]);
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

  const clearAll = () => {
    setText("");
    setArea("");
    setParams({ kind: null, status: null, project: null, fw: null });
  };

  return (
    <>
      <PageTitle
        icon={BookOpenIcon}
        sub="Decisions, plans, guides and notes your agents keep. Agents write them; you accept or decline the decisions."
        bar={
          <div className="grid gap-3">
            <BarRow ref={chipsRef} role="group" aria-label="Kind of record" className="scroll-strip mx-0 w-fit max-w-full flex-nowrap gap-0.5 rounded-lg bg-muted p-0.5 ring-1 ring-border/70 ring-inset sm:mx-0 sm:px-0.5">
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
                  <span aria-hidden className="mx-1 h-4 w-px shrink-0 bg-border" />
                  <KindChip
                    active={showFramework}
                    n={q.data ? frameworkCount : undefined}
                    title={showFramework ? "Hide the guides PULSE comes with" : "Show the guides PULSE comes with: workflows, skills, prompts, templates and more"}
                    onClick={() => setParams(showFramework ? { fw: null, kind: kind && !OWNER_KINDS.includes(kind) ? null : kind } : { fw: "1" })}
                  >
                    Built-in guides
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
              <div className="relative w-full shrink-0 sm:w-60">
                <SearchIcon className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden />
                <Input type="search" aria-label="Search records" placeholder="Search records" value={text} onChange={(e) => setText(e.target.value)} className="h-9 pl-8" />
              </div>
              <Select value={project || "__all"} onValueChange={(v) => setParams({ project: v === "__all" ? null : v })}>
                <SelectTrigger aria-label="Project" className={SELECT}>
                  <SelectValue placeholder="Project" />
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
              <Select value={statusKey || "__all"} onValueChange={(v) => setParams({ status: v === "__all" ? null : statuses.find((s) => s.toLowerCase() === v) || v })}>
                <SelectTrigger aria-label="Status" className={SELECT}>
                  <SelectValue placeholder="Status" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="__all">All statuses</SelectItem>
                  {statuses.map((s) => (
                    <SelectItem key={s} value={s.toLowerCase()}>
                      {statusWord(s)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {areas.length ? (
                <Select value={area || "__all"} onValueChange={(v) => setArea(v === "__all" ? "" : v)}>
                  <SelectTrigger aria-label="Area" className={SELECT}>
                    <SelectValue placeholder="Area" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="__all">All areas</SelectItem>
                    {areas.map((a) => (
                      <SelectItem key={a} value={a}>
                        {sentence(a)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : null}
              {filtered ? (
                <Button size="sm" variant="ghost" className="shrink-0 text-muted-foreground" onClick={clearAll}>
                  <XIcon /> Clear filters
                </Button>
              ) : null}
              <ViewToggle value={view} onChange={setView} className="ml-auto" />
            </BarRow>
          </div>
        }
      >
        Records
      </PageTitle>
      {waiting && !showingWaiting ? (
        <div className="surface-you mb-5 flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border px-4 py-3 text-sm">
          <ScaleIcon className="size-4 shrink-0 text-you-ink" aria-hidden />
          <span className="min-w-0 flex-1 basis-56 font-medium anywhere">{plural(waiting, "decision") + (waiting === 1 ? " waits" : " wait") + " for your answer."}</span>
          <Link href={hrefs.home() + "#for-you"} className="text-[13px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline">
            Answer on Home
          </Link>
          <Button size="sm" onClick={() => setParams({ kind: "adr", status: "Proposed", project: null })}>
            Show them
          </Button>
        </div>
      ) : null}
      {q.error && !q.data ? (
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      ) : !q.data ? (
        <SkeletonRows n={10} />
      ) : !rows.length ? (
        <EmptyState
          icon={<BookOpenIcon className="size-[18px]" />}
          title={all.length ? "No records match" : "No records yet"}
          action={
            filtered ? (
              <Button size="sm" variant="outline" onClick={clearAll}>
                Clear filters
              </Button>
            ) : undefined
          }
        >
          {all.length ? "Try another kind or status, or clear the filters to see every record." : "Agents write decisions, plans and notes here once a project is set up and work starts."}
        </EmptyState>
      ) : (
        <>
          <p className="mb-3 text-[13px] text-muted-foreground" aria-live="polite">
            {showingWaiting ? plural(rows.length, "decision") + " waiting for your answer" : "Showing " + plural(rows.length, "record")}
          </p>
          {view === "table" ? (
            <div className="overflow-hidden rounded-2xl border bg-card">
              <DataTable columns={RECORD_COLUMNS(!project, !kind || TIER_KINDS.includes(kind))} rows={rows} rowKey={(r) => r.project + "/" + r.id} onRow={(r) => setParams({ id: r.id })} initialSort="updated" caption="Records" />
            </div>
          ) : (
            <RecordCards rows={rows} showProject={!project} onOpen={(r) => setParams({ id: r.id })} activeId={id} cols={3} />
          )}
        </>
      )}
      <RecordSheet id={id} project={project || undefined} onClose={() => setParams({ id: null })} />
    </>
  );
}

const SELECT = "h-9 w-[calc(50%-0.25rem)] shrink-0 sm:w-40";

// The kinds an owner writes or decides on; every other kind is the framework's own documentation.
const OWNER_KINDS: string[] = ["adr", "plan", "wu", "feature", "memory", "evolution"];

// One row of the filter bar. The kind chips add scroll-strip, so they scroll
// sideways on their own; the controls row wraps instead.
function BarRow({ className, children, ...rest }: React.ComponentProps<"div">) {
  return (
    <div className={cn("-mx-3 flex flex-wrap items-center gap-2 px-3 py-0.5 sm:-mx-4 sm:px-4", className)} {...rest}>
      {children}
    </div>
  );
}

// A shelf of the library: one kind and how many records it holds under the
// other filters, styled like a segmented tab. An empty shelf is dimmed.
function KindChip({ active, n, title, onClick, children }: { active: boolean; n?: number; title?: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={active}
      title={title}
      onClick={onClick}
      className={cn(
        "inline-flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-transparent px-3 text-sm font-medium whitespace-nowrap transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset motion-reduce:transition-none",
        active ? "border-border bg-card text-foreground shadow-[0_1px_2px_oklch(0.26_0.04_262/.08)]" : "text-muted-foreground hover:text-foreground",
        !active && n === 0 && "opacity-60",
      )}
    >
      {children}
      {n !== undefined ? <span className={cn("tnum text-xs font-normal", active ? "text-primary" : "text-muted-foreground")}>{n}</span> : null}
    </button>
  );
}
