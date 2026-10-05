"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import type { Column } from "@/components/pulse/data-table";
import { CardGrid, ItemCard, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Markdown } from "@/components/pulse/markdown";
import { EmptyState, ErrorNotice, Kv, Panel, SkeletonRows } from "@/components/pulse/panel";
import { Mono, ProjectLink, StatusBadge, ToneBadge, When, Who } from "@/components/pulse/who";
import { awaitsDecision, RecordDecisionButtons } from "@/components/records/decide-record";
import { hrefs } from "@/lib/nav";
import { recordEvidence } from "@/lib/record-evidence";
import { enc, type RecordDetail, type RecordRow } from "@/lib/api";
import { useApi } from "@/lib/use-api";
import { fmtDate, num, type Tone } from "@/lib/format";

const KIND_LABEL: Record<string, string> = {
  adr: "ADR",
  plan: "Plan",
  wu: "Work unit",
  feature: "Feature",
  memory: "Memory",
  workflow: "Workflow",
  skill: "Skill",
  guide: "Guide",
  context: "Context",
  architecture: "Architecture",
  evolution: "Evolution",
  instruction: "Instruction",
  template: "Template",
  usage: "Usage",
  prompt: "Prompt",
};

const KIND_TONE: Record<string, Tone> = { adr: "accent", plan: "info", wu: "info", feature: "ok", memory: "warn", evolution: "bad" };

export const kindLabel = (k: string) => KIND_LABEL[k] || k;

export function RecordKindBadge({ kind }: { kind: string }) {
  return <ToneBadge tone={KIND_TONE[kind] || "neutral"}>{kindLabel(kind)}</ToneBadge>;
}

export const RECORD_COLUMNS = (showProject: boolean): Column<RecordRow>[] => [
  { key: "id", label: "id", nowrap: true, sort: (r) => r.kind + (r.number ?? 0).toString().padStart(5, "0") + r.slug, render: (r) => <Link href={hrefs.record(r.id, r.project)} className="font-mono text-[12.5px] hover:underline">{r.id}</Link> },
  { key: "kind", label: "kind", sort: (r) => r.kind, render: (r) => <RecordKindBadge kind={r.kind} /> },
  { key: "title", label: "title", sort: (r) => r.title, render: (r) => <Link href={hrefs.record(r.id, r.project)} className="anywhere hover:underline">{r.title}</Link> },
  { key: "status", label: "status", sort: (r) => r.status, render: (r) => <StatusBadge status={r.status} /> },
  ...(showProject ? ([{ key: "project", label: "project", sort: (r) => r.project, render: (r) => <ProjectLink slug={r.project} /> }] as Column<RecordRow>[]) : []),
  { key: "tier", label: "tier", align: "right", hide: "md", sort: (r) => r.tier ?? -1, render: (r) => (r.tier === null || r.tier === undefined ? "" : <Mono>{"T" + r.tier}</Mono>) },
  { key: "areas", label: "areas", hide: "lg", render: (r) => <span className="text-xs text-muted-foreground">{(r.areas || []).join(", ")}</span> },
  { key: "delivery", label: "delivery", hide: "lg", sort: (r) => r.delivery || "", render: (r) => (r.delivery ? <ToneBadge tone={r.delivered ? "ok" : "neutral"}>{r.delivery + (r.delivered ? " · " + r.delivered : "")}</ToneBadge> : "") },
  { key: "updated", label: "updated", nowrap: true, sort: (r) => r.updated_at || r.date, render: (r) => <When d={r.updated_at || r.date} /> },
  { key: "rev", label: "rev", align: "right", hide: "md", sort: (r) => r.current_rev, render: (r) => "r" + r.current_rev },
];

const MAX_TAGS = 4;

