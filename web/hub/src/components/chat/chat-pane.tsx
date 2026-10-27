"use client";

import Link from "next/link";
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { Loader2Icon, MessagesSquareIcon, SendIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Separator } from "@/components/ui/separator";
import { showApiError } from "@/components/pulse/confirm";
import { Markdown } from "@/components/pulse/markdown";
import { EmptyState } from "@/components/pulse/panel";
import { Avatar, When } from "@/components/pulse/who";
import { AmendTaskDialog } from "@/components/tasks/task-dialogs";
import { cn } from "@/lib/utils";
import { fmtDate } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { writes, type LeadWork, type Message, type MessageChoice, type Task } from "@/lib/api";
import { invalidate, useNow } from "@/lib/use-api";
import { Ago } from "@/components/tasks/task-work";
import { isOwner, taskMessageLine, taskMessageState } from "@/lib/task-message-status";

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

// A touch screen has no Shift+Enter: there the return key starts a new line
// and only the Send button sends. Read from matchMedia after hydration
// (the server renders the desktop behaviour).
const COARSE = "(pointer: coarse)";
function subscribeCoarse(cb: () => void) {
  if (typeof window === "undefined" || !window.matchMedia) return () => {};
  const mq = window.matchMedia(COARSE);
  mq.addEventListener("change", cb);
  return () => mq.removeEventListener("change", cb);
}
function useCoarsePointer(): boolean {
  return useSyncExternalStore(
    subscribeCoarse,
    () => typeof window !== "undefined" && !!window.matchMedia && window.matchMedia(COARSE).matches,
    () => false,
  );
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

// The conversation with a project's lead agent. Messages come grouped by task
// (or about the whole project); the owner's bubbles sit on the right in a
// soft teal, the agent's on the left on white; a "New" line marks what
// arrived since this browser last looked. Enter sends (Shift+Enter for a new
// line; on a touch screen the return key is a new line and Send sends); a
// message sent with `taskId` is pinned to that task. The send wakes the lead.
// An agent's latest message on an open task may carry choices: they show as
// buttons under it, so the owner taps an answer instead of typing one.
// Under the owner's latest message of each thread a line says whether the
// lead has it: Sent, in line, reading (with the dots), seen - from
// `leadWork` (where the lead's sitting is) and the message's read marks;
// an answer below it replaces the line.
// Only the message list scrolls to the newest message, never the page, and
// it follows new messages only while the owner is already at the bottom.
export function ChatPane({ project, taskId, task, messages, tasks, lead, leadWork, embedded, className }: { project: string; taskId?: string | null; task?: Task; messages: Message[]; tasks?: Task[]; lead?: string; leadWork?: LeadWork | null; embedded?: boolean; className?: string }) {
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [amending, setAmending] = useState<Task | null>(null);
  const hintId = useId();
  const list = useRef<HTMLDivElement>(null);
  // Whether the owner is at (or near) the newest message; starts true so the
  // list opens on the latest message.
  const stick = useRef(true);
  const coarse = useCoarsePointer();
  const seenKey = project + (taskId ? ":" + taskId : "");
  const latest = messages.length ? [...messages].sort((a, b) => (a.at < b.at ? 1 : -1))[0]!.id : undefined;
  const [seen, markSeen] = useSeen(seenKey, latest);
  const groups = useMemo(() => groupByTask(messages), [messages]);
  const byId = useMemo(() => {
    const m = new Map((tasks ?? []).map((t) => [t.id, t]));
    if (task) m.set(task.id, task);
    return m;
  }, [tasks, task]);
  const titles = useMemo(() => new Map([...byId.values()].map((t) => [t.id, t.title])), [byId]);
  // The newest message of each task: choices are live only there, and only
  // while the task is still open - an older ask has been answered or overtaken.
  const lastOfTask = useMemo(() => {
    const m = new Map<string, string>();
    for (const g of groups) if (g.task) m.set(g.task, g.items[g.items.length - 1]!.id);
    return m;
  }, [groups]);
  // The owner's latest message of each thread: the only one whose delivery
  // is still a question.
  const lastOwnerOf = useMemo(() => {
    const m = new Map<string, string>();
    for (const g of groups) {
      const last = [...g.items].reverse().find((x) => isOwner(x.author));
      if (last) m.set(g.key, last.id);
    }
    return m;
  }, [groups]);
  const firstUnread = useMemo(() => {
    if (!seen) return null;
    const sorted = [...messages].sort((a, b) => (a.at < b.at ? -1 : 1));
    const i = sorted.findIndex((m) => m.id === seen);
    return i >= 0 && i + 1 < sorted.length ? sorted[i + 1]!.id : null;
  }, [messages, seen]);

  useLayoutEffect(() => {
    const el = list.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [messages.length]);

  // Mark as seen once the pane has been visible with the latest message.
  useEffect(() => {
    if (!latest || seen === latest) return;
    const id = setTimeout(markSeen, 1500);
    return () => clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [latest, seen]);

  const refresh = () => invalidate((p) => p.startsWith("messages") || p.startsWith("tasks") || p.startsWith("events"));

  const send = async () => {
    const body = draft.trim();
    if (!body || busy) return;
    setBusy(true);
    const id = toast.loading("Sending…");
    try {
      await writes.sendMessage(project, body, taskId || undefined);
      toast.success("Sent. " + sentenceStart(lead || "the lead agent") + " will reply here.", { id, duration: 3000 });
      setDraft("");
      stick.current = true;
      refresh();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Your message wasn't sent");
    } finally {
      setBusy(false);
    }
  };

  // One tap answers: a reply goes to the chat as the owner's words (and by
  // itself ends a review), "done" closes the task with them, "amend" opens
  // the brief.
  const pick = async (m: Message, c: MessageChoice) => {
    if (busy || !m.task_id) return;
    const t = byId.get(m.task_id);
    if (c.action === "amend") {
      if (t) setAmending(t);
      return;
    }
    setBusy(true);
    const id = toast.loading(c.action === "done" ? "Closing…" : "Sending…");
    try {
      if (c.action === "done") {
        await writes.setTaskStatus(m.task_id, "done", c.label);
        toast.success("Closed " + (t?.title || m.task_id) + ".", { id, duration: 3000 });
      } else {
        await writes.sendMessage(project, c.label, m.task_id);
        toast.success("Answered. " + sentenceStart(lead || "the lead agent") + " carries on.", { id, duration: 3000 });
      }
      stick.current = true;
      refresh();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, c.action === "done" ? "The task wasn't closed" : "Your answer wasn't sent");
    } finally {
      setBusy(false);
    }
  };

  const leadName = lead || "the lead agent";
  return (
    <div className={cn("flex min-h-0 flex-col", embedded ? "h-full" : "h-[calc(100vh-14rem)] min-h-[420px] overflow-hidden rounded-2xl border bg-card", className)}>
      {amending ? <AmendTaskDialog key={amending.id} task={amending} open onOpenChange={(o) => (o ? null : setAmending(null))} /> : null}
      <div
        ref={list}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
        }}
        className="scroll-thin min-h-0 flex-1 overflow-y-auto bg-background/60 px-4 py-4 sm:px-5"
        role="log"
        aria-live="polite"
        aria-label="Conversation"
      >
        {messages.length === 0 ? (
          <EmptyState className="mx-auto mt-6 max-w-lg" title="No messages yet" icon={<MessagesSquareIcon className="size-[18px]" />}>
            {taskId ? (
              <>Write below to ask how it&apos;s going or to steer the work. {sentenceStart(leadName)} replies here.</>
            ) : (
              <>Ask the lead agent for a status, give it a direction, or give a task and it will split it into steps. Its replies show up here.</>
            )}
          </EmptyState>
        ) : (
          groups.map((g, gi) => (
            <section key={g.key + gi} className="mb-5 last:mb-0" aria-label={g.task ? "About " + (titles.get(g.task) || g.task) : "About the whole project"}>
              {!taskId ? (
                <div className="mb-3 flex items-center gap-3 text-xs text-muted-foreground">
                  <Separator className="flex-1" />
                  {g.task ? (
                    <Link href={hrefs.task(project, g.task)} title={g.task} className="max-w-[70%] truncate font-medium text-foreground underline-offset-2 hover:underline">
                      {"About: " + (titles.get(g.task) || g.task)}
                    </Link>
                  ) : (
                    <span className="font-medium">About the whole project</span>
                  )}
                  <Separator className="flex-1" />
                </div>
              ) : null}
              <ol className="grid gap-3">
                {g.items.map((m, i) => {
                  const human = isOwner(m.author);
                  const prev = g.items[i - 1];
                  const newDay = !prev || dayLabel(prev.at) !== dayLabel(m.at);
                  // Consecutive messages from the same author share one name line.
                  const sameAuthor = !!prev && !newDay && prev.author === m.author && firstUnread !== m.id;
                  return (
                    <li key={m.id} className={cn(sameAuthor && "-mt-1.5")}>
                      {newDay ? <div className="mb-2 text-center text-xs font-medium text-muted-foreground">{dayLabel(m.at)}</div> : null}
                      {firstUnread === m.id ? (
                        <div className="mb-2 flex items-center gap-2 text-xs font-medium text-primary" role="separator" aria-label="New messages">
                          <Separator className="flex-1 bg-primary/40" /> New <Separator className="flex-1 bg-primary/40" />
                        </div>
                      ) : null}
                      <div className={cn("flex min-w-0 items-end gap-2", human && "flex-row-reverse")}>
                        {sameAuthor ? <span className="w-7 shrink-0" aria-hidden /> : <Avatar seed={m.author} human={human} size={28} className="mb-0.5" />}
                        <div className={cn("flex min-w-0 max-w-[min(85%,40rem)] flex-col", human ? "items-end" : "items-start")}>
                          {!sameAuthor ? (
                            <div className="mb-1 flex items-baseline gap-2 px-1 text-xs text-muted-foreground">
                              <span className="font-medium text-foreground">{human ? "You" : m.author}</span>
                              <When d={m.at} />
                            </div>
                          ) : null}
                          <div
                            title={sameAuthor ? fmtDate(m.at) : undefined}
                            className={cn("min-w-0 max-w-full rounded-2xl px-3.5 py-2.5 text-sm leading-relaxed text-foreground anywhere", human ? "rounded-br-md bg-primary/10 ring-1 ring-primary/15" : "rounded-bl-md border bg-card")}
                          >
                            <Markdown source={m.body} className="[&>:last-child]:mb-0" />
                          </div>
                          {human && lastOwnerOf.get(g.key) === m.id ? <DeliveryLine m={m} later={g.items.slice(i + 1)} lead={lead} work={leadWork} /> : null}
                          {!human && m.task_id && m.choices?.length && lastOfTask.get(m.task_id) === m.id && isOpen(byId.get(m.task_id)) ? (
                            <div className="mt-2 flex max-w-full flex-wrap gap-2 px-1" role="group" aria-label="Answers to pick">
                              {m.choices.map((c, ci) => {
                                const t = byId.get(m.task_id!);
                                if (c.action === "amend" && !t) return null;
                                return (
                                  <Button key={c.label} type="button" size="sm" variant={ci === 0 ? "default" : "outline"} className="h-8 rounded-full" disabled={busy} onClick={() => void pick(m, c)}>
                                    {c.label}
                                  </Button>
                                );
                              })}
                              <span className="self-center text-xs text-muted-foreground">or write below</span>
                            </div>
                          ) : null}
                        </div>
                      </div>
                    </li>
                  );
                })}
              </ol>
            </section>
          ))
        )}
      </div>
      <form
        className="border-t bg-card p-3"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <div className="flex items-end gap-2 rounded-xl border bg-background/40 p-1.5 pl-3 focus-within:border-ring focus-within:ring-2 focus-within:ring-ring/25">
          <Textarea
            aria-label={taskId ? "Message the lead agent about " + taskId : "Message the lead agent of " + project}
            aria-describedby={hintId}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !coarse && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void send();
              }
            }}
            placeholder="Write to the lead agent…"
            enterKeyHint={coarse ? "enter" : "send"}
            rows={2}
            className="max-h-40 min-h-10 flex-1 resize-none border-0 bg-transparent px-0 py-1.5 text-sm shadow-none focus-visible:ring-0 dark:bg-transparent"
            disabled={busy}
          />
          <Button type="submit" disabled={busy || !draft.trim()}>
            {busy ? <Loader2Icon className="animate-spin" /> : <SendIcon />}
            Send
          </Button>
        </div>
        <p id={hintId} className="mt-1.5 px-1 text-xs text-muted-foreground">
          {coarse ? (taskId ? "This message is about this task." : "Tap Send when your message is ready.") : "Enter sends, Shift+Enter starts a new line." + (taskId ? " This message is about this task." : "")}
        </p>
      </form>
    </div>
  );
}

