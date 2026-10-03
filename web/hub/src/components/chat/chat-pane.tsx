"use client";

import Link from "next/link";
import { useEffect, useMemo, useRef, useState } from "react";
import { Loader2Icon, SendIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Separator } from "@/components/ui/separator";
import { showApiError } from "@/components/pulse/confirm";
import { Markdown } from "@/components/pulse/markdown";
import { EmptyState } from "@/components/pulse/panel";
import { AuthorBadge, Avatar, Mono, When } from "@/components/pulse/who";
import { cn } from "@/lib/utils";
import { fmtDate } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { writes, type Message, type Task } from "@/lib/api";
import { invalidate } from "@/lib/use-api";

const SEEN_KEY = "pulse.chat.seen";

function readSeen(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem(SEEN_KEY) || "{}") as Record<string, string>;
  } catch {
    return {};
  }
}

// Unread = newer than the last message id this browser saw for the
// project/task. Kept in localStorage: the owner's "have I read this" is a
// browser fact, not a store fact (read_by tracks the agents).
export function useSeen(key: string, latest?: string): [string, () => void] {
  const [seen, setSeen] = useState(() => (typeof window === "undefined" ? "" : readSeen()[key] || ""));
  const [lastKey, setLastKey] = useState(key);
  if (lastKey !== key) {
    setLastKey(key);
    setSeen(readSeen()[key] || "");
  }
  const mark = () => {
    if (!latest) return;
    const all = readSeen();
    all[key] = latest;
    localStorage.setItem(SEEN_KEY, JSON.stringify(all));
    setSeen(latest);
  };
  return [seen, mark];
}

interface Group {
  key: string;
  task: string | null;
  items: Message[];
}

function groupByTask(messages: Message[]): Group[] {
  const out: Group[] = [];
  for (const m of [...messages].sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : a.id < b.id ? -1 : 1))) {
    const key = m.task_id || "";
    const last = out[out.length - 1];
    if (last && last.key === key) last.items.push(m);
    else out.push({ key, task: m.task_id || null, items: [m] });
  }
  return out;
}

function dayLabel(at: string): string {
  const d = new Date(at);
  if (isNaN(d.getTime())) return at;
  const today = new Date();
  const same = (a: Date, b: Date) => a.toDateString() === b.toDateString();
  if (same(d, today)) return "Today";
  const y = new Date(today.getTime() - 86400000);
  if (same(d, y)) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}

