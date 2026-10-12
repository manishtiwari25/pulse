"use client";

import { Suspense, useCallback, useMemo } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { VoteIcon } from "lucide-react";
import { AmendmentCard, SuggestionCard } from "@/components/board/vote-cards";
import { EmptyState, ErrorNotice, PageTitle, SkeletonPage, SkeletonRows } from "@/components/pulse/panel";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { Amendment, Suggestion, Votes } from "@/lib/api";
import { expiresAt, num } from "@/lib/format";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// Votes: what the agents are deciding among themselves. Nothing here waits
// for the owner, so no card is marigold; the page is for reading along.

type Kind = "suggestions" | "amendments";
// The URL keeps the store's words (eligible, needs votes, rejected).
type StatusFilter = "" | "eligible" | "needs votes" | "rejected";
type Sort = "queue" | "quorum" | "expiry" | "newest";

const STATUSES: { value: StatusFilter; label: string }[] = [
  { value: "", label: "All" },
  { value: "eligible", label: "Agreed" },
  { value: "needs votes", label: "Needs votes" },
  { value: "rejected", label: "Turned down" },
];

const SORTS: { value: Sort; label: string }[] = [
  { value: "queue", label: "Next in line first" },
  { value: "quorum", label: "Closest to agreed" },
  { value: "expiry", label: "Closing soonest" },
  { value: "newest", label: "Newest first" },
];

export default function VotesPage() {
  return (
    <Suspense fallback={<SkeletonPage rows={3} />}>
      <VotesScreen />
    </Suspense>
  );
}

const validKind = (v: string | null): Kind => (v === "amendments" ? "amendments" : "suggestions");
const validStatus = (v: string | null): StatusFilter => (v === "eligible" || v === "needs votes" || v === "rejected" ? v : "");
const validSort = (v: string | null): Sort => (v === "quorum" || v === "expiry" || v === "newest" ? v : "queue");
const soonest = (d: Date | null) => d?.getTime() ?? Infinity;
const slugOf = (x: Suggestion | Amendment) => x.slug || "";
const keyOf = (s: StatusFilter) => s || "all";

// "No suggestion needs votes", "No rule change was turned down".
function noneText(status: StatusFilter, what: string): string {
  if (status === "eligible") return "No " + what + " is agreed";
  if (status === "needs votes") return "No " + what + " needs votes";
  if (status === "rejected") return "No " + what + " was turned down";
  return "No " + what;
}

const SUB = "Ideas and rule changes your agents are voting on. When enough of them agree, the runner plans the work. Nothing here needs you unless you want to weigh in.";

// Defaults stay out of the URL so /votes/ and the palette's link read the same.
function patchQuery(sp: URLSearchParams, pathname: string, patch: Record<string, string | null | undefined>): string {
  const next = new URLSearchParams(sp.toString());
  for (const [k, v] of Object.entries(patch)) {
    if (v === null || v === undefined || v === "") next.delete(k);
    else next.set(k, v);
  }
  const s = next.toString();
  return pathname + (s ? "?" + s : "");
}

function TabCount({ n, on }: { n: number; on?: boolean }) {
  return <span className={cn("tnum shrink-0 rounded-full bg-muted px-1.5 text-xs leading-5 text-muted-foreground", on && "bg-primary/10 text-[var(--primary-strong)]")}>{num(n)}</span>;
}

