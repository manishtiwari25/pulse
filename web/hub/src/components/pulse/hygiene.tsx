"use client";

import Link from "next/link";
import { useEffect, useMemo, useRef, useState } from "react";
import { BrushCleaningIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Disclosure, EmptyState, Kv, Panel } from "./panel";
import { Mono, ToneBadge, When, Who } from "./who";
import { invalidate, useApi, useLiveEvents } from "@/lib/use-api";
import { andList, num, plural, sizeText, type Tone } from "@/lib/format";
import { statusWord } from "@/lib/words";
import { hrefs } from "@/lib/nav";
import type { AgentCleanup, Hygiene, Leftover, StoreEvent, SweptItem } from "@/lib/api";
import { RunnersSection } from "./runners";

// The cleaning events the hygiene route serves; a live one of these refreshes the panel.
const CLEANING = new Set(["daemon.sweep", "worktree.swept", "land.swept", "agent.cleanup"]);

export { sizeText };

// Kinds of things a cleanup removes or keeps, as a person would count them.
const KIND: Record<string, [string, string]> = {
  clone: ["clone", "clones"],
  scratch: ["scratch folder", "scratch folders"],
  worktree: ["worktree", "worktrees"],
  branch: ["branch", "branches"],
  patch: ["patch file", "patch files"],
  temp: ["temp folder", "temp folders"],
  session: ["agent session", "agent sessions"],
};
const kindCount = (k: string, n: number) => num(n) + " " + (KIND[k] ? KIND[k][n === 1 ? 0 : 1] : k + (n === 1 ? "" : "s"));
const kindWord = (k: string) => (KIND[k] ? KIND[k][0].charAt(0).toUpperCase() + KIND[k][0].slice(1) : k.charAt(0).toUpperCase() + k.slice(1));
const sentence = (s: string) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : s);

// "2 clones, 5 scratch folders and 1 worktree"; "" when nothing counts.
const countsText = (m: Record<string, number> | undefined | null): string =>
  andList(
    Object.entries(m ?? {})
      .filter(([, n]) => n > 0)
      .map(([k, n]) => kindCount(k, n)),
  );

const countsOf = (items: SweptItem[]): Record<string, number> => items.reduce<Record<string, number>>((m, it) => ({ ...m, [it.kind]: (m[it.kind] ?? 0) + 1 }), {});

export function cleanupOf(e: StoreEvent): AgentCleanup | null {
  if (e.action !== "agent.cleanup" || !e.detail || typeof e.detail !== "object") return null;
  const d = e.detail as Partial<AgentCleanup>;
  return { job: d.job ?? "", project: d.project ?? "", executor: d.executor ?? "", status: d.status ?? "", clone_mb: Number(d.clone_mb ?? 0), left: Array.isArray(d.left) ? (d.left as Leftover[]) : [], text: d.text ?? "" };
}

// What one cleaning event did, in a line: the agent's own cleanup, a sweep,
// or one worktree or branch the runner took from a project checkout.
export function cleaningText(e: StoreEvent): string {
  const c = cleanupOf(e);
  if (c) return sentence(c.text) || (c.left.length ? plural(c.left.length, "worktree") + " left behind" : "Nothing left behind");
  const d = (e.detail && typeof e.detail === "object" ? e.detail : {}) as Record<string, unknown>;
  if (e.action === "daemon.sweep") {
    const removed = countsText(d.removed as Record<string, number>);
    const held = Number(d.held ?? 0);
    const errors = Array.isArray(d.errors) ? d.errors.length : 0;
    return (removed ? "Removed " + removed + " (" + sizeText(Number(d.bytes ?? 0)) + " freed)" : "Nothing to remove") + (held ? "; kept " + num(held) : "") + (errors ? "; " + plural(errors, "error") : "");
  }
  const why = typeof d.why === "string" ? d.why : "";
  const project = typeof d.project === "string" && d.project ? d.project : "";
  return (project ? project + ": " : "") + why;
}

// Why the sweep kept something, in plain words; the raw rule stays in the tooltip.
function whyWord(why: string): string {
  const job = /^held by job \S+ \((\w+)\)/i.exec(why);
  if (job) return "A " + statusWord(job[1]).toLowerCase() + " job is using it";
  const kept = /^not merged; kept (\d+) days?/i.exec(why);
  if (kept) return "Not merged yet, so it is kept for " + plural(Number(kept[1]), "day");
  if (/uncommitted changes/i.test(why)) return "It has changes nobody has committed";
  return sentence(why);
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
  if (e.action === "agent.cleanup") return "Cleaned up after itself";
  if (e.action === "daemon.sweep") return "Cleanup pass";
  if (e.action === "worktree.swept") return "Removed a worktree";
  if (e.action === "land.swept") return "Removed a merged branch";
  return e.action;
}

