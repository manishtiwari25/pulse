"use client";

import { useMemo } from "react";
import { RadioIcon } from "lucide-react";
import { EmptyState, Panel } from "./panel";
import { Mono, ToneBadge, When, Who } from "./who";
import { useApi, useLiveEvents } from "@/lib/use-api";
import type { EventsPayload, StoreEvent } from "@/lib/api";
import { statusTone, type Tone } from "@/lib/format";

function actionTone(a: string): Tone {
  if (/\.(failed|cancelled|denied|removed|pruned|stopped)$/.test(a)) return "bad";
  if (/^decide\.|\.(accepted|approved|finished|landed|ready|amended)$/.test(a)) return "ok";
  if (/^task\.|^message|^msg/.test(a)) return "info";
  if (/^compute|^record|^constitution/.test(a)) return "accent";
  return statusTone(a);
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
// from the initial GET and everything the SSE stream delivered since.
export function LiveFeed({ project, limit = 40, title = "Live", height }: { project?: string; limit?: number; title?: string; height?: number }) {
  const { events: live, state } = useLiveEvents();
  const { data, error, loading } = useApi<EventsPayload>("events?limit=" + limit + (project ? "&project=" + encodeURIComponent(project) : ""), { refresh: 30000 });
  const rows = useMemo(() => {
    const seen = new Set<number>();
    const out: StoreEvent[] = [];
    for (const e of [...live, ...(data?.events ?? [])]) {
      if (seen.has(e.id)) continue;
      if (project && e.subject && !matchesProject(e, project)) continue;
      seen.add(e.id);
      out.push(e);
    }
    return out.sort((a, b) => b.id - a.id).slice(0, limit);
  }, [live, data, project, limit]);
  return (
    <Panel
      title={
        <span className="inline-flex items-center gap-1.5">
          <RadioIcon className={"size-3.5 " + (state === "live" ? "text-ok" : "text-muted-foreground")} /> {title}
        </span>
      }
      actions={<span>{state === "live" ? "streaming" : state === "polling" ? "polling every 10 s" : "connecting"}</span>}
      dense
    >
      {loading && !data ? (
        <div className="p-3 text-xs text-muted-foreground">Loading events…</div>
      ) : error && !rows.length ? (
        <div className="p-3 text-xs text-muted-foreground">{"The events route is not available yet (" + error + "). The board views still refresh on their own."}</div>
      ) : rows.length === 0 ? (
        <div className="p-3">
          <EmptyState>No events yet. Every CLI write and every hub write appears here the moment it lands in the store.</EmptyState>
        </div>
      ) : (
        <ul className="divide-y overflow-y-auto" style={height ? { maxHeight: height } : undefined}>
          {rows.map((e) => (
            <li key={e.id} className="px-3 py-1.5 text-xs">
              <div className="flex items-center gap-2">
                <When d={e.at} className="w-14 shrink-0 text-muted-foreground" />
                <ToneBadge tone={actionTone(e.action)} className="shrink-0">
                  {e.action}
                </ToneBadge>
                <span className="min-w-0 flex-1" />
                <Who row={{ agent: e.actor }} compact className="shrink-0 max-w-40" />
              </div>
              {e.subject || detailText(e.detail) ? (
                <div className="mt-0.5 pl-16 anywhere">
                  {e.subject ? <Mono className="text-foreground">{e.subject}</Mono> : null}
                  {detailText(e.detail) ? <span className="ml-1.5 text-muted-foreground">{detailText(e.detail)}</span> : null}
                </div>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function matchesProject(e: StoreEvent, project: string): boolean {
  const d = e.detail as Record<string, unknown> | null;
  if (d && typeof d === "object" && "project" in d) return d.project === project;
  return true;
}
