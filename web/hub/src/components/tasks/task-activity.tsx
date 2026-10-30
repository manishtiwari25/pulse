"use client";

import Link from "next/link";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ArrowRightIcon, BotIcon, CheckIcon, ChevronDownIcon, ChevronRightIcon, FilePenIcon, FilePlusIcon, ListChecksIcon, MessageSquareIcon, PlugIcon, RadioIcon, TerminalIcon, TriangleAlertIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Markdown } from "@/components/pulse/markdown";
import { Disclosure, EmptyState, ErrorNotice, SkeletonRows } from "@/components/pulse/panel";
import { Avatar, Mono, ToneBadge, When } from "@/components/pulse/who";
import { fmtDate, plural } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { postKindWord } from "@/lib/words";
import { cn } from "@/lib/utils";
import { enc, type ActivityItem, type Task, type TaskActivity } from "@/lib/api";
import { chapterLineCount, durationText, eventTouchesTask, exitBadge, groupActivity, isOwnerName, lastAt, runOutcome, runSummaryLine, systemLine, whoWord, type ActivityBlock } from "@/lib/task-activity";
import { invalidate, useApi, useLiveEvents, useNow } from "@/lib/use-api";
import { Ago } from "./task-work";

// How long the feed waits between refreshes while steps stream in: the
// store writes one or two a second, the eye needs far fewer.
const LIVE_THROTTLE_MS = 2500;

export function TaskActivityPane({ task, onChat }: { task: Task; onChat?: () => void }) {
  const [all, setAll] = useState(false);
  const path = "tasks/" + enc(task.id) + "/activity" + (all ? "?limit=0" : "");
  const { data, error, loading, reload } = useApi<TaskActivity>(path, { refresh: 15000 });
  const live = useLiveEvents();
  const list = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const lastEvent = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const jobIds = useMemo(() => new Set((data?.items || []).map((i) => i.job_id || "").filter(Boolean).concat((data?.runs || []).map((r) => r.job_id))), [data]);
  const sessionIds = useMemo(() => new Set((data?.runs || []).map((r) => r.session_id || "").filter(Boolean)), [data]);

  // Harness steps carry no task id and the hub's refresh rules leave them
  // alone (one or two a second across every agent). The feed watches the
  // live buffer for steps of its own jobs and refetches, at most every few
  // seconds. Job, run, task, message and landing events refresh tasks/*
  // through the rules already.
  useEffect(() => {
    if (!data) return;
    const hit = live.events.filter((e) => e.id > lastEvent.current && eventTouchesTask(e, jobIds, sessionIds));
    if (!hit.length) return;
    lastEvent.current = Math.max(...hit.map((e) => e.id));
    if (timer.current) return;
    timer.current = setTimeout(() => {
      timer.current = null;
      invalidate(path);
    }, LIVE_THROTTLE_MS);
  }, [live.events, data, jobIds, sessionIds, path]);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const count = data?.items.length ?? 0;
  useLayoutEffect(() => {
    const el = list.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [count, data?.running]);

  if (error && !data)
    return (
      <div className="p-5">
        <ErrorNotice error={error} onRetry={() => void reload()}>
          The activity didn&apos;t load.
        </ErrorNotice>
      </div>
    );
  if (!data) return <SkeletonRows n={6} className="p-5" />;
  const blocks = groupActivity(data);
  const latestRun = data.runs.length ? data.runs[data.runs.length - 1] : null;
  const closed = task.status === "done" || task.status === "cancelled";
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b px-5 py-2.5 text-[13px] text-muted-foreground">
        <span className="inline-flex items-center gap-2">
          {data.running ? (
            <>
              <RadioIcon className="size-3.5 text-primary" aria-hidden />
              <span className="text-foreground">Live: an agent is working on this task right now.</span>
            </>
          ) : closed ? (
            <span>Everything that happened on this task, in order.</span>
          ) : (
            <span>What has happened so far. New steps show up here as agents work.</span>
          )}
        </span>
        <span className="tnum">{plural(data.total, "line")}</span>
      </div>
      <div
        ref={list}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
        }}
        className="scroll-thin min-h-0 flex-1 overflow-y-auto bg-background/60 px-4 py-4 sm:px-5"
        role="log"
        aria-live="polite"
        aria-label="Activity on this task"
      >
        {data.has_more ? (
          <div className="mb-4 flex justify-center">
            <Button type="button" size="sm" variant="outline" className="rounded-full" onClick={() => setAll(true)} disabled={loading}>
              {"Show everything (" + plural(data.total - data.items.length, "earlier line") + ")"}
            </Button>
          </div>
        ) : null}
        <ol className="grid gap-3">
          {blocks.map((b) => (
            <li key={b.kind === "run" ? b.run.id + ":" + b.items[0].id : b.item.id} className="min-w-0">
              {b.kind === "run" ? <RunChapter block={b} expanded={!b.run.finished_at || (latestRun?.id === b.run.id && !closed)} task={task} /> : <Line item={b.item} task={task} onChat={onChat} />}
            </li>
          ))}
        </ol>
        {data.items.length <= 1 && !data.running ? (
          <EmptyState className="mx-auto mt-6 max-w-lg" title={closed ? "Nothing more happened" : "Nothing has happened yet"} icon={<BotIcon className="size-[18px]" />}>
            {closed ? "This task was closed before any agent worked on it." : "When the lead agent picks this task up, every message, step and command it runs shows up here as it happens."}
          </EmptyState>
        ) : null}
      </div>
    </div>
  );
}