function HeldList({ items }: { items: SweptItem[] }) {
  const [open, setOpen] = useState(false);
  if (!items.length) return null;
  const shown = open ? items : items.slice(0, 4);
  return (
    <div className="mt-4" aria-label="Kept for now">
      <p className="text-[13px] font-medium text-foreground">{"Kept for now (" + num(items.length) + ")"}</p>
      <p className="text-[13px] text-muted-foreground">Each one waits for a reason. The next cleanup takes it once the reason is gone.</p>
      <ul className="mt-2 grid gap-1.5 text-[13px]">
        {shown.map((it, i) => (
          <li key={it.kind + it.path + i} className="min-w-0 rounded-xl border bg-muted/30 px-3 py-2">
            <div className="flex min-w-0 items-center gap-2">
              <span className="shrink-0 font-medium text-foreground">{kindWord(it.kind)}</span>
              {it.project ? <span className="shrink-0 text-muted-foreground">{"in " + it.project}</span> : null}
              {it.bytes ? <span className="tnum ml-auto shrink-0 text-muted-foreground">{sizeText(it.bytes)}</span> : null}
            </div>
            <div className="mt-0.5 text-muted-foreground anywhere" title={it.why}>
              {whyWord(it.why)}
            </div>
            <div className="mt-1 min-w-0 truncate" title={it.path}>
              <Mono className="whitespace-nowrap text-xs [overflow-wrap:normal]">{it.path}</Mono>
            </div>
          </li>
        ))}
      </ul>
      {items.length > 4 ? (
        <Button size="xs" variant="ghost" className="mt-1 text-muted-foreground" onClick={() => setOpen((v) => !v)}>
          {open ? "Show fewer" : "Show all " + num(items.length)}
        </Button>
      ) : null}
    </div>
  );
}

function LeftoverList({ left }: { left: Leftover[] }) {
  if (!left.length) return null;
  return (
    <ul className="mt-1.5 grid gap-1 text-xs">
      {left.map((l, i) => (
        <li key={l.path + i} className="min-w-0 text-muted-foreground">
          <div className="flex min-w-0 items-center gap-1.5">
            <span className={"size-1.5 shrink-0 rounded-full " + (l.removed ? "bg-warn" : "bg-bad")} aria-hidden />
            <span className="shrink-0">{kindWord(l.kind)}</span>
            <span className="min-w-0 flex-1 truncate" title={l.path}>
              <Mono className="whitespace-nowrap text-xs [overflow-wrap:normal]">{l.path}</Mono>
            </span>
            <span className="tnum shrink-0">{sizeText(l.bytes)}</span>
          </div>
          <div className={"pl-3 anywhere " + (l.removed ? "" : "text-bad")}>{(l.branch ? "Branch " + l.branch + ". " : "") + (l.removed ? "The runner removed it." : "The runner couldn't remove it" + (l.note ? ": " + l.note : "."))}</div>
        </li>
      ))}
    </ul>
  );
}

