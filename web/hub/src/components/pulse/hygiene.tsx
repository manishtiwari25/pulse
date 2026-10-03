"use client";

import Link from "next/link";
import { useEffect, useMemo, useRef, useState } from "react";
import { BrushCleaningIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { EmptyState, Kv, Panel } from "./panel";
import { Mono, ToneBadge, When, Who } from "./who";
import { invalidate, useApi, useLiveEvents } from "@/lib/use-api";
import { num, plural, type Tone } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import type { AgentCleanup, Hygiene, Leftover, StoreEvent, SweptItem } from "@/lib/api";

// The cleaning events the hygiene route serves; a live one of these refreshes the panel.
const CLEANING = new Set(["daemon.sweep", "worktree.swept", "land.swept", "agent.cleanup"]);

export function sizeText(n: number | undefined | null): string {
  const b = Number(n || 0);
  if (b >= 1 << 30) return (b / (1 << 30)).toFixed(1) + " GB";
  if (b >= 1 << 20) return (b / (1 << 20)).toFixed(1) + " MB";
  if (b >= 1 << 10) return Math.round(b / (1 << 10)) + " KB";
  return b + " B";
}

const countsText = (m: Record<string, number> | undefined | null): string =>
  Object.entries(m ?? {})
    .filter(([, n]) => n > 0)
    .map(([k, n]) => k + " " + num(n))
    .join(", ");

const countsOf = (items: SweptItem[]): Record<string, number> => items.reduce<Record<string, number>>((m, it) => ({ ...m, [it.kind]: (m[it.kind] ?? 0) + 1 }), {});

export function cleanupOf(e: StoreEvent): AgentCleanup | null {
  if (e.action !== "agent.cleanup" || !e.detail || typeof e.detail !== "object") return null;
  const d = e.detail as Partial<AgentCleanup>;
  return { job: d.job ?? "", project: d.project ?? "", executor: d.executor ?? "", status: d.status ?? "", clone_mb: Number(d.clone_mb ?? 0), left: Array.isArray(d.left) ? (d.left as Leftover[]) : [], text: d.text ?? "" };
}

// What one cleaning event did, in a line: the agent's own cleanup, a sweep,
// or one worktree or branch the daemon took from a project checkout.
export function cleaningText(e: StoreEvent): string {
  const c = cleanupOf(e);
  if (c) return c.text || (c.left.length ? plural(c.left.length, "worktree") + " left behind" : "nothing left behind");
  const d = (e.detail && typeof e.detail === "object" ? e.detail : {}) as Record<string, unknown>;
  if (e.action === "daemon.sweep") {
    const removed = countsText(d.removed as Record<string, number>);
    const held = Number(d.held ?? 0);
    const errors = Array.isArray(d.errors) ? d.errors.length : 0;
    return (removed ? "removed " + removed + " (" + sizeText(Number(d.bytes ?? 0)) + " freed)" : "nothing removed") + (held ? "; " + num(held) + " held" : "") + (errors ? "; " + plural(errors, "error") : "");
  }
  const why = typeof d.why === "string" ? d.why : "";
  const branch = typeof d.branch === "string" && d.branch ? "branch " + d.branch : "";
  const project = typeof d.project === "string" && d.project ? "in " + d.project : "";
  return [branch, project, why].filter(Boolean).join(" · ");
}

function cleaningTone(e: StoreEvent): Tone {
  const c = cleanupOf(e);
  if (c) return c.left.length ? (c.left.every((l) => l.removed) ? "warn" : "bad") : "ok";
  if (e.action === "daemon.sweep") {
    const d = e.detail as Record<string, unknown> | null;
    return d && Array.isArray(d.errors) && d.errors.length ? "bad" : "neutral";
  }
  return "neutral";
}

function cleaningLabel(e: StoreEvent): string {
  if (e.action === "agent.cleanup") return "cleaned up";
  if (e.action === "daemon.sweep") return "sweep";
  if (e.action === "worktree.swept") return "worktree swept";
  if (e.action === "land.swept") return "branch swept";
  return e.action;
}

function HeldList({ items }: { items: SweptItem[] }) {
  const [open, setOpen] = useState(false);
  if (!items.length) return null;
  const shown = open ? items : items.slice(0, 4);
  return (
    <div className="mt-3" aria-label="held by the sweep">
      <p className="text-xs font-medium text-warn">{"Held, not removed (" + num(items.length) + "): each waits for a reason"}</p>
      <ul className="mt-1 grid gap-1 text-xs">
        {shown.map((it, i) => (
          <li key={it.kind + it.path + i} className="min-w-0 rounded-md border border-warn/30 bg-warn/5 px-2.5 py-1.5">
            <div className="flex min-w-0 items-center gap-2">
              <ToneBadge tone="warn" className="shrink-0">
                {it.kind}
              </ToneBadge>
              {it.project ? <span className="shrink-0 text-muted-foreground">{it.project}</span> : null}
              <span className="min-w-0 flex-1 truncate" title={it.path}>
                <Mono className="whitespace-nowrap [overflow-wrap:normal]">{it.path}</Mono>
              </span>
              {it.bytes ? <span className="tnum shrink-0 text-muted-foreground">{sizeText(it.bytes)}</span> : null}
            </div>
            <div className="mt-0.5 text-muted-foreground anywhere">{it.why}</div>
          </li>
        ))}
      </ul>
      {items.length > 4 ? (
        <Button size="xs" variant="ghost" className="mt-1 text-muted-foreground" onClick={() => setOpen((v) => !v)}>
          {open ? "show fewer" : "show all " + num(items.length)}
        </Button>
      ) : null}
    </div>
  );
}

function LeftoverList({ left }: { left: Leftover[] }) {
  if (!left.length) return null;
  return (
    <ul className="mt-1 grid gap-1 text-[11.5px]">
      {left.map((l, i) => (
        <li key={l.path + i} className="min-w-0 text-muted-foreground">
          <div className="flex min-w-0 items-center gap-1.5">
            <span className={"size-1.5 shrink-0 rounded-full " + (l.removed ? "bg-warn" : "bg-bad")} aria-hidden />
            <span className="shrink-0">{l.kind}</span>
            <span className="min-w-0 flex-1 truncate" title={l.path}>
              <Mono className="whitespace-nowrap [overflow-wrap:normal]">{l.path}</Mono>
            </span>
            <span className="tnum shrink-0">{sizeText(l.bytes)}</span>
          </div>
          <div className={"pl-3 anywhere " + (l.removed ? "" : "text-bad")}>{(l.branch ? "on " + l.branch + " · " : "") + (l.removed ? "removed by the runner" : "could not be removed" + (l.note ? ": " + l.note : ""))}</div>
        </li>
      ))}
    </ul>
  );
}

// Hygiene: who cleans up after themselves and what the daemon still holds.
// Every run ends with the agent's own cleanup written against its name
// (`agent.cleanup`: the clone went, and what it left behind went with it or
// says why not); every ten minutes the daemon's sweep takes what finished
// work left and holds the rest with a reason. The list refreshes the moment
// a cleaning event lands in the store.
export function HygienePanel({ project, limit = 30 }: { project?: string; limit?: number }) {
  const path = "hygiene?limit=" + limit + (project ? "&project=" + encodeURIComponent(project) : "");
  const q = useApi<Hygiene>(path, { refresh: 60000 });
  const { events: live } = useLiveEvents();
  // Refresh when a cleaning event arrives on the stream (once per event).
  const seen = useRef(0);
  useEffect(() => {
    const newest = live.reduce((m, e) => (CLEANING.has(e.action) && e.id > m ? e.id : m), 0);
    if (newest > seen.current) {
      seen.current = newest;
      invalidate("hygiene");
    }
  }, [live]);
  const rows = useMemo(() => {
    const out = new Map<number, StoreEvent>();
    for (const e of [...live.filter((e) => CLEANING.has(e.action)), ...(q.data?.events ?? [])]) if (!out.has(e.id)) out.set(e.id, e);
    return [...out.values()]
      .filter((e) => !project || (e.detail as Record<string, unknown> | null)?.project === project || e.subject === project)
      .sort((a, b) => b.id - a.id)
      .slice(0, limit);
  }, [live, q.data, project, limit]);
  const h = q.data;
  const title = "Hygiene";
  if (!h) return <Panel title={title} icon={BrushCleaningIcon}>{q.error ? <EmptyState>{"The hygiene route is not available yet (" + q.error + ")."}</EmptyState> : <div className="text-xs text-muted-foreground">Loading…</div>}</Panel>;
  const sweep = h.sweep;
  // Scoped to one project the sweep's own items say what came from its checkout;
  // the counts and bytes are then those items', not the whole sweep's.
  const mine = (xs: SweptItem[] | undefined) => (xs ?? []).filter((it) => !project || it.project === project);
  const held = mine(sweep?.items?.held);
  const removed = mine(sweep?.items?.removed);
  const removedText = project ? countsText(countsOf(removed)) : countsText(sweep?.removed);
  const removedBytes = project ? removed.reduce((n, it) => n + (it.bytes || 0), 0) : (sweep?.bytes ?? 0);
  const heldText = project ? countsText(countsOf(held)) : countsText(sweep?.held);
  const messy = h.agents.filter((a) => a.left > 0);
  const leftover = held.length + messy.reduce((n, a) => n + a.left, 0);
  const summary = !leftover
    ? "Nothing to clean up."
    : plural(leftover, "leftover item") + " from finished runs - swept automatically" + (sweep ? "; last sweep " : ".");
  const details = (
    <>
      <Kv
        rows={[
          ["last sweep", sweep ? <When key="at" d={sweep.at} /> : "none yet"],
          ["removed", sweep ? (removedText ? removedText + " (" + sizeText(removedBytes) + " freed)" : project ? "nothing from this checkout" : "nothing to remove") : "—"],
          ["held", sweep ? heldText || (project ? "nothing from this checkout" : "nothing") : "—"],
          ...(sweep && sweep.errors && !project ? [["errors", num(sweep.errors) + " - pulse sweep --dry-run shows them"] as [React.ReactNode, React.ReactNode]] : []),
          ["agents", h.agents.length ? num(h.agents.reduce((n, a) => n + a.runs, 0)) + " runs ended in " + plural(h.agent_days, "day") + (messy.length ? "; " + messy.map((a) => a.agent + " left " + plural(a.left, "worktree") + (a.removed ? " (" + num(a.removed) + " removed by the runner" + (a.bytes ? ", " + sizeText(a.bytes) : "") + ")" : "")).join("; ") : "; every agent cleaned up after itself") : "no run has ended in " + plural(h.agent_days, "day")],
        ]}
      />
      <HeldList items={held} />
      <div className="mt-3" aria-label="cleaning">
        <p className="text-xs font-medium">What was cleaned</p>
        {rows.length ? (
          <ul className="scroll-thin mt-1 max-h-80 divide-y overflow-y-auto rounded-md border">
            {rows.map((e) => {
              const c = cleanupOf(e);
              return (
                <li key={e.id} className="min-w-0 px-2.5 py-2 text-xs">
                  <div className="flex min-w-0 items-center gap-2">
                    <Who row={{ agent: e.actor }} compact className="min-w-0 shrink" />
                    <ToneBadge tone={cleaningTone(e)} className="shrink-0">
                      {cleaningLabel(e)}
                    </ToneBadge>
                    <When d={e.at} className="ml-auto shrink-0 text-right text-muted-foreground" />
                  </div>
                  <div className="mt-1 truncate text-muted-foreground" title={c ? c.job || e.subject : e.subject}>
                    {c ? (c.project ? <span>{c.project + " · "}</span> : null) : null}
                    <Mono className="whitespace-nowrap [overflow-wrap:normal]">{c ? c.job || e.subject : e.subject}</Mono>
                  </div>
                  <div className="mt-0.5 text-foreground/90 anywhere">{cleaningText(e)}</div>
                  {c ? <LeftoverList left={c.left} /> : null}
                </li>
              );
            })}
          </ul>
        ) : (
          <EmptyState className="mt-1">{project ? "Nothing cleaned for " + project + " yet." : "Nothing cleaned yet. Finished runs and sweeps will show here."}</EmptyState>
        )}
      </div>
    </>
  );
  return (
    <Panel
      title={title}
      icon={BrushCleaningIcon}
      actions={
        <span className="flex items-center gap-1.5">
          <ToneBadge tone={h.settings.sweep === "off" ? "bad" : sweep ? "ok" : "neutral"} dot>
            {h.settings.sweep === "off" ? "sweep off" : sweep ? "sweeping" : "no sweep yet"}
          </ToneBadge>
          <Link href={hrefs.settings("daemon")} className="hover:underline">
            housekeeping
          </Link>
        </span>
      }
      footer={"Each agent removes the worktrees, scratch clones and temp folders it made before it finishes (brief rule 9); when a run ends the runner removes its clone and whatever the agent left, and writes it down against the agent. The daemon sweeps finished work every ten minutes after " + num(h.settings.grace_minutes) + " quiet minutes, keeps unmerged branches " + plural(h.settings.branch_days, "day") + ", and holds anything with uncommitted changes or a running job."}
    >
      <p className="mb-3 text-sm font-medium">
        {summary}
        {leftover && sweep ? <When d={sweep.at} className="ml-1 text-muted-foreground" /> : null}
      </p>
      {rows.length > 3 ? (
        <details className="rounded-xl border bg-card/60 px-3 py-2">
          <summary className="cursor-pointer select-none text-xs font-medium text-muted-foreground hover:text-foreground">Show details</summary>
          <div className="mt-3">{details}</div>
        </details>
      ) : (
        details
      )}
    </Panel>
  );
}