// A line outside any run: the owner's words, a message, a step added, a
// landing, a post. Words from people stand as bubbles; the rest as one
// quiet system line.
function Line({ item, task, onChat }: { item: ActivityItem; task: Task; onChat?: () => void }) {
  if (item.kind === "task" && item.what === "created") return <Bubble who={item.who} at={item.at} title={item.title} body={item.text} />;
  if (item.kind === "message") {
    const open = task.status !== "done" && task.status !== "cancelled";
    // A worker's report on one of the task's jobs, written the moment the
    // job ended: the chat tab shows it with the step's landing state.
    const note = item.what === "report" ? "reported back" + (item.job_id ? " on job " + item.job_id : "") : undefined;
    return (
      <Bubble who={item.who} at={item.at} note={note} body={item.text} truncated={item.truncated}>
        {item.choices?.length && !isOwnerName(item.who) ? (
          <div className="mt-2 flex flex-wrap items-center gap-1.5 px-1">
            {item.choices.map((c) => (
              <span key={c} className="rounded-full border bg-card px-2.5 py-0.5 text-xs text-muted-foreground">
                {c}
              </span>
            ))}
            {open && onChat ? (
              <button type="button" onClick={onChat} className="inline-flex items-center gap-1 text-xs font-medium text-primary underline-offset-2 hover:underline">
                Answer in the conversation <ArrowRightIcon className="size-3" aria-hidden />
              </button>
            ) : null}
          </div>
        ) : null}
      </Bubble>
    );
  }
  if (item.kind === "post") {
    return (
      <div className="mx-auto flex w-full max-w-[40rem] items-start gap-2 rounded-xl border bg-card/80 px-3 py-2 text-[13px]">
        <Avatar seed={item.who} size={20} className="mt-0.5" />
        <div className="min-w-0 flex-1">
          <p className="flex flex-wrap items-baseline gap-x-2 text-xs text-muted-foreground">
            <span className="font-medium text-foreground">{item.who}</span>
            <span>{"posted a " + postKindWord(item.what).toLowerCase() + " on the board"}</span>
            {item.status ? <span>{"\u00b7 " + item.status}</span> : null}
            <When d={item.at} />
          </p>
          {item.text ? <p className="mt-0.5 leading-relaxed text-foreground anywhere">{item.text}</p> : null}
        </div>
      </div>
    );
  }
  if (item.kind === "step" || item.kind === "quiet") return <StepLine item={item} />;
  const tone = item.kind === "land" ? (item.what === "landed" || item.what === "pushed" || item.what === "pr" ? "ok" : item.what === "conflict" ? "bad" : item.what === "pr-asked" || item.what === "pr-answered" ? "neutral" : "warn") : item.kind === "task" && item.what === "cancelled" ? "bad" : "neutral";
  return <SystemLine at={item.at} tone={tone} text={systemLine(item)} detail={item.kind === "task" && item.what === "status" && item.text ? item.text : item.kind === "task" && item.what === "revised" ? item.text : undefined} />;
}

