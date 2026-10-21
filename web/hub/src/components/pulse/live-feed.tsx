"use client";

import { useMemo } from "react";
import { RadioIcon } from "lucide-react";
import { EmptyState, Panel } from "./panel";
import { Avatar, When } from "./who";
import { useApi, useLiveEvents } from "@/lib/use-api";
import type { EventsPayload, StoreEvent } from "@/lib/api";
import { statusTone, type Tone } from "@/lib/format";
import { settingLabel } from "@/lib/setting-labels";
import { statusWord, threadName } from "@/lib/words";
import { cn } from "@/lib/utils";

function actionTone(a: string): Tone {
  if (a === "mention" || a === "post.question" || a === "post.handoff") return "warn";
  if (a === "hook.failed") return "bad";
  if (a === "land.pushed" || a === "land.landed") return "ok";
  if (a === "land.held") return "warn";
  // Housekeeping: a run's own cleanup and the daemon's sweep are routine, not failures.
  if (a === "agent.cleanup" || a === "daemon.sweep" || a.endsWith(".swept")) return "neutral";
  if (a === "post" || a.startsWith("post.")) return "neutral";
  if (/\.(fail|failed|stop|stopped|deny|denied|cancelled|removed|pruned)$/.test(a)) return "bad";
  if (/\.(add|added|create|created)$/.test(a)) return "ok";
  if (/^decide\.|\.(decide|decided|approve|approved)$/.test(a)) return "info";
  return statusTone(a) === "neutral" ? "neutral" : statusTone(a);
}

const DOT: Record<Tone, string> = {
  neutral: "bg-muted-foreground/50",
  ok: "bg-ok",
  warn: "bg-warn",
  bad: "bg-bad",
  info: "bg-info",
  accent: "bg-accent-2",
  you: "bg-you",
};

// What a post of each kind reads as in a sentence: "aurora picked up a thread".
const POSTED: Record<string, string> = {
  claim: "picked up a thread",
  handoff: "handed work over",
  finding: "posted a finding",
  question: "asked a question",
  answer: "answered a question",
  suggestion: "made a suggestion",
  opinion: "gave an opinion",
  vote: "voted",
  amendment: "proposed a rule change",
  result: "posted a result",
  notice: "posted a notice",
  proposal: "made a proposal",
};

// The rest of the store's actions, as verbs. Unknown actions read as their
// own words ("thing.happened" reads "thing happened").
const VERB: Record<string, string> = {
  "compute.requested": "asked for an extra machine",
  "compute.approved": "approved an extra machine",
  "compute.denied": "turned down an extra machine",
  "compute.released": "released an extra machine",
  "job.add": "queued a job",
  "job.done": "finished a job",
  "job.failed": "stopped a job that failed",
  "job.cancelled": "cancelled a job",
  "message.sent": "sent a message",
  "record.created": "added a record",
  "record.updated": "updated a record",
  "record.revised": "updated a record",
  "executor.add": "added a machine",
  "executor.remove": "removed a machine",
  "executor.removed": "removed a machine",
  "task.created": "created a task",
  "task.amended": "changed a task",
  "task.cancelled": "cancelled a task",
  "land.landed": "merged finished work",
  "land.pushed": "pushed merged work",
  "land.held": "held back a merge",
  "hook.failed": "had a check fail",
  "project.add": "added the project",
  "project.removed": "removed the project",
  "project.restored": "restored the project",
  "settings.set": "changed a setting",
  "agent.cleanup": "cleaned up after a run",
  "daemon.sweep": "ran the runner's cleanup",
};

const rec = (d: unknown): Record<string, unknown> | null => (d && typeof d === "object" && !Array.isArray(d) ? (d as Record<string, unknown>) : null);
const str = (v: unknown) => (typeof v === "string" ? v : "");
const scalar = (v: unknown) => (typeof v === "string" || typeof v === "number" || typeof v === "boolean" ? String(v) : "");