// One record as a library card: what it is (kind, status, tier), its id and
// title, and the small facts a reader scans for (areas, tags, delivery).
// Opens the record on click; a plain link when no onOpen is given.
export function RecordCard({ r, showProject, onOpen, active }: { r: RecordRow; showProject?: boolean; onOpen?: (r: RecordRow) => void; active?: boolean }) {
  const open = onOpen ? () => onOpen(r) : undefined;
  const areas = r.areas || [];
  const tags = r.tags || [];
  return (
    <ItemCard onOpen={open} href={open ? undefined : hrefs.record(r.id, r.project)} active={active} ariaLabel={r.id + " " + r.title}>
      <ItemCardHeader
        right={
          <>
            <When d={r.updated_at || r.date} />
            <span className="tnum">{"r" + r.current_rev}</span>
          </>
        }
      >
        <RecordKindBadge kind={r.kind} />
        <StatusBadge status={r.status} />
        {r.tier !== null && r.tier !== undefined ? <ToneBadge tone="neutral"><Mono>{"T" + r.tier}</Mono></ToneBadge> : null}
        {showProject ? (
          // A link card cannot nest another link, so the project is plain text there.
          open ? <ProjectLink slug={r.project} className="text-xs" /> : <Mono className="text-xs">{r.project}</Mono>
        ) : null}
      </ItemCardHeader>
      <ItemCardTitle chevron={!!open}>
        <Mono className="mr-2 text-muted-foreground">{r.id}</Mono>
        {r.title}
      </ItemCardTitle>
      {areas.length || tags.length || r.delivery ? (
        <ItemCardMeta>
          {areas.length ? <span>{areas.join(", ")}</span> : null}
          {tags.length ? (
            <span className="flex flex-wrap items-center gap-1">
              {tags.slice(0, MAX_TAGS).map((t) => (
                <ToneBadge key={t} tone="neutral" className="h-4 px-1 text-[10.5px] font-normal">
                  {t}
                </ToneBadge>
              ))}
              {tags.length > MAX_TAGS ? <span className="tnum">{"+" + (tags.length - MAX_TAGS)}</span> : null}
            </span>
          ) : null}
          {r.delivery ? <ToneBadge tone={r.delivered ? "ok" : "neutral"}>{(r.delivered ? "delivered" : r.delivery) + (r.delivered ? " · " + r.delivered : "")}</ToneBadge> : null}
        </ItemCardMeta>
      ) : null}
    </ItemCard>
  );
}

export function RecordCards({ rows, showProject, onOpen, activeId, cols = 3 }: { rows: RecordRow[]; showProject?: boolean; onOpen?: (r: RecordRow) => void; activeId?: string | null; cols?: 1 | 2 | 3 | 4 }) {
  if (!rows.length) return null;
  return (
    <CardGrid cols={cols}>
      {rows.map((r) => (
        <RecordCard key={r.project + "/" + r.id} r={r} showProject={showProject} onOpen={onOpen} active={activeId === r.id} />
      ))}
    </CardGrid>
  );
}

export function RecordSheet({ id, project, onClose }: { id: string | null; project?: string; onClose: () => void }) {
  return (
    <Sheet open={!!id} onOpenChange={(o) => !o && onClose()}>
      {/* The Sheet sizes itself with data-[side] variants, so the override must use the same ones. */}
      <SheetContent side="right" className="scroll-thin overflow-y-auto rounded-xl data-[side=right]:w-full data-[side=right]:sm:max-w-4xl">
        {id ? <RecordBody id={id} project={project} /> : null}
      </SheetContent>
    </Sheet>
  );
}