function SystemLine({ text, at, tone = "neutral", detail }: { text: string; at: string; tone?: "neutral" | "ok" | "warn" | "bad"; detail?: string }) {
  return (
    <div className="mx-auto grid max-w-[40rem] justify-items-center gap-1 text-center text-xs text-muted-foreground">
      <p className="flex flex-wrap items-baseline justify-center gap-x-2">
        <span className={cn("font-medium", tone === "ok" ? "text-ok" : tone === "bad" ? "text-bad" : tone === "warn" ? "text-warn" : "text-foreground/80")}>{text}</span>
        <When d={at} />
      </p>
      {detail ? <p className="max-w-full rounded-xl bg-muted/50 px-3 py-1.5 text-left text-[13px] leading-relaxed text-foreground anywhere">{detail}</p> : null}
    </div>
  );
}

function Bubble({ who, at, title, note, body, truncated, children }: { who: string; at: string; title?: string; note?: string; body?: string; truncated?: boolean; children?: React.ReactNode }) {
  const human = isOwnerName(who);
  return (
    <div className={cn("flex min-w-0 items-end gap-2", human && "flex-row-reverse")}>
      <Avatar seed={who} human={human} size={28} className="mb-0.5" />
      <div className={cn("flex min-w-0 max-w-[min(85%,40rem)] flex-col", human ? "items-end" : "items-start")}>
        <div className="mb-1 flex flex-wrap items-baseline gap-x-2 px-1 text-xs text-muted-foreground">
          <span className="font-medium text-foreground">{whoWord(who)}</span>
          {note ? <span>{note}</span> : null}
          <When d={at} />
        </div>
        <div className={cn("min-w-0 max-w-full rounded-2xl px-3.5 py-2.5 text-sm leading-relaxed text-foreground anywhere", human ? "rounded-br-md bg-primary/10 ring-1 ring-primary/15" : note ? "rounded-bl-md border border-dashed bg-muted/30" : "rounded-bl-md border bg-card")}>
          {title ? <p className="mb-1 font-medium">{title}</p> : null}
          {body ? <Markdown source={body} className="[&>:last-child]:mb-0" /> : null}
          {truncated ? <p className="mt-1 text-xs text-muted-foreground">Shortened here.</p> : null}
        </div>
        {children}
      </div>
    </div>
  );
}