// Cleanup: who cleans up after themselves and what the runner still keeps.
// Every run ends with the agent's own cleanup written against its name
// (`agent.cleanup`: the clone went, and what it left behind went with it or
// says why not); every ten minutes the runner's sweep takes what finished
// work left and keeps the rest with a reason. The panel opens on one
// sentence; the history folds under "Show what was cleaned" and refreshes
// the moment a cleaning event lands in the store. `heading={false}` drops
// the title row when the page already gives the section a heading.
export function HygienePanel({ project, limit = 30, heading = true }: { project?: string; limit?: number; heading?: boolean }) {
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
  const title = heading ? "Cleanup" : undefined;
  if (!h) return <Panel title={title} icon={BrushCleaningIcon}>{q.error ? <EmptyState>{"The cleanup history didn't load (" + q.error + "). It refreshes on its own every minute."}</EmptyState> : <div className="text-[13px] text-muted-foreground">Loading…</div>}</Panel>;
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
  const stuck = messy.reduce((n, a) => n + Math.max(0, a.left - a.removed), 0);
  const off = h.settings.sweep === "off";
  const summary = !held.length && !stuck ? "Nothing left behind." : [held.length ? plural(held.length, "leftover") + " kept for now, each for a reason." : "", stuck ? plural(stuck, "leftover") + " the runner couldn't remove." : ""].filter(Boolean).join(" ");
  const status = (
    <span className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
      {/* "On" is the normal case and the summary says when it last ran; only an exception gets a badge. */}
      {off || !sweep ? (
        <ToneBadge tone={off ? "bad" : "neutral"} dot>
          {off ? "Cleanup is off" : "Not run yet"}
        </ToneBadge>
      ) : null}
      <Link href={hrefs.settings("daemon")} className="text-[13px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline">
        Cleanup settings
      </Link>
    </span>
  );
  const agentsText = h.agents.length
    ? plural(
        h.agents.reduce((n, a) => n + a.runs, 0),
        "run",
      ) +
      " ended in the last " +
      plural(h.agent_days, "day") +
      ". " +
      (messy.length ? messy.map((a) => a.agent + " left " + plural(a.left, "worktree") + " behind" + (a.removed ? " (the runner removed " + num(a.removed) + (a.bytes ? ", " + sizeText(a.bytes) : "") + ")" : "")).join("; ") + "." : "Every agent cleaned up after itself.")
    : "No run has ended in the last " + plural(h.agent_days, "day") + ".";
  return (
    <Panel title={title} icon={BrushCleaningIcon} actions={heading ? status : undefined}>
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-6 gap-y-3">
        <div className="min-w-0 flex-1 basis-64">
          <p className="text-sm font-medium text-foreground">{summary}</p>
          <p className="mt-0.5 text-[13px] text-muted-foreground">
            {sweep ? (
              <>
                {"Last cleaned "}
                <When d={sweep.at} />
                {removedText ? ": removed " + removedText + ", " + sizeText(removedBytes) + " freed." : project ? ": nothing from this project." : ": nothing to remove."}
              </>
            ) : off ? (
              "Turn cleanup on in Settings and the runner cleans up every 10 minutes."
            ) : (
              "The runner hasn't cleaned up yet. It does so every 10 minutes while it runs."
            )}
          </p>
        </div>
        {heading ? null : status}
      </div>
      {project ? null : <RunnersSection note={h.runners} />}
      <Disclosure summary="Show what was cleaned" className="mt-3 border-t border-border/70 pt-3" contentClassName="mt-3">
        <div className="grid min-w-0 gap-1 [&>*]:min-w-0">
          <p className="max-w-3xl text-[13px] leading-relaxed text-muted-foreground">
            <span title="Rule 9 of the agent brief">Before an agent finishes, it removes the worktrees, scratch clones and temp folders it made.</span>
            {" When a run ends, the runner deletes the run's clone and anything the agent left, and notes it under the agent's name. Every 10 minutes the runner cleans up finished work that has been quiet for " +
              plural(h.settings.grace_minutes, "minute") +
              ", keeps unmerged branches for " +
              plural(h.settings.branch_days, "day") +
              ", and leaves alone anything with uncommitted changes or a running job."}
          </p>
          <Kv
            className="mt-3 text-[13px]"
            rows={[
              ["Last cleanup", sweep ? <When key="at" d={sweep.at} /> : "None yet"],
              ["Removed", sweep ? (removedText ? sentence(removedText) + " (" + sizeText(removedBytes) + " freed)" : project ? "Nothing from this project" : "Nothing to remove") : "—"],
              ["Kept", sweep ? sentence(heldText) || (project ? "Nothing from this project" : "Nothing") : "—"],
              ...(sweep && sweep.errors && !project
                ? [
                    [
                      "Errors",
                      <span key="err">
                        {plural(sweep.errors, "error") + ". To see them, run "}
                        <Mono>pulse sweep --dry-run</Mono>
                      </span>,
                    ] as [React.ReactNode, React.ReactNode],
                  ]
                : []),
              ["Agents", agentsText],
            ]}
          />
          <HeldList items={held} />
          <div className="mt-4" aria-label="cleaning">
            <p className="text-[13px] font-medium text-foreground">What was cleaned</p>
            {rows.length ? (
              <ul className="scroll-thin mt-2 max-h-96 divide-y overflow-y-auto rounded-xl border">
                {rows.map((e) => {
                  const c = cleanupOf(e);
                  const d = (e.detail && typeof e.detail === "object" ? e.detail : {}) as Record<string, unknown>;
                  const thing = e.action === "worktree.swept" || e.action === "land.swept" ? (typeof d.branch === "string" && d.branch ? d.branch : e.subject) : "";
                  return (
                    <li key={e.id} className="min-w-0 px-3 py-2.5 text-[13px]">
                      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
                        {e.actor === "daemon" ? (
                          <span className="inline-flex shrink-0 items-center gap-1.5 font-medium text-foreground" title="daemon">
                            <BrushCleaningIcon className="size-3.5 text-muted-foreground" aria-hidden />
                            The runner
                          </span>
                        ) : (
                          <Who row={{ agent: e.actor }} compact className="min-w-0 shrink" />
                        )}
                        <ToneBadge tone={cleaningTone(e)} className="shrink-0">
                          {cleaningLabel(e)}
                        </ToneBadge>
                        <When d={e.at} className="ml-auto shrink-0 text-right text-muted-foreground" />
                      </div>
                      <div className="mt-1 text-foreground/90 anywhere" title={c ? c.job || e.subject : e.subject}>
                        {c && c.project ? <span className="text-muted-foreground">{c.project + ": "}</span> : null}
                        {cleaningText(e)}
                      </div>
                      {thing ? (
                        <div className="mt-1 min-w-0 truncate" title={e.subject}>
                          <Mono className="whitespace-nowrap text-xs [overflow-wrap:normal]">{thing}</Mono>
                        </div>
                      ) : null}
                      {c ? <LeftoverList left={c.left} /> : null}
                    </li>
                  );
                })}
              </ul>
            ) : (
              <EmptyState className="mt-2">{project ? "Nothing cleaned for " + project + " yet. Finished runs show up here." : "Nothing cleaned yet. Finished runs and cleanup passes show up here."}</EmptyState>
            )}
          </div>
        </div>
      </Disclosure>
    </Panel>
  );
}