export function RecordBody({ id, project }: { id: string; project?: string }) {
  const [rev, setRev] = useState<string>("");
  const params = [project ? "project=" + enc(project) : "", rev ? "rev=" + enc(rev) : ""].filter(Boolean).join("&");
  const { data, error, reload } = useApi<RecordDetail>("records/" + enc(id) + (params ? "?" + params : ""));
  const revisions = useMemo(() => [...(data?.revisions || [])].sort((a, b) => b.rev - a.rev), [data]);
  if (error && !data)
    return (
      <div>
        <SheetHeader>
          <SheetTitle className="font-mono text-base">{id}</SheetTitle>
          <SheetDescription>Record</SheetDescription>
        </SheetHeader>
        <div className="px-4">
          <ErrorNotice error={error} onRetry={() => void reload()} />
        </div>
      </div>
    );
  if (!data)
    return (
      <div>
        <SheetHeader>
          <SheetTitle className="font-mono text-base">{id}</SheetTitle>
          <SheetDescription>Loading…</SheetDescription>
        </SheetHeader>
        <div className="px-4">
          <SkeletonRows n={8} />
        </div>
      </div>
    );
  const r = data.record;
  const fm = data.frontmatter || {};
  const shownRev = rev ? Number(rev) : r.current_rev;
  const areas = r.areas || [];
  const author = r.created_by || (typeof fm.author === "string" ? fm.author : "");
  const evidence = recordEvidence(r.project, data.body || "", fm);
  return (
    <>
      <SheetHeader className="gap-2 border-b">
        <div className="flex flex-wrap items-center gap-1.5 pr-8 text-xs">
          <RecordKindBadge kind={r.kind} />
          <StatusBadge status={r.status} />
          {r.tier !== null && r.tier !== undefined ? <ToneBadge tone="neutral"><Mono>{"T" + r.tier}</Mono></ToneBadge> : null}
          {r.delivery ? <ToneBadge tone={r.delivered ? "ok" : "neutral"}>{"delivery: " + r.delivery + (r.delivered ? " · " + r.delivered : "")}</ToneBadge> : null}
        </div>
        <Mono className="text-muted-foreground">{r.id}</Mono>
        <SheetTitle className="pr-8 text-lg font-semibold leading-snug anywhere">
          {r.title}
        </SheetTitle>
        <SheetDescription className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px]">
          <ProjectLink slug={r.project} />
          {r.tier !== null && r.tier !== undefined ? <span>{"Tier " + r.tier}</span> : null}
          {areas.length ? <span>{areas.join(", ")}</span> : null}
          {author ? <Who row={{ author }} compact /> : null}
          <span>
            {"dated "}
            <When d={r.date} exact />
          </span>
          {r.updated_at ? (
            <span>
              {"updated "}
              <When d={r.updated_at} />
            </span>
          ) : null}
          <Mono className="text-muted-foreground">{r.path}</Mono>
        </SheetDescription>
      </SheetHeader>
      <div className="grid gap-3 px-4 pb-4">
        {awaitsDecision(r) ? (
          <div className="surface-tint sticky top-0 z-10 flex flex-wrap items-center gap-2 rounded-lg border border-primary/30 p-3 shadow-card">
            <span className="min-w-0 flex-1 text-sm font-medium anywhere">Waiting for your yes, defer or no</span>
            <RecordDecisionButtons record={r} size="xs" />
          </div>
        ) : null}
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="text-xs text-muted-foreground">
            {shownRev === r.current_rev ? "Current revision r" + r.current_rev : "Showing revision r" + shownRev + " (current is r" + r.current_rev + ")"}
            {" · " + num(revisions.length) + " in history"}
          </div>
          {revisions.length > 1 ? (
            <div className="flex items-center gap-2">
              <Select value={rev || String(r.current_rev)} onValueChange={(v) => setRev(Number(v) === r.current_rev ? "" : v)}>
                <SelectTrigger aria-label="Revision" className="h-7 w-56 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {revisions.map((x) => (
                    <SelectItem key={x.rev} value={String(x.rev)}>
                      {"r" + x.rev + " · " + fmtDate(x.at).slice(0, 16) + (x.author ? " · " + x.author : "")}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {rev ? (
                <Button size="xs" variant="ghost" onClick={() => setRev("")}>
                  Latest
                </Button>
              ) : null}
            </div>
          ) : null}
        </div>
        <article className="rounded-lg border bg-card p-4 shadow-card">
          <Markdown source={data.body || ""} />
        </article>
        {evidence.length ? (
          <Panel title="Evidence">
            <p className="mb-2 text-xs text-muted-foreground">Jobs and board posts this record names: open the thread or post here instead of running `pulse record show` and searching the board.</p>
            <ul className="flex flex-wrap gap-2">
              {evidence.map((e) => (
                <li key={e.kind + e.id}>
                  <Link href={e.href} className="inline-flex items-center gap-1 rounded-md border px-2 py-1 text-xs hover:bg-accent" title={e.kind === "job" ? "Open the job's board thread" : "Open the board post"}>
                    <span className="text-muted-foreground">{e.kind}</span>
                    <Mono className="anywhere">{e.id}</Mono>
                  </Link>
                </li>
              ))}
            </ul>
          </Panel>
        ) : null}
        <Panel title="Frontmatter">
          <Kv rows={Object.entries(fm).map(([k, v]) => [k, <Mono key={k} className="anywhere">{Array.isArray(v) ? v.join(", ") : typeof v === "object" && v !== null ? JSON.stringify(v) : String(v)}</Mono>])} />
        </Panel>
        <Panel title="Revisions">
          {revisions.length ? (
            <ol className="ml-2 grid gap-0 border-l text-[12.5px]">
              {revisions.map((x) => (
                <li key={x.rev} className="relative grid gap-1 pb-3 pl-4 last:pb-0">
                  <span className="absolute -left-[4.5px] top-1.5 size-2 rounded-full bg-primary ring-2 ring-card" aria-hidden />
                  <div className="flex flex-wrap items-center gap-2">
                    <button type="button" className={"rounded-sm font-mono hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring " + (x.rev === shownRev ? "font-semibold text-foreground" : "text-muted-foreground")} onClick={() => setRev(x.rev === r.current_rev ? "" : String(x.rev))}>
                      {"r" + x.rev}
                    </button>
                    {x.author ? <Who row={{ author: x.author }} compact /> : null}
                    <When d={x.at} className="text-muted-foreground" />
                  </div>
                  <span className="text-muted-foreground anywhere">{x.note}</span>
                </li>
              ))}
            </ol>
          ) : (
            <EmptyState>No revision history yet - the store keeps one from the first edit through the hub or an agent.</EmptyState>
          )}
        </Panel>
      </div>
    </>
  );
}