// One run of one step: the worker's name on top, then what it said and did.
// An ended run folds to its summary line until asked; the live one stays
// open and follows along.
function RunChapter({ block, expanded, task }: { block: Extract<ActivityBlock, { kind: "run" }>; expanded: boolean; task: Task }) {
  const { run, items, first } = block;
  const [open, setOpen] = useState<boolean | null>(null);
  const now = useNow(10000);
  const show = open ?? expanded;
  const live = run.status === "running";
  const lines = chapterLineCount(items);
  const outcome = runOutcome(run.status);
  const last = lastAt(items);
  const ended = items.find((i) => i.kind === "run" && i.what === "ended");
  return (
    <section className="min-w-0 rounded-2xl border bg-card/60" aria-label={run.agent + " on " + run.job_title}>
      <header className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 text-[13px]">
        <Avatar seed={run.agent} size={24} />
        <div className="min-w-0 flex-1 basis-48">
          <p className="min-w-0 leading-snug">
            <span className="font-medium text-foreground">{run.agent}</span>
            <span className="text-muted-foreground">{first ? " started on " : " continued on "}</span>
            <span className="font-medium text-foreground anywhere">{"\u201c" + (run.job_title || "a step") + "\u201d"}</span>
            {run.executor && run.executor !== "local" ? <span className="text-muted-foreground">{" on " + run.executor}</span> : null}
          </p>
          <p className="text-xs text-muted-foreground">
            {live ? (
              <span className="inline-flex items-center gap-1.5 text-primary">
                <TypingDots />
                <span>{run.agent + " is working"}</span>
                {last ? (
                  <>
                    <span aria-hidden>·</span>
                    <span>
                      last step <Ago d={last} />
                    </span>
                  </>
                ) : null}
              </span>
            ) : (
              <span>{runSummaryLine(run, now)}</span>
            )}
          </p>
        </div>
        <div className="flex items-center gap-1.5">
          {!live ? <ToneBadge tone={outcome.tone === "live" ? "info" : outcome.tone}>{outcome.text}</ToneBadge> : null}
          {run.session_id ? (
            <Link href={hrefs.harness({ id: run.session_id })} title={"Run " + run.session_id} className="text-xs font-medium text-primary underline-offset-2 hover:underline">
              Full run
            </Link>
          ) : null}
          {lines ? (
            <Button type="button" size="sm" variant="ghost" className="h-7 gap-1 px-2 text-xs" onClick={() => setOpen(!show)} aria-expanded={show}>
              {show ? <ChevronDownIcon className="size-3.5" /> : <ChevronRightIcon className="size-3.5" />}
              {show ? "Hide" : "Show what it did (" + lines + ")"}
            </Button>
          ) : null}
        </div>
      </header>
      {show ? (
        <ol className="grid gap-2 border-t px-3 py-3">
          {items.map((i) => {
            if (i.kind === "run") return null;
            return (
              <li key={i.id} className="min-w-0">
                <StepLine item={i} inChapter />
              </li>
            );
          })}
          {live ? (
            <li className="flex items-center gap-2 px-1 text-xs text-primary" role="status">
              <TypingDots /> <span>{run.agent + " is working…"}</span>
            </li>
          ) : null}
        </ol>
      ) : null}
      {ended && (ended.text || ended.exit_code) ? (
        <footer className="border-t px-3 py-2 text-[13px]">
          <p className="flex flex-wrap items-baseline gap-x-2 text-xs text-muted-foreground">
            <span className={cn("font-medium", outcome.tone === "ok" ? "text-ok" : outcome.tone === "bad" ? "text-bad" : outcome.tone === "warn" ? "text-warn" : "text-foreground/80")}>{run.agent + " " + outcome.text + "."}</span>
            <When d={ended.at} />
            {ended.count ? <span>{plural(ended.count, "commit")}</span> : null}
          </p>
          {ended.text ? <p className="mt-1 leading-relaxed text-foreground anywhere">{ended.text}</p> : null}
        </footer>
      ) : null}
      {task.status === "cancelled" && live ? <p className="border-t px-3 py-2 text-xs text-muted-foreground">The task was cancelled; this run stops at its next step.</p> : null}
    </section>
  );
}