// A task's new status as what the agent did: "started on a task".
const MOVED: Record<string, string> = {
  needs_owner: "asked you about a task",
  review: "asked you about a task",
  in_progress: "started on a task",
  planned: "planned a task",
  done: "finished a task",
  cancelled: "cancelled a task",
  open: "reopened a task",
};

// The author a post id ends with: "20260901T203300Z-cinder" -> "cinder".
const postAuthor = (id: string) => id.replace(/^\d{8}T\d{6}Z-?/, "");
// A value kept private ("[owner-private]") is not printed.
const hidden = (v: string) => /^\[.*\]$/.test(v);
const isSetting = (a: string) => a === "setting" || a === "settings.set";
const isMachine = (a: string) => a.startsWith("executor.");

function verb(e: StoreEvent): string {
  const d = rec(e.detail);
  const a = e.action;
  if (a === "mention") return "was mentioned" + (str(d?.by) ? " by " + str(d?.by) : "");
  if (a === "post" || a.startsWith("post.")) {
    const kind = a.startsWith("post.") ? a.slice(5) : str(d?.kind);
    return POSTED[kind] ?? "posted on the board";
  }
  if (a === "decide") {
    const answer = str(d?.answer);
    const who = postAuthor(e.subject || "");
    const what = who ? " to " + who + "\u2019s proposal" : "";
    return answer === "yes" ? "said yes" + what : answer === "no" ? "said no" + what : "answered" + (who ? " " + who + "\u2019s proposal" : "");
  }
  if (a === "task.status" && str(d?.to)) return MOVED[str(d?.to)] ?? "moved a task to " + statusWord(str(d?.to)).toLowerCase();
  if (a === "message.sent") return "sent a message" + (str(d?.task_id) ? " about " + str(d?.task_id) : "");
  if (isSetting(a)) {
    const value = scalar(e.detail) || scalar(d?.value);
    if (!e.subject) return "changed a setting";
    return "changed \u201c" + settingLabel(e.subject) + "\u201d" + (value && !hidden(value) ? " to " + value : "");
  }
  if (isMachine(a) && VERB[a] && e.subject) return VERB[a].replace(/a machine$/, "the machine " + e.subject);
  if (VERB[a]) return VERB[a];
  return a
    .replace(/[._-]+/g, " ")
    .replace(/\bdaemon\b/g, "runner")
    .replace(/\bexecutor\b/g, "machine")
    .trim();
}

function actorName(e: StoreEvent): string {
  if (e.action === "mention") return e.subject || "An agent";
  const a = (e.actor || "").trim();
  if (!a) return "PULSE";
  if (a === "human" || a === "owner") return "You";
  return a;
}

// The second line: what the event was about, in its own words when it has
// any, else where it happened (on the all-projects feed).
function detailText(e: StoreEvent, project?: string): string {
  const d = e.detail;
  if (d === null || d === undefined || d === "") return "";
  // A setting's value is already in the sentence.
  if (isSetting(e.action)) return "";
  if (typeof d !== "object") return scalar(d);
  const o = rec(d);
  if (!o) return "";
  const where = !project && str(o.project) ? str(o.project) : "";
  if ((e.action === "post" || e.action.startsWith("post.")) && str(o.thread)) return "In \u201c" + threadName(str(o.thread)) + "\u201d" + (where ? " on " + where : "");
  // A red base names the checks main itself fails (the runner's `failures`),
  // not its 1500-char verify tail.
  if (e.action === "land.base-red" && Array.isArray(o.failures)) {
    const names = o.failures.filter((f): f is string => typeof f === "string" && f.trim() !== "");
    if (names.length) return (str(o.note) ? str(o.note) + ": " : "") + names.slice(0, 5).join(", ") + (names.length > 5 ? " +" + (names.length - 5) + " more" : "");
  }
  for (const k of ["summary", "title", "reason", "words", "note", "text"]) if (str(o[k])) return str(o[k]);
  return where ? "In " + where : "";
}