// The conversation with a project's lead. Messages come grouped by task (or
// project-wide), human vs agent are badged, unread ones are marked, Enter
// sends (Shift-Enter for a new line). The lead is woken by the send.
export function ChatPane({ project, taskId, messages, tasks, lead, embedded, className }: { project: string; taskId?: string | null; messages: Message[]; tasks?: Task[]; lead?: string; embedded?: boolean; className?: string }) {
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const bottom = useRef<HTMLDivElement>(null);
  const seenKey = project + (taskId ? ":" + taskId : "");
  const latest = messages.length ? [...messages].sort((a, b) => (a.at < b.at ? 1 : -1))[0]!.id : undefined;
  const [seen, markSeen] = useSeen(seenKey, latest);
  const groups = useMemo(() => groupByTask(messages), [messages]);
  const titles = useMemo(() => new Map((tasks ?? []).map((t) => [t.id, t.title])), [tasks]);
  const firstUnread = useMemo(() => {
    if (!seen) return null;
    const sorted = [...messages].sort((a, b) => (a.at < b.at ? -1 : 1));
    const i = sorted.findIndex((m) => m.id === seen);
    return i >= 0 && i + 1 < sorted.length ? sorted[i + 1]!.id : null;
  }, [messages, seen]);

  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [messages.length]);

  // Mark as seen once the pane has been visible with the latest message.
  useEffect(() => {
    if (!latest || seen === latest) return;
    const id = setTimeout(markSeen, 1500);
    return () => clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [latest, seen]);

  const send = async () => {
    const body = draft.trim();
    if (!body || busy) return;
    setBusy(true);
    const id = toast.loading("Sending…");
    try {
      await writes.sendMessage(project, body, taskId || undefined);
      toast.success("Sent - the runner wakes " + (lead || project + "-lead") + ".", { id, duration: 3000 });
      setDraft("");
      invalidate((p) => p.startsWith("messages") || p.startsWith("tasks") || p.startsWith("events"));
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not send the message");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={cn("flex min-h-0 flex-col", embedded ? "h-full" : "h-[calc(100vh-14rem)] min-h-[420px] rounded-xl border bg-card shadow-card", className)}>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3" role="log" aria-live="polite" aria-label="Conversation">
        {messages.length === 0 ? (
          <EmptyState className="mx-auto mt-6 max-w-lg border-0">
            {taskId ? (
              <>
                No message on {taskId} yet. Write below; the lead replies here.
              </>
            ) : (
              <>
                No lead has answered yet. Ask for a status, give a direction, or open a task and the lead will break it into work.
              </>
            )}
          </EmptyState>
        ) : (
          groups.map((g, gi) => (
            <section key={g.key + gi} className="mb-4">
              {!taskId ? (
                <div className="mb-2 flex items-center gap-2 text-[11px] uppercase tracking-wide text-muted-foreground">
                  <Separator className="flex-1" />
                  {g.task ? (
                    <Link href={hrefs.task(project, g.task)} className="hover:underline">
                      <Mono className="text-[11px]">{g.task}</Mono>
                      {titles.get(g.task) ? <span className="ml-1 normal-case tracking-normal">{"· " + titles.get(g.task)}</span> : null}
                    </Link>
                  ) : (
                    <span>project-wide</span>
                  )}
                  <Separator className="flex-1" />
                </div>
              ) : null}
              <ol className="grid gap-2.5">
                {g.items.map((m, i) => {
                  const human = m.author === "human" || m.author === "owner";
                  const prev = g.items[i - 1];
                  const newDay = !prev || dayLabel(prev.at) !== dayLabel(m.at);
                  return (
                    <li key={m.id}>
                      {newDay ? <div className="my-1 text-center text-[11px] text-muted-foreground">{dayLabel(m.at)}</div> : null}
                      {firstUnread === m.id ? (
                        <div className="my-1 flex items-center gap-2 text-[11px] font-medium text-info">
                          <Separator className="flex-1 bg-info/40" /> new <Separator className="flex-1 bg-info/40" />
                        </div>
                      ) : null}
                      <div className={cn("flex gap-2", human && "flex-row-reverse")}>
                        <Avatar seed={m.author} human={human} size={26} className="mt-0.5" />
                        <div className={cn("max-w-[85%] min-w-0 rounded-2xl px-3 py-2 shadow-card", human ? "rounded-br-md bg-primary/10 text-foreground" : "rounded-bl-md border bg-card")}>
                          <div className="mb-1 flex items-center gap-2 text-[11px] text-muted-foreground">
                            {human ? <span className="font-medium">you</span> : <AuthorBadge author={m.author} />}
                            <time dateTime={m.at} title={fmtDate(m.at)}>
                              <When d={m.at} />
                            </time>
                          </div>
                          <Markdown source={m.body} />
                        </div>
                      </div>
                    </li>
                  );
                })}
              </ol>
            </section>
          ))
        )}
        <div ref={bottom} />
      </div>
      <form
        className="m-3 flex items-end gap-2 rounded-xl border bg-card p-2 shadow-card focus-within:ring-2 focus-within:ring-ring/30"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <Textarea
          aria-label={taskId ? "Message the lead about " + taskId : "Message the lead of " + project}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
          placeholder={taskId ? "Reply about " + taskId + "… (Enter sends, Shift-Enter for a new line)" : "Message " + (lead || project + "-lead") + "… (Enter sends, Shift-Enter for a new line)"}
          rows={2}
          className="max-h-40 min-h-10 flex-1 resize-y border-0 bg-transparent text-[13px] shadow-none focus-visible:ring-0"
          disabled={busy}
        />
        <Button type="submit" size="default" disabled={busy || !draft.trim()} aria-label="Send">
          {busy ? <Loader2Icon className="animate-spin" /> : <SendIcon />}
          Send
        </Button>
      </form>
    </div>
  );
}
