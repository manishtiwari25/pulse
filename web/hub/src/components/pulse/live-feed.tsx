"use client";

import { useMemo } from "react";
import { RadioIcon } from "lucide-react";
import { EmptyState, Panel } from "./panel";
import { Mono, ToneBadge, When, Who } from "./who";
import { useApi, useLiveEvents } from "@/lib/use-api";
import type { EventsPayload, StoreEvent } from "@/lib/api";
import { statusTone, type Tone } from "@/lib/format";

function actionTone(a: string): Tone {
  if (a === "mention" || a === "post.question" || a === "post.handoff") return "warn";
  if (a === "hook.failed") return "bad";
  if (a === "land.pushed" || a === "land.landed") return "ok";
  if (a === "land.held") return "warn";
  if (a === "post" || a.startsWith("post.")) return "neutral";
  if (/\.(fail|failed|stop|stopped|deny|denied|cancelled|removed|pruned)$/.test(a)) return "bad";
  if (/\.(add|added|create|created)$/.test(a)) return "ok";
  if (/^decide\.|\.(decide|decided|approve|approved)$/.test(a)) return "info";
  return statusTone(a) === "neutral" ? "neutral" : statusTone(a);
}

function actionLabel(e: StoreEvent): string {
  if (e.action === "mention") {
    const d = e.detail as Record<string, unknown> | null;
    const by = d && typeof d.by === "string" && d.by ? " mentioned by " + d.by : " mentioned";
    return "@" + (e.subject || "agent") + by;
  }
  return e.action;
}

function detailText(d: unknown): string {
  if (d === null || d === undefined || d === "") return "";
  if (typeof d === "string") return d;
  try {
    const o = d as Record<string, unknown>;
    const keys = ["summary", "title", "reason", "words", "note", "status", "value", "kind"];
    for (const k of keys) if (typeof o[k] === "string" && o[k]) return String(o[k]);
    // Plain key/value details read as "project demo-app · task TASK-001".
    const parts = Object.entries(o)
      .filter(([, v]) => v !== null && v !== undefined && v !== "" && (typeof v !== "object"))
      .slice(0, 4)
      .map(([k, v]) => k.replace(/_id$/, "") + " " + String(v));
    return parts.length ? parts.join(" · ") : JSON.stringify(d);
  } catch {
    return String(d);
  }
}

// The live timeline: the last events from the store, newest first, merged
// from the initial GET and everything the SSE stream delivered since. With a
// project, both halves are that project's: the GET asks the hub for it and
// the stream (one connection for the whole hub) is filtered here.
export function LiveFeed({ project, limit = 40, title = "Live", height }: { project?: string; limit?: number; title?: string; height?: number }) {
  const { events: live, state } = useLiveEvents();
  const { data, error, loading } = useApi<EventsPayload>("events?limit=" + limit + (project ? "&project=" + encodeURIComponent(project) : ""), { refresh: 30000 });
  const rows = useMemo(() => {
    const seen = new Set<number>();
    const out: StoreEvent[] = [];
    for (const e of [...live, ...(data?.events ?? [])]) {
      if (seen.has(e.id)) continue;
      if (project && !matchesProject(e, project)) continue;
      seen.add(e.id);
      out.push(e);
    }
    return out.sort((a, b) => b.id - a.id).slice(0, limit);
  }, [live, data, project, limit]);
  return (
    <Panel
      title={title}
      icon={RadioIcon}
      actions={
        <span className="inline-flex items-center gap-1.5 rounded-full border bg-muted/40 px-2 py-0.5 text-[11.5px]">
          <span className={"size-1.5 rounded-full " + (state === "live" ? "bg-ok" : state === "polling" ? "bg-warn" : "bg-muted-foreground")} aria-hidden />
          {state === "live" ? "streaming" : state === "polling" ? "polling" : "connecting"}
        </span>
      }
      dense
    >
      {loading && !data ? (
        <div className="p-3 text-xs text-muted-foreground">Loading events…</div>
      ) : error && !rows.length ? (
        <div className="p-3 text-xs text-muted-foreground">{"The events route is not available yet (" + error + "). The board views still refresh on their own."}</div>
      ) : rows.length === 0 ? (
        <div className="p-3">
          <EmptyState>{project ? "No events for " + project + " yet. Every CLI write and every hub write on this project appears here the moment it lands in the store." : "No events yet. Every CLI write and every hub write appears here the moment it lands in the store."}</EmptyState>
        </div>
      ) : (
        <ul className="scroll-thin divide-y overflow-y-auto" style={height ? { maxHeight: height } : undefined}>
          {rows.map((e) => (
            <li key={e.id} className="px-3 py-2 text-xs transition-colors hover:bg-muted/40">
              <div className="flex items-center gap-2">
                <Who row={{ agent: e.actor }} compact className="w-24 shrink-0" />
                <ToneBadge tone={actionTone(e.action)} className="shrink-0">
                  {actionLabel(e)}
                </ToneBadge>
                <span className="min-w-0 flex-1 truncate">{e.subject ? <Mono className="truncate text-foreground">{e.subject}</Mono> : <span className="text-muted-foreground">{detailText(e.detail)}</span>}</span>
                <When d={e.at} className="ml-auto shrink-0 text-right text-muted-foreground" />
              </div>
              {e.subject && detailText(e.detail) ? <div className="mt-0.5 pl-[8.5rem] pr-16 text-muted-foreground anywhere">{detailText(e.detail)}</div> : null}
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

// The same rule the hub applies to GET /events?project= (bus.Filter): an
// event belongs to a project when its detail names it or its subject is the
// project itself (project.add, project.removed). Events that name no project
// - settings, executors, decisions, jobs of other projects - are not its.
function matchesProject(e: StoreEvent, project: string): boolean {
  if (e.subject === project) return true;
  const d = e.detail;
  return typeof d === "object" && d !== null && (d as Record<string, unknown>).project === project;
}