const sentenceStart = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

// Under the owner's latest message: whether the lead has picked it up
// (task-message-status.ts). The dots run while the lead's sitting reads it;
// "seen" and "sent" carry the time; an answer below it says nothing here.
function DeliveryLine({ m, later, lead, work }: { m: Message; later: Message[]; lead?: string; work?: LeadWork | null }) {
  const now = useNow();
  const state = taskMessageState(m, later, lead, work, now);
  if (state.kind === "answered") return null;
  const line = taskMessageLine(state);
  return (
    <div className={cn("mt-1 flex items-center gap-1.5 px-1 text-xs", state.kind === "reading" ? "text-primary" : state.kind === "sent" && state.stale ? "text-amber-700 dark:text-amber-400" : "text-muted-foreground")} role="status">
      {state.kind === "reading" ? <TypingDots className="shrink-0" /> : null}
      <span>{line}</span>
      {state.kind === "reading" ? (
        <>
          <span aria-hidden>·</span>
          <Ago d={state.since} />
        </>
      ) : state.kind === "seen" ? (
        <Ago d={state.at} />
      ) : null}
    </div>
  );
}

function TypingDots({ className }: { className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-0.5", className)} aria-hidden>
      <span className="typing-dot size-1.5 rounded-full bg-current" />
      <span className="typing-dot size-1.5 rounded-full bg-current" />
      <span className="typing-dot size-1.5 rounded-full bg-current" />
    </span>
  );
}

// Choices stay live while the task can still take the answer. Without the
// task at hand (an older hub, a project-wide chat without the list) the
// answer is still offered: the lead asked, and a reply never hurts.
const isOpen = (t?: Task) => !t || (t.status !== "done" && t.status !== "cancelled");