// The subject rides along in brackets when people know it by name (TASK-001,
// ADR-001, EVO-constitution, a project). Post and job ids, message ids and
// timestamps stay in the tooltip; some actions already name it in the verb.
function subjectText(e: StoreEvent, project?: string): string {
  const s = (e.subject || "").trim();
  if (!s || s === project) return "";
  if (e.action === "mention" || e.action === "decide" || e.action === "message.sent" || isSetting(e.action) || isMachine(e.action)) return "";
  if (/^\d{8}T\d{6}Z/.test(s) || /^(job|msg)-/i.test(s) || /^[0-9a-f]{12,}$/i.test(s)) return "";
  return s;
}

// The live timeline: the last events from the store, newest first, merged
// from the initial GET and everything the SSE stream delivered since. With a
// project, both halves are that project's: the GET asks the hub for it and
// the stream (one connection for the whole hub) is filtered here.
export function LiveFeed({ project, limit = 40, title = "What just happened", height }: { project?: string; limit?: number; title?: string; height?: number }) {
  const { events: live, state } = useLiveEvents();
  const { data, error, loading } = useApi<EventsPayload>("events?limit=" + limit + (project ? "&project=" + encodeURIComponent(project) : ""), { refresh: 30000 });
  const rows = useMemo(() => {
    const seen = new Set<number>();
    const out: StoreEvent[] = [];
    for (const e of [...live, ...(data?.events ?? [])]) {
      if (seen.has(e.id)) continue;
      // Step events belong to the Harness page; here they would bury everything else.
      if (e.action === "harness.step") continue;
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
        <span className="inline-flex items-center gap-1.5 text-xs" title={state === "live" ? "New events arrive as they happen" : state === "polling" ? "The live stream is down; the list refreshes every 30 seconds" : "Connecting to the live stream"}>
          <span className={cn("size-2 rounded-full", state === "live" ? "bg-ok" : state === "polling" ? "bg-warn" : "bg-muted-foreground")} aria-hidden />
          {state === "live" ? "Live" : state === "polling" ? "Refreshing every 30 s" : "Connecting"}
        </span>
      }
      dense
    >
      {loading && !data ? (
        <div className="px-4 py-3 text-sm text-muted-foreground sm:px-5">Loading what happened…</div>
      ) : error && !rows.length ? (
        <div className="px-4 py-3 text-sm text-muted-foreground sm:px-5">{"The activity list didn't load (" + error + "). The rest of the page still refreshes on its own."}</div>
      ) : rows.length === 0 ? (
        <div className="p-4">
          <EmptyState>{project ? "Nothing has happened in " + project + " yet. New posts, decisions and task updates show up here as they happen." : "Nothing has happened yet. New posts, decisions and task updates show up here as they happen."}</EmptyState>
        </div>
      ) : (
        <ul className="scroll-thin divide-y divide-border/70 overflow-y-auto" style={height ? { maxHeight: height } : undefined}>
          {rows.map((e) => {
            const who = actorName(e);
            const text = detailText(e, project);
            const subject = subjectText(e, project);
            const tone = actionTone(e.action);
            return (
              <li key={e.id} className="flex min-w-0 items-start gap-3 px-4 py-2.5 sm:px-5" title={[e.action, e.subject].filter(Boolean).join(" ")}>
                <Avatar seed={who} size={24} human={who === "You"} className="mt-0.5" />
                <div className="min-w-0 flex-1">
                  <p className="text-sm leading-snug anywhere">
                    <span className="font-medium text-foreground">{who}</span> <span className="text-muted-foreground">{verb(e)}</span>
                    {subject ? <span className="text-muted-foreground">{" (" + subject + ")"}</span> : null}
                    {tone !== "neutral" ? <span className={cn("ml-1.5 inline-block size-1.5 -translate-y-px rounded-full align-middle", DOT[tone])} aria-hidden /> : null}
                  </p>
                  {text ? <p className="mt-0.5 line-clamp-2 text-[13px] leading-snug text-muted-foreground anywhere">{text}</p> : null}
                </div>
                <When d={e.at} className="shrink-0 pt-0.5 text-xs text-muted-foreground" />
              </li>
            );
          })}
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