// A harness step as the feed shows it: the agent's words as a bubble, a
// command or file as one compact line with its outcome, a quiet stretch as
// a muted count.
function StepLine({ item, inChapter }: { item: ActivityItem; inChapter?: boolean }) {
  if (item.kind === "quiet") {
    return (
      <p className="px-1 text-xs text-muted-foreground" title={plural(item.count || 0, "step") + " the feed folds: reads, searches and notes"}>
        {item.title}
      </p>
    );
  }
  switch (item.what) {
    case "assistant":
      return (
        <div className={cn("flex min-w-0 items-end gap-2", inChapter && "pl-0")}>
          {!inChapter ? <Avatar seed={item.who} size={24} className="mb-0.5" /> : null}
          <div className="min-w-0 max-w-[min(92%,44rem)] rounded-2xl rounded-bl-md border bg-card px-3.5 py-2 text-sm leading-relaxed text-foreground anywhere">
            <Markdown source={item.text || ""} className="[&>:last-child]:mb-0" />
            {item.truncated ? <p className="mt-1 text-xs text-muted-foreground">Shortened here; the full run has all of it.</p> : null}
          </div>
        </div>
      );
    case "command": {
      const badge = exitBadge(item.exit_code);
      const dur = durationText(item.duration_ms);
      return (
        <div className="flex min-w-0 items-start gap-2 px-1 text-[13px]" title={item.text ? item.text : undefined}>
          <TerminalIcon className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden />
          <span className="min-w-0 flex-1 anywhere">
            <span className="text-foreground">{item.title || "Ran a command"}</span>
            {item.text && item.text !== item.title ? <span className="ml-2 font-mono text-xs text-muted-foreground">{firstLine(item.text)}</span> : null}
          </span>
          {dur ? <span className="tnum shrink-0 text-xs text-muted-foreground">{dur}</span> : null}
          {badge ? (
            <span className={cn("inline-flex shrink-0 items-center gap-0.5 text-xs", badge.bad ? "text-bad" : "text-ok")} title={badge.bad ? "The command failed (" + badge.text + ")" : "The command worked"}>
              {badge.bad ? <XIcon className="size-3" aria-hidden /> : <CheckIcon className="size-3" aria-hidden />}
              {badge.text}
            </span>
          ) : null}
        </div>
      );
    }
    case "edit":
    case "write":
      return (
        <div className="flex min-w-0 items-start gap-2 px-1 text-[13px]">
          {item.what === "edit" ? <FilePenIcon className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden /> : <FilePlusIcon className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden />}
          <span className="min-w-0 flex-1 text-muted-foreground">
            {item.what === "edit" ? "Edited " : "Wrote "}
            <Mono>{item.title}</Mono>
          </span>
        </div>
      );
    case "plan":
      return (
        <Disclosure
          className="px-1 text-[13px]"
          summary={
            <span className="inline-flex items-center gap-2 text-muted-foreground">
              <ListChecksIcon className="size-3.5" aria-hidden /> Updated its plan
            </span>
          }
          contentClassName="mt-1.5"
        >
          <div className="rounded-xl bg-muted/50 px-3 py-2 text-[13px]">
            <Markdown source={item.text || ""} className="[&>:last-child]:mb-0" />
          </div>
        </Disclosure>
      );
    case "error":
      return (
        <div className="flex min-w-0 items-start gap-2 rounded-xl bg-warn/10 px-3 py-2 text-[13px] text-foreground">
          <TriangleAlertIcon className="mt-0.5 size-3.5 shrink-0 text-warn" aria-hidden />
          <span className="min-w-0 flex-1 anywhere">{item.text}</span>
        </div>
      );
    case "exit":
      return (
        <p className="px-1 text-xs text-muted-foreground">
          {"Handed the work back: " + (item.status || "ended") + "."} <When d={item.at} />
        </p>
      );
    default:
      return (
        <div className="flex min-w-0 items-start gap-2 px-1 text-[13px]" title={fmtDate(item.at)}>
          {item.what === "mcp" ? <PlugIcon className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden /> : <MessageSquareIcon className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden />}
          <span className="min-w-0 flex-1 text-muted-foreground anywhere">
            {stepVerb(item.what)} {item.title ? <Mono>{item.title}</Mono> : null}
          </span>
        </div>
      );
  }
}

function stepVerb(kind: string): string {
  switch (kind) {
    case "mcp":
      return "Called a tool";
    case "fetch":
      return "Read a web page";
    case "search":
      return "Searched the web";
    case "skill":
      return "Loaded a skill";
    case "subagent":
      return "Handed a sub-task to a helper";
    default:
      return kind.charAt(0).toUpperCase() + kind.slice(1);
  }
}

const firstLine = (s: string) => s.split("\n")[0];

function TypingDots({ className }: { className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-0.5", className)} aria-hidden>
      <span className="typing-dot size-1.5 rounded-full bg-current" />
      <span className="typing-dot size-1.5 rounded-full bg-current" />
      <span className="typing-dot size-1.5 rounded-full bg-current" />
    </span>
  );
}