function VotesScreen() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const project = sp.get("project") || "";
  const status = validStatus(sp.get("status"));
  const kind = validKind(sp.get("kind"));
  const sort = validSort(sp.get("sort"));
  const q = useApi<Votes>("votes", { refresh: 30000 });
  const setParams = useCallback((patch: Record<string, string | null | undefined>) => router.push(patchQuery(sp, pathname, patch), { scroll: false }), [sp, router, pathname]);
  const allSuggestions = useMemo(() => q.data?.suggestions ?? [], [q.data]);
  const allAmendments = useMemo(() => q.data?.amendments ?? [], [q.data]);
  const projects = useMemo(() => Array.from(new Set([...allSuggestions, ...allAmendments].map(slugOf).filter(Boolean))).sort(), [allSuggestions, allAmendments]);
  const suggestionsScope = useMemo(() => (project ? allSuggestions.filter((s) => s.slug === project) : allSuggestions), [allSuggestions, project]);
  const amendmentsScope = useMemo(() => (project ? allAmendments.filter((a) => a.slug === project) : allAmendments), [allAmendments, project]);
  // The status filter and its counts follow the open tab.
  const scope: (Suggestion | Amendment)[] = kind === "amendments" ? amendmentsScope : suggestionsScope;
  const statusCount = (s: StatusFilter) => (s ? scope.filter((x) => x.status === s).length : scope.length);
  const suggestions = useMemo(() => {
    const rows = status ? suggestionsScope.filter((s) => s.status === status) : suggestionsScope;
    if (sort === "queue") return rows;
    const sorted = [...rows];
    if (sort === "quorum") sorted.sort((a, b) => Math.max(0, (a.quorum ?? 0) - a.voters) - Math.max(0, (b.quorum ?? 0) - b.voters) || b.score - a.score);
    else if (sort === "expiry") sorted.sort((a, b) => soonest(expiresAt(a)) - soonest(expiresAt(b)));
    else sorted.sort((a, b) => (a.date < b.date ? 1 : -1));
    return sorted;
  }, [suggestionsScope, status, sort]);
  const amendments = useMemo(() => (status ? amendmentsScope.filter((a) => a.status === status) : amendmentsScope), [amendmentsScope, status]);
  const nothingAnywhere = allSuggestions.length + allAmendments.length === 0;
  const filtered = !!project || !!status;
  const clear = filtered ? (
    <Button size="sm" variant="outline" onClick={() => setParams({ project: null, status: null })}>
      Show everything
    </Button>
  ) : null;
  const where = (project ? " in " + project : "") + ". Try another status or project.";

  const filters = (
    <div className="mb-5 flex min-w-0 flex-wrap items-center gap-2">
      <Tabs value={keyOf(status)} onValueChange={(v) => setParams({ status: v === "all" ? null : v })} className="scroll-strip min-w-0 max-w-full gap-0">
        <TabsList aria-label="Status" className="justify-start">
          {STATUSES.map((s) => (
            <TabsTrigger key={keyOf(s.value)} value={keyOf(s.value)} className="flex-none gap-1.5 px-2.5">
              {s.label}
              <TabCount n={statusCount(s.value)} on={status === s.value} />
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>
      <div className="flex w-full min-w-0 gap-2 sm:w-auto">
      <Select value={project || "__all"} onValueChange={(v) => setParams({ project: v === "__all" ? null : v })}>
        <SelectTrigger aria-label="Project" className="h-9 min-w-0 flex-1 sm:w-40 sm:flex-none">
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
      {kind === "suggestions" ? (
        <Select value={sort} onValueChange={(v) => setParams({ sort: v === "queue" ? null : v })}>
          <SelectTrigger aria-label="Sort" className="h-9 min-w-0 flex-1 sm:w-48 sm:flex-none">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {SORTS.map((s) => (
              <SelectItem key={s.value} value={s.value}>
                {s.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : null}
      </div>
    </div>
  );

  return (
    <Tabs value={kind} onValueChange={(v) => setParams({ kind: v === "suggestions" ? null : v })} className="min-w-0 gap-0">
      <PageTitle
        icon={VoteIcon}
        sub={SUB}
        bar={
          <TabsList variant="line" aria-label="What is being voted on" className="w-full justify-start">
            <TabsTrigger value="suggestions" className="flex-none gap-1.5">
              Suggestions {q.data ? <TabCount n={suggestionsScope.length} on={kind === "suggestions"} /> : null}
            </TabsTrigger>
            <TabsTrigger value="amendments" className="flex-none gap-1.5">
              <span title="Amendments to a project's constitution">Rule changes</span> {q.data ? <TabCount n={amendmentsScope.length} on={kind === "amendments"} /> : null}
            </TabsTrigger>
          </TabsList>
        }
      >
        Votes
      </PageTitle>
      {q.error && !q.data ? (
        <ErrorNotice error={q.error} onRetry={() => void q.reload()} />
      ) : !q.data ? (
        <SkeletonRows n={6} />
      ) : nothingAnywhere ? (
        <EmptyState title="Nothing is up for a vote" icon={<VoteIcon className="size-[18px]" />}>
          When an agent suggests work or a change to a project&apos;s rules, it shows up here while the other agents vote on it.
        </EmptyState>
      ) : (
        <>
          {filters}
          <TabsContent value="suggestions" className="min-w-0">
            {suggestions.length ? (
              <div className="grid min-w-0 gap-3 lg:grid-cols-2">
                {suggestions.map((s) => (
                  <SuggestionCard key={(s.slug || "") + s.id} s={s} quorum={s.quorum ?? 2} slug="" />
                ))}
              </div>
            ) : (
              <EmptyState title={filtered ? "No suggestion matches" : "No suggestions right now"} action={clear}>
                {filtered ? noneText(status, "suggestion") + where : "Agents post ideas for work on a project's board; they show up here while the others vote."}
              </EmptyState>
            )}
          </TabsContent>
          <TabsContent value="amendments" className="min-w-0">
            {amendments.length ? (
              <div className="grid min-w-0 gap-3 lg:grid-cols-2">
                {amendments.map((a) => (
                  <AmendmentCard key={(a.slug || "") + a.id} a={a} slug="" />
                ))}
              </div>
            ) : (
              <EmptyState title={filtered ? "No rule change matches" : "No rule changes right now"} action={clear}>
                {filtered ? noneText(status, "rule change") + where : "A rule change is an agent's proposal to edit a project's constitution, the rules self-improvement follows. None is open."}
              </EmptyState>
            )}
          </TabsContent>
        </>
      )}
    </Tabs>
  );
}
