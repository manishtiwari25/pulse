"use client";

// Conversations: the window where the owner asks an agent to work and watches
// it go - like the Copilot or Claude Code apps, inside the hub. The hub runs
// the agent (`/api/v1/chat`); this file only shows what it says, answers what
// it asks, and keeps the transcript growing without a reload.

import Link from "next/link";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import {
  ArchiveIcon,
  ArrowLeftIcon,
  ArrowUpIcon,
  CheckIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  CircleAlertIcon,
  CircleCheckIcon,
  CircleDashedIcon,
  CircleIcon,
  CircleStopIcon,
  CpuIcon,
  FolderGitIcon,
  GitBranchIcon,
  InfoIcon,
  ListChecksIcon,
  LoaderCircleIcon,
  MessageSquareIcon,
  MessageSquarePlusIcon,
  PaperclipIcon,
  PencilIcon,
  RefreshCwIcon,
  ShieldCheckIcon,
  ShieldQuestionIcon,
  SparklesIcon,
  XIcon,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { ArtifactList, ArtifactViewer, ChangeList, ChangesViewer, ReviewAllButton, bytesWord, fileIcon, useChatDiff, type ChatDiff, type ChatView } from "@/components/chat/changes";
import { STEP_ICON, STEP_LABEL, exitWords, mergeSteps, planItems } from "@/components/harness/harness";
import { Markdown } from "@/components/pulse/markdown";
import { PulseMascot, type PipMood } from "@/components/pulse/mascots";
import { EmptyState, ErrorNotice, Notice, PageTitle, SkeletonRows } from "@/components/pulse/panel";
import { ProjectLink, When } from "@/components/pulse/who";
import type { ChatArtifact, ChatAttachment, ChatChanges, ChatListResponse, ChatPending, ChatShowResponse, Conversation, ConversationLive, ConversationMessage, HarnessModel, HarnessModelsResponse, HarnessSessionResponse, HarnessStep, ProjectsPayload } from "@/lib/api";
import { ApiError, CHAT_ATTACHMENTS_PER_MESSAGE, CHAT_ATTACHMENT_LIMIT, api, chatAttachmentHref, chatAttachments, chatUpload, enc, errorText, isChatImage, query, writes } from "@/lib/api";
import { num, plural, sentence, span } from "@/lib/format";
import { messageStatus, type MessageStatus } from "@/lib/message-status";
import { hrefs } from "@/lib/nav";
import { ACTION_KINDS, beatsOf, brief, countWords, groupNotes, streamTail } from "@/lib/transcript";
import type { ActionItem } from "@/lib/transcript";
import { invalidate, useApi, useNow } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// ----- words ----------------------------------------------------------------

type Tone = "run" | "wait" | "ok" | "neutral" | "bad";

interface Mood {
  word: string;
  tone: Tone;
  pip: PipMood;
  dot: string;
}

// One sentence per state, the same on the list, the header and the tab title.
function moodOf(c: Conversation, live?: ConversationLive | null): Mood {
  const l = live ?? c.live;
  const status = l?.status || c.status;
  const pending = l?.pending ?? null;
  if (status === "waiting" && pending?.kind === "approval") return { word: "Needs a yes", tone: "wait", pip: "waiting", dot: "bg-warn" };
  if (status === "waiting") return { word: "Asked you", tone: "wait", pip: "waiting", dot: "bg-warn" };
  if (status === "running") return { word: "Working", tone: "run", pip: "busy", dot: "bg-primary" };
  if (status === "archived") return { word: "Archived", tone: "neutral", pip: "asleep", dot: "bg-muted-foreground/40" };
  if (status === "ended") return { word: "Session ended", tone: "neutral", pip: "curious", dot: "bg-muted-foreground/60" };
  return { word: c.turns ? "Idle" : "New", tone: "ok", pip: "happy", dot: "bg-ok" };
}

const TONE_TEXT: Record<Tone, string> = {
  run: "text-primary",
  wait: "text-warn",
  ok: "text-ok",
  neutral: "text-muted-foreground",
  bad: "text-bad",
};

const LET_AGENT_DECIDE = "__decide__";

function stepRange(m: ConversationMessage): { session: string; from: number; to: number } | null {
  const session = typeof m.extra?.session === "string" ? m.extra.session : "";
  const r = m.extra?.steps;
  if (!session || !Array.isArray(r) || r.length !== 2) return null;
  const from = Number(r[0]);
  const to = Number(r[1]);
  if (!Number.isFinite(from) || !Number.isFinite(to) || to < 0) return null;
  return { session, from, to };
}

// Steps worth showing under a reply: what the agent wrote along the way,
// the tools it used and what it noted - not the run's exit, and not the
// question it asked (the transcript carries that as its own row). The reply
// itself is the message; its step stays only for the time it took to write.
function turnSteps(all: HarnessStep[], from: number, to: number | null, reply: string): HarnessStep[] {
  return all.filter((s) => {
    if (s.n <= from || (to !== null && s.n > to)) return false;
    if (s.kind === "exit" && !(s.depth ?? 0)) return false;
    if (isQuestionStep(s)) return false;
    return true;
  }).map((s) => (s.kind === "assistant" && reply && s.content.trim() === reply.trim() ? { ...s, content: "" } : s));
}

// The question tool leaves a note step behind ("question: ... | choices");
// the transcript shows the question itself, so the note would be a repeat.
function isQuestionStep(s: HarnessStep): boolean {
  return s.kind === "note" && (s.command === "question" || /^question:/i.test(s.content.trimStart()));
}

// The folds behind the transcript - ACTION_KINDS, beatsOf, groupNotes,
// thoughtSeconds, streamTail, brief, ActionItem and Beat - live in
// @/lib/transcript, plain TypeScript the node test suite imports directly.

// A row's two parts: what the agent did in plain words (for a command, the
// description it gave the bash tool) and the exact request, dimmed after.
function actionWords(s: HarnessStep): { words: string; detail: string } {
  switch (s.kind) {
    case "command":
      return { words: (s.title ?? "").trim() || "Ran a command", detail: s.command || firstLine(s.content) };
    case "subagent":
      return { words: STEP_LABEL.subagent, detail: [s.command, firstLine(s.content)].filter(Boolean).join(": ") };
    case "exit":
      return { words: s.content === "submitted" ? "The helper reported back." : "The helper stopped: " + (s.content || "unknown") + ".", detail: "" };
    case "note":
      return { words: firstLine(sentence(s.content)), detail: "" };
    case "error":
      return { words: "Something went wrong", detail: firstLine(s.content) };
    default:
      return { words: STEP_LABEL[s.kind] ?? s.kind, detail: firstLine(s.content) || s.command };
  }
}

function firstLine(t: string): string {
  return (t || "").split("\n").find((l) => l.trim()) ?? "";
}

function Activity({ steps, live = false }: { steps: HarnessStep[]; live?: boolean }) {
  const beats = useMemo(() => beatsOf(steps), [steps]);
  if (!beats.length) return null;
  return (
    <div className="grid min-w-0 gap-3">
      {beats.map((b, i) => {
        const last = i === beats.length - 1;
        if (b.kind === "said") {
          return (
            <div key={b.step.n} className="min-w-0 text-[15px] leading-relaxed text-foreground">
              <Markdown source={b.step.content} className="[&_pre]:whitespace-pre-wrap" />
            </div>
          );
        }
        if (b.kind === "plan") return <PlanList key={b.step.n} step={b.step} />;
        return <ActionRun key={"a" + b.items[0].step.n} items={b.items} running={live && last} />;
      })}
    </div>
  );
}

const SHOW_LAST = 8;

function NoteFold({ items }: { items: ActionItem[] }) {
  const depth = Math.min(items[0].step.depth ?? 0, 3);
  const first = actionWords(items[0].step).words;
  return (
    <li className="min-w-0" style={depth ? { marginLeft: depth * 20 } : undefined}>
      <details className="group/notes min-w-0">
        <summary className="flex min-w-0 cursor-pointer list-none items-center gap-2.5 rounded-md px-1.5 py-1 text-[13px] leading-snug text-muted-foreground select-none hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
          <InfoIcon className="size-3.5 shrink-0 text-muted-foreground/70" aria-hidden />
          <span className="shrink-0">{plural(items.length, "note")} from PULSE</span>
          <span className="min-w-0 flex-1 truncate text-muted-foreground/60 group-open/notes:hidden" title={first}>
            {first}
          </span>
        </summary>
        <ol className="grid min-w-0" role="list">
          {items.map((it) => (
            <ActionRow key={it.step.n + ":" + (it.step.depth ?? 0)} item={it} running={false} />
          ))}
        </ol>
      </details>
    </li>
  );
}

// A run of tool calls, newest rows always in view: a long run folds its
// earlier rows under one line rather than filling the screen.
function ActionRun({ items, running }: { items: ActionItem[]; running: boolean }) {
  const rows = (list: ActionItem[], tailRunning: boolean) => {
    const groups = groupNotes(list);
    return (
      <ol className="grid min-w-0" role="list">
        {groups.map((g, i) =>
          Array.isArray(g) ? (
            <NoteFold key={"notes" + g[0].step.n} items={g} />
          ) : (
            <ActionRow key={g.step.n + ":" + (g.step.depth ?? 0)} item={g} running={tailRunning && i === groups.length - 1 && ACTION_KINDS.has(g.step.kind) && !g.result} />
          ),
        )}
      </ol>
    );
  };
  if (items.length <= SHOW_LAST) return rows(items, running);
  const earlier = items.slice(0, items.length - SHOW_LAST);
  return (
    <div className="grid min-w-0">
      <details className="group/more min-w-0">
        <summary className="flex w-fit cursor-pointer list-none items-center gap-2.5 rounded-md px-1.5 py-1 text-[13px] leading-snug text-muted-foreground select-none hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
          <ChevronRightIcon className="size-3.5 shrink-0 transition-transform duration-150 group-open/more:rotate-90 motion-reduce:transition-none" aria-hidden />
          {plural(earlier.length, "earlier step")} - {countWords(earlier)}
        </summary>
        {rows(earlier, false)}
      </details>
      {rows(items.slice(-SHOW_LAST), running)}
    </div>
  );
}

const OUTPUT_LINES = 60;

// One line of the transcript, the way the Copilot app lists a tool call:
// the kind's icon, what the agent did in plain words, the exact request
// dimmed after it - and nothing else unless it failed or is still running.
// A click folds the request and its output open underneath.
function ActionRow({ item, running }: { item: ActionItem; running: boolean }) {
  const { step: s, result } = item;
  const depth = Math.min(s.depth ?? 0, 3);
  const indent = depth ? { marginLeft: depth * 20 } : undefined;
  if (item.thought) {
    return (
      <li className="flex min-w-0 items-center gap-2.5 px-1.5 py-1 text-[13px] leading-snug text-muted-foreground" style={indent}>
        <CircleDashedIcon className="size-3.5 shrink-0 text-muted-foreground/70" aria-hidden />
        <span>Thought for {brief(item.thought)}</span>
      </li>
    );
  }
  const quiet = !ACTION_KINDS.has(s.kind);
  const Icon = s.kind === "exit" ? CircleCheckIcon : (STEP_ICON[s.kind] ?? InfoIcon);
  const { words, detail } = actionWords(s);
  const exit = result ? exitWords(result.exit_code) : null;
  const outLines = result?.content ? result.content.split("\n") : [];
  const bad = !!exit?.bad || s.kind === "error";
  const foldable = !quiet || (s.content || "").trim().length > words.length;
  const head = (
    <>
      <Icon className={cn("size-3.5 shrink-0", bad ? "text-bad" : quiet ? "text-muted-foreground/70" : "text-muted-foreground")} aria-hidden />
      <span className={cn("min-w-0 truncate", quiet ? "text-muted-foreground" : "text-foreground")} title={words}>
        {words}
      </span>
      {detail ? (
        <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-muted-foreground/60" title={detail}>
          {detail}
        </span>
      ) : null}
      {running ? (
        <LoaderCircleIcon className="ml-auto size-3 shrink-0 animate-spin text-primary" aria-label="running" />
      ) : bad ? (
        <span className="ml-auto shrink-0 text-[11px] text-bad" title={result ? exitWords(result.exit_code)?.text : undefined}>
          Failed
        </span>
      ) : null}
    </>
  );
  if (!foldable) {
    return (
      <li className="flex min-w-0 items-center gap-2.5 px-1.5 py-1 text-[13px] leading-snug" style={indent}>
        {head}
      </li>
    );
  }
  return (
    <li className="min-w-0" style={indent}>
      <details className="group/row min-w-0">
        <summary className="flex min-w-0 cursor-pointer list-none items-center gap-2.5 rounded-md px-1.5 py-1 text-[13px] leading-snug select-none hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
          {head}
        </summary>
        <div className="mt-1 mb-2 ml-6 grid min-w-0 gap-1.5">
          {s.kind === "command" ? (
            <pre className="min-w-0 rounded-lg bg-zinc-950 px-3 py-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-zinc-100 anywhere dark:bg-black">
              <span className="text-emerald-300 select-none">$ </span>
              {s.command}
            </pre>
          ) : (s.content || s.command) && !quiet ? (
            <pre className="min-w-0 rounded-lg border bg-muted/30 px-3 py-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-foreground/90 anywhere">{s.content || s.command}</pre>
          ) : quiet ? (
            <p className="min-w-0 text-[13px] leading-relaxed whitespace-pre-wrap text-muted-foreground anywhere">{s.content}</p>
          ) : null}
          {result ? (
            <pre className="max-h-72 min-w-0 overflow-auto rounded-lg border bg-background/60 px-3 py-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-foreground/80 anywhere">
              {outLines.length ? outLines.slice(0, OUTPUT_LINES).join("\n") : "(no output)"}
              {outLines.length > OUTPUT_LINES ? "\n… " + num(outLines.length - OUTPUT_LINES) + " more lines (the Harness page has them all)" : ""}
            </pre>
          ) : null}
          {result && (exit || result.duration_ms >= 1000) ? (
            <p className="text-[11px] text-muted-foreground tnum">{[exit?.text, result.duration_ms >= 1000 ? span(result.duration_ms / 1000) : ""].filter(Boolean).join(" · ")}</p>
          ) : null}
        </div>
      </details>
    </li>
  );
}

// The checklist the agent keeps with its plan tool: todo, doing, done.
function PlanList({ step, compact = false }: { step: HarnessStep; compact?: boolean }) {
  const items = planItems(step.content);
  if (!items.length) return null;
  const done = items.filter((it) => it.status === "done").length;
  return (
    <div className={cn("grid min-w-0 gap-1.5 rounded-xl border bg-muted/30 px-3 py-2.5", !compact && "border-primary/25 bg-primary/[0.04]")}>
      <p className="flex min-w-0 items-center gap-1.5 text-[12px] font-medium text-muted-foreground">
        <ListChecksIcon className="size-3.5 shrink-0 text-primary" aria-hidden />
        Plan
        <span className="tnum">
          {done}/{items.length} done
        </span>
      </p>
      <ul className="grid gap-1 text-[13px] leading-snug">
        {items.map((it, i) => (
          <li key={i} className="flex min-w-0 items-start gap-2">
            {it.status === "done" ? <CircleCheckIcon className="mt-0.5 size-3.5 shrink-0 text-ok" aria-label="done" /> : it.status === "doing" ? <LoaderCircleIcon className="mt-0.5 size-3.5 shrink-0 animate-spin text-primary" aria-label="in progress" /> : <CircleIcon className="mt-0.5 size-3.5 shrink-0 text-muted-foreground/60" aria-label="to do" />}
            <span className={cn("min-w-0 anywhere", it.status === "done" && "text-muted-foreground line-through", it.status === "doing" && "font-medium text-foreground")}>{it.text}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

// The agent's current plan: the last plan step of the session it runs in.
function latestPlan(d: Loaded): HarnessStep | null {
  const all = d.steps[d.sessionId] ?? [];
  for (let i = all.length - 1; i >= 0; i--) if (all[i].kind === "plan" && !(all[i].depth ?? 0)) return all[i];
  return null;
}

function maxN(rows: { n: number }[]): number {
  return rows.reduce((m, s) => Math.max(m, s.n), -1);
}

function fail(e: unknown, what: string) {
  const t = errorText(e);
  toast.error(what, { description: [t.why, t.fix].filter(Boolean).join(" ") || t.error });
}

// ----- the model ------------------------------------------------------------

const PROVIDER_DEFAULT = "__default__";

// Models an agent can run on: ones that chat and call tools. The provider's
// own default stays first so a conversation that never chose keeps working
// after a sign-in to a different provider.
function agentModels(models: HarnessModel[]): HarnessModel[] {
  return models.filter((m) => m.chat !== false && m.tool_calls !== false);
}

function modelWords(m: HarnessModel): string {
  const bits = [m.vendor, m.premium ? (m.premium === 1 ? "1 premium request" : num(m.premium) + " premium requests") : "no premium requests", m.preview ? "preview" : ""].filter(Boolean);
  return bits.join(" · ");
}

function ModelPicker({ value, onChange, models, disabled, compact, label }: { value: string; onChange: (model: string) => void; models: HarnessModel[]; disabled?: boolean; compact?: boolean; label: string }) {
  const rows = agentModels(models);
  const known = !value || rows.some((m) => m.id === value);
  const def = rows.find((m) => m.default);
  const current = value ? rows.find((m) => m.id === value)?.name || value : "Provider default";
  return (
    <Select value={value || PROVIDER_DEFAULT} onValueChange={(v) => onChange(v === PROVIDER_DEFAULT ? "" : v)} disabled={disabled}>
      <SelectTrigger size={compact ? "sm" : "default"} className={cn(compact ? "h-7 min-w-0 max-w-56 gap-1 rounded-full border-border/70 bg-background/60 px-2.5 text-xs" : "w-60")} aria-label={label} title={label}>
        <CpuIcon className={cn("shrink-0 text-primary", compact ? "size-3" : "size-3.5")} aria-hidden />
        {/* The trigger shows the name alone; the items below carry a second line. */}
        <SelectValue placeholder="Model" className="min-w-0 flex-1">
          <span className="truncate">{current}</span>
        </SelectValue>
      </SelectTrigger>
      <SelectContent position="popper" align={compact ? "end" : "start"} className="max-h-80 w-72">
        <SelectItem value={PROVIDER_DEFAULT}>
          <span className="grid">
            <span>Provider default{def ? " · " + (def.name || def.id) : ""}</span>
            <span className="text-[11px] text-muted-foreground">Whatever the sign-in picks; changes with it.</span>
          </span>
        </SelectItem>
        {!known ? <SelectItem value={value}>{value}</SelectItem> : null}
        {rows.map((m) => (
          <SelectItem key={m.id} value={m.id}>
            <span className="grid">
              <span className="flex items-center gap-1.5">
                {m.name || m.id}
                {m.default ? <span className="rounded-full bg-primary/10 px-1.5 text-[10px] font-medium text-primary">default</span> : null}
                {m.premium ? <span className="rounded-full bg-you-soft px-1.5 text-[10px] font-medium text-you-ink">{m.premium === 1 ? "premium" : m.premium + "× premium"}</span> : null}
              </span>
              {!compact ? <span className="text-[11px] text-muted-foreground">{modelWords(m)}</span> : null}
            </span>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

// ----- attachments ----------------------------------------------------------

// Files picked for the next message, before they are uploaded.
interface Picked {
  key: string;
  file: File;
}

// Adds files to the pick, honouring the hub's limits; says why one was left out.
function pickFiles(cur: Picked[], incoming: FileList | File[] | null | undefined): Picked[] {
  if (!incoming) return cur;
  const next = [...cur];
  const tooBig: string[] = [];
  let full = false;
  for (const f of Array.from(incoming)) {
    if (!f || !f.name) continue;
    if (f.size > CHAT_ATTACHMENT_LIMIT) {
      tooBig.push(f.name);
      continue;
    }
    if (next.length >= CHAT_ATTACHMENTS_PER_MESSAGE) {
      full = true;
      break;
    }
    if (next.some((p) => p.file.name === f.name && p.file.size === f.size)) continue;
    next.push({ key: f.name + ":" + f.size + ":" + f.lastModified + ":" + Math.random().toString(36).slice(2, 7), file: f });
  }
  if (tooBig.length) toast.error(tooBig.length === 1 ? tooBig[0] + " is over 8 MB" : plural(tooBig.length, "file") + " are over 8 MB", { description: "Attachments go up to 8 MB each. Put a bigger file in the project and name its path instead." });
  if (full) toast.error("That is enough for one message", { description: "Up to " + CHAT_ATTACHMENTS_PER_MESSAGE + " files go with a message. Send these, then attach the rest." });
  return next;
}

// The chips under a message: what was attached, each a link to the file.
// Images (a pasted screenshot, a picked photo) show a thumbnail that opens
// the full file in a new tab; every other file keeps the download chip.
function AttachmentChips({ id, list, onUser }: { id: string; list: ChatAttachment[]; onUser?: boolean }) {
  if (!list.length) return null;
  const images = list.filter(isChatImage);
  const files = list.filter((a) => !isChatImage(a));
  return (
    <div className={cn("grid min-w-0 gap-1.5", onUser && "justify-items-end")}>
      {images.length ? (
        <ul className={cn("flex min-w-0 flex-wrap gap-1.5", onUser && "justify-end")} aria-label={plural(images.length, "image")}>
          {images.map((a) => (
            <li key={a.name} className="min-w-0">
              <a
                href={chatAttachmentHref(id, a.name)}
                target="_blank"
                rel="noreferrer"
                className={cn("block overflow-hidden rounded-lg border transition-colors", onUser ? "border-primary-foreground/25 hover:border-primary-foreground/50" : "border-border hover:border-foreground/40")}
                title={a.name + " (" + bytesWord(a.size) + ")"}
              >
                {/* plain img: the hub serves the file itself, no next/image loader involved */}
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img src={chatAttachmentHref(id, a.name)} alt={a.name} loading="lazy" className="h-28 w-auto max-w-56 object-cover" />
              </a>
            </li>
          ))}
        </ul>
      ) : null}
      {files.length ? (
        <ul className={cn("flex min-w-0 flex-wrap gap-1.5", onUser && "justify-end")} aria-label={plural(files.length, "attachment")}>
          {files.map((a) => {
            const Icon = fileIcon(a.name);
            return (
              <li key={a.name} className="min-w-0">
                <a
                  href={chatAttachmentHref(id, a.name)}
                  target="_blank"
                  rel="noreferrer"
                  className={cn("flex min-w-0 max-w-64 items-center gap-1.5 rounded-lg border px-2 py-1 text-xs transition-colors", onUser ? "border-primary-foreground/25 bg-primary-foreground/10 text-primary-foreground hover:bg-primary-foreground/20" : "border-border bg-card text-foreground hover:bg-muted/60")}
                  title={a.name + " (" + bytesWord(a.size) + ")"}
                >
                  <Icon className="size-3.5 shrink-0 opacity-80" aria-hidden />
                  <span className="min-w-0 truncate font-medium">{a.name}</span>
                  <span className={cn("shrink-0 tnum", onUser ? "opacity-75" : "text-muted-foreground")}>{bytesWord(a.size)}</span>
                </a>
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}

// Three dots that breathe, for "the agent is thinking".
function TypingDots({ className }: { className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-0.5", className)} aria-hidden>
      <span className="typing-dot size-1.5 rounded-full bg-current" />
      <span className="typing-dot size-1.5 rounded-full bg-current" />
      <span className="typing-dot size-1.5 rounded-full bg-current" />
    </span>
  );
}

// ----- page -----------------------------------------------------------------

export function ChatPageClient() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const id = sp.get("id") || "";
  const [showArchived, setShowArchived] = useState(false);
  const list = useApi<ChatListResponse>("chat" + (showArchived ? "?all=1" : ""), { refresh: 5000 });
  const projects = useApi<ProjectsPayload>("projects", { refresh: 60000 });

  const open = useCallback(
    (next: string | null) => {
      router.replace(pathname + query({ id: next || undefined }), { scroll: false });
    },
    [router, pathname],
  );

  const rows = useMemo(() => {
    const all = list.data?.conversations ?? [];
    return [...all].sort((a, b) => (b.last_at || b.created_at).localeCompare(a.last_at || a.created_at));
  }, [list.data]);
  const current = rows.find((c) => c.id === id);
  const attention = rows.filter((c) => (c.live?.status || c.status) === "waiting").length;

  // The tab says when an agent waits on you, even from another window
  // (the shell names the page; this only adds the count).
  useEffect(() => {
    if (typeof document === "undefined") return;
    const base = "Chat · PULSE";
    document.title = attention ? `(${attention}) ${base}` : base;
  }, [attention]);

  // An older hub has no /chat routes (404) or no conversations table (503).
  const unavailable = !!list.error && !list.data && /not available yet|not found|HTTP 404|HTTP 503/i.test(list.error);

  const listNode = <ConversationList rows={rows} loading={list.loading && !list.data} current={id} onOpen={open} showArchived={showArchived} onToggleArchived={() => setShowArchived((v) => !v)} />;

  return (
    <div className={cn("grid min-w-0 gap-4", id ? "lg:flex lg:h-[calc(100dvh-1rem)] lg:min-h-[34rem] lg:flex-col" : "lg:pb-10")}>
      <PageTitle
        icon={MessageSquareIcon}
        className={id ? "mb-0 pt-4 sm:pt-5" : undefined}
        sub={id ? undefined : "Ask an agent to do something. It works in its own branch, shows every step, and asks when it needs you. Leave a conversation without a project and the agent picks the project itself."}
        actions={
          <Button type="button" size="sm" onClick={() => open(null)} disabled={!id}>
            <MessageSquarePlusIcon /> New conversation
          </Button>
        }
      >
        Chat
      </PageTitle>
      {unavailable ? (
        <Notice tone="warn">Conversations need a hub built with this feature. Update PULSE (<code>pulse update</code>) and restart the hub.</Notice>
      ) : list.error && !list.data ? (
        <ErrorNotice error={list.error} onRetry={() => void list.reload()}>
          The conversation list did not load.
        </ErrorNotice>
      ) : null}
      {id ? (
        <ConversationView key={id} id={id} seed={current} projects={projects.data} list={listNode} onBack={() => open(null)} onGone={() => open(null)} />
      ) : (
        <Split list={listNode} listOnMobile main={<NewConversation projects={projects.data} onCreated={(c) => open(c.id)} busyElsewhere={rows.filter((c) => (c.live?.status || c.status) === "running").length} />} />
      )}
    </div>
  );
}

// The page's columns. On a phone it flows as one: the transcript, then the
// side panel (plan, changes, artifacts, project); the list gives way to the
// open conversation. On a desktop the conversations list takes a column on
// the left and the transcript the rest, the panel under the list. Given
// enough width the panel gets a column of its own on the right - the way
// the Copilot app keeps a session's changes beside the chat - so it sits at
// the top where you see it instead of under however many conversations the
// list holds. The width is read from the box itself, not the window: the
// hub's own sidebar and the page's margins take a share that differs by
// breakpoint. With `fill` (a conversation open on a desktop) the columns
// take the rest of the window: the transcript scrolls inside its card, the
// composer stays on screen, and each side column scrolls on its own, so a
// long diff never pushes the page about.
const THREE_COLUMNS_AT = 1100;
const ROOMY_AT = 1264;

function useWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver((entries) => {
      const e = entries[0];
      if (e) setWidth(e.contentRect.width);
    });
    ro.observe(el);
    setWidth(el.getBoundingClientRect().width);
    return () => ro.disconnect();
  }, [ref]);
  return width;
}

function Split({ list, main, panel, listOnMobile, fill }: { list: ReactNode; main: ReactNode; panel?: ReactNode; listOnMobile?: boolean; fill?: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  const width = useWidth(box);
  const three = !!panel && width >= THREE_COLUMNS_AT;
  const side = cn("scroll-thin grid min-w-0 content-start gap-4 lg:order-none lg:overflow-y-auto lg:pb-1", fill ? "lg:min-h-0" : "lg:sticky lg:top-4 lg:max-h-[calc(100dvh-2rem)]");
  return (
    <div
      ref={box}
      className={cn(
        "grid min-w-0 gap-4",
        three ? (width >= ROOMY_AT ? "lg:grid-cols-[15rem_minmax(0,1fr)_22rem]" : "lg:grid-cols-[14rem_minmax(0,1fr)_19rem]") : "lg:grid-cols-[17rem_minmax(0,1fr)] xl:grid-cols-[19rem_minmax(0,1fr)] 2xl:grid-cols-[21rem_minmax(0,1fr)]",
        fill ? "lg:min-h-0 lg:flex-1 lg:grid-rows-[minmax(0,1fr)]" : "lg:items-start",
      )}
    >
      <aside className={cn(side, "order-2 lg:pr-1", (!panel || three) && !listOnMobile && "hidden lg:grid")}>
        <div className={cn("min-w-0", !listOnMobile && "hidden lg:block")}>{list}</div>
        {panel && !three ? <div className="min-w-0">{panel}</div> : null}
      </aside>
      <main className={cn("order-1 min-w-0 lg:order-none", fill && "lg:flex lg:min-h-0 lg:flex-col")}>{main}</main>
      {panel && three ? (
        <aside className={cn(side, "order-3 lg:pl-1")} aria-label="Work of this conversation">
          {panel}
        </aside>
      ) : null}
    </div>
  );
}

// ----- the list -------------------------------------------------------------

function ConversationList({ rows, loading, current, onOpen, showArchived, onToggleArchived }: { rows: Conversation[]; loading: boolean; current: string; onOpen: (id: string) => void; showArchived: boolean; onToggleArchived: () => void }) {
  const now = useNow(15000);
  return (
    <div className="grid min-w-0 gap-2">
      <div className="flex items-center justify-between px-1">
        <h2 className="text-[13px] font-semibold tracking-wide text-muted-foreground uppercase">Conversations</h2>
        <button type="button" className="text-xs text-muted-foreground underline-offset-4 hover:text-foreground hover:underline" onClick={onToggleArchived}>
          {showArchived ? "Hide archived" : "Show archived"}
        </button>
      </div>
      {loading ? (
        <SkeletonRows n={4} />
      ) : rows.length === 0 ? (
        <EmptyState icon={<PulseMascot mood="happy" size={56} decorative />} hint="Start one on the right. Each conversation is its own agent with its own branch.">
          No conversations yet.
        </EmptyState>
      ) : (
        <ul className="grid min-w-0 gap-1.5" role="list">
          {rows.map((c, i) => {
            const m = moodOf(c);
            const live = (c.live?.status || c.status) === "running";
            const active = c.id === current;
            return (
              <li key={c.id} className="enter min-w-0" style={{ animationDelay: Math.min(i, 8) * 30 + "ms" }}>
                <button
                  type="button"
                  onClick={() => onOpen(c.id)}
                  aria-current={active ? "page" : undefined}
                  className={cn(
                    "lift grid w-full min-w-0 gap-1 rounded-xl border bg-card px-3 py-2.5 text-left transition-colors",
                    active ? "border-primary/50 bg-primary/[0.06] shadow-card" : "hover:bg-muted/40",
                    m.tone === "wait" && !active && "border-warn/50",
                  )}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <span className={cn("live-dot size-2 shrink-0 rounded-full", m.dot)} data-live={live} aria-hidden />
                    <span className="min-w-0 flex-1 truncate text-[14px] font-medium text-foreground">{c.title || "Untitled"}</span>
                    {m.tone === "wait" ? <ShieldQuestionIcon className="size-3.5 shrink-0 text-warn attn rounded-full" aria-label="waiting for you" /> : null}
                  </span>
                  <span className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
                    <span className={cn("shrink-0 font-medium", TONE_TEXT[m.tone])}>{m.word}</span>
                    <span aria-hidden>·</span>
                    <span className="truncate">{c.project || "no project yet"}</span>
                    <span className="ml-auto shrink-0 tnum">{relShort(c.last_at || c.created_at, now)}</span>
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function relShort(d: string, now: number): string {
  const t = Date.parse(d);
  if (!Number.isFinite(t)) return "";
  const s = Math.max(0, Math.round((now - t) / 1000));
  if (s < 60) return "now";
  if (s < 3600) return Math.round(s / 60) + "m";
  if (s < 86400) return Math.round(s / 3600) + "h";
  return Math.round(s / 86400) + "d";
}

// ----- new conversation -----------------------------------------------------

const STARTERS = [
  "Look through the open tasks and pick the most useful one to do next.",
  "Read the latest failing check and fix it.",
  "Write a short guide for a new contributor and put it in docs/.",
  "Review what landed this week and tell me what looks risky.",
];

function NewConversation({ projects, onCreated, busyElsewhere }: { projects?: ProjectsPayload; onCreated: (c: Conversation) => void; busyElsewhere: number }) {
  const [text, setText] = useState("");
  const [project, setProject] = useState(LET_AGENT_DECIDE);
  const [model, setModel] = useState("");
  const [auto, setAuto] = useState(true);
  const [busy, setBusy] = useState(false);
  const list = (projects?.projects ?? []).filter((p) => p.status !== "inactive");
  const models = useApi<HarnessModelsResponse>("harness/models", { refresh: 60000 });

  const start = async () => {
    const t = text.trim();
    if (!t || busy) return;
    setBusy(true);
    try {
      const r = await writes.chatCreate({ text: t, project: project === LET_AGENT_DECIDE ? "" : project, auto_approve: auto, model });
      invalidate("chat");
      onCreated(r.conversation);
    } catch (e) {
      fail(e, "The conversation did not start");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="enter grid min-w-0 gap-4">
      <section className="relative overflow-hidden rounded-2xl border bg-card p-5 shadow-card sm:p-6">
        <div className="pointer-events-none absolute -top-10 -right-10 size-48 rounded-full bg-primary/10 blur-3xl" aria-hidden />
        <div className="flex min-w-0 flex-col gap-5 sm:flex-row sm:items-start">
          <PulseMascot mood={busy ? "busy" : text.trim() ? "curious" : "happy"} size={84} className="float shrink-0 self-center sm:self-start" title="Pip is ready" />
          <div className="grid min-w-0 flex-1 gap-4">
            <div className="grid gap-1">
              <h2 className="text-xl font-semibold tracking-tight text-foreground">What should the agent do?</h2>
              <p className="text-sm leading-relaxed text-muted-foreground">Say it the way you would to a colleague. The agent reads the project, works in its own branch, and reports back here. {busyElsewhere ? `${plural(busyElsewhere, "other conversation")} ${busyElsewhere === 1 ? "is" : "are"} working right now.` : ""}</p>
            </div>
            <Textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault();
                  void start();
                }
              }}
              placeholder="For example: add a dark mode toggle to the settings page and make sure the tests still pass."
              className="min-h-28 text-[15px]"
              autoFocus
              aria-label="What should the agent do"
            />
            <div className="flex flex-wrap gap-1.5">
              {STARTERS.map((s) => (
                <button key={s} type="button" onClick={() => setText(s)} className="rounded-full border bg-muted/40 px-3 py-1 text-xs text-muted-foreground transition-colors hover:border-primary/40 hover:bg-primary/[0.06] hover:text-foreground">
                  {s}
                </button>
              ))}
            </div>
            <div className="flex flex-wrap items-end gap-x-4 gap-y-3">
              <label className="grid gap-1 text-xs font-medium text-muted-foreground">
                Project
                <Select value={project} onValueChange={setProject}>
                  <SelectTrigger className="w-56" aria-label="Project">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={LET_AGENT_DECIDE}>
                      <span className="flex items-center gap-1.5">
                        <SparklesIcon className="size-3.5 text-primary" /> Let the agent decide
                      </span>
                    </SelectItem>
                    {list.map((p) => (
                      <SelectItem key={p.slug} value={p.slug}>
                        {p.slug}
                        {p.brain ? " (brain)" : ""}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </label>
              <label className="grid gap-1 text-xs font-medium text-muted-foreground">
                Model
                <ModelPicker value={model} onChange={setModel} models={models.data?.models ?? []} label="Model for this conversation" />
              </label>
              <label className="flex items-center gap-2 text-sm text-foreground">
                <Switch checked={auto} onCheckedChange={setAuto} aria-label="Approve tool calls automatically" />
                <span className="grid">
                  <span className="font-medium">Approve tools automatically</span>
                  <span className="text-xs text-muted-foreground">{auto ? "Runs commands and edits files without stopping to ask." : "Stops before each risky command and waits for your yes."}</span>
                </span>
              </label>
              <Button type="button" className="ml-auto" onClick={() => void start()} disabled={!text.trim() || busy}>
                {busy ? <LoaderCircleIcon className="animate-spin" /> : <ArrowUpIcon />} Start
                <kbd className="ml-1 hidden rounded border border-primary-foreground/30 px-1 font-mono text-[10px] opacity-80 sm:inline">⌘↵</kbd>
              </Button>
            </div>
          </div>
        </div>
      </section>
      <div className="grid gap-3 sm:grid-cols-3">
        {[
          { icon: GitBranchIcon, title: "Its own branch", body: "Each conversation works in a worktree on pulse/chat-… so your checkout stays clean. Land it when you like." },
          { icon: ShieldCheckIcon, title: "Asks when it must", body: "Questions and risky commands pause the agent until you answer here. Answer from your phone, too." },
          { icon: PaperclipIcon, title: "Shows its work", body: "Every command, file edit and note appears as it happens. Attach files for it to read; its screenshots and reports land in Artifacts." },
        ].map((f, i) => (
          <div key={f.title} className="enter grid gap-1.5 rounded-xl border bg-card/60 px-4 py-3" style={{ animationDelay: 80 + i * 60 + "ms" }}>
            <span className="flex items-center gap-2 text-sm font-semibold text-foreground">
              <f.icon className="size-4 text-primary" /> {f.title}
            </span>
            <p className="text-[13px] leading-relaxed text-muted-foreground">{f.body}</p>
          </div>
        ))}
      </div>
    </div>
  );
}

// ----- one conversation -----------------------------------------------------

interface Loaded {
  conversation: Conversation;
  live: ConversationLive;
  sessionId: string;
  messages: ConversationMessage[];
  steps: Record<string, HarnessStep[]>;
  artifacts: ChatArtifact[];
  changes: ChatChanges | null;
}

function ConversationView({ id, seed, projects, list, onBack, onGone }: { id: string; seed?: Conversation; projects?: ProjectsPayload; list: ReactNode; onBack: () => void; onGone: () => void }) {
  const [d, setD] = useState<Loaded | null>(null);
  const [error, setError] = useState("");
  const dRef = useRef<Loaded | null>(null);
  const inflight = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const goneRef = useRef(onGone);
  useEffect(() => {
    goneRef.current = onGone;
  }, [onGone]);

  // One incremental fetch: messages past what we hold, steps past the last
  // one we hold for the current session. A session change starts the steps
  // over (the rebuilt agent writes a new harness session).
  const pull = useCallback(async () => {
    if (inflight.current) return;
    inflight.current = true;
    try {
      const cur = dRef.current;
      const since = cur ? Math.max(0, maxN(cur.messages)) : 0;
      const after = cur && cur.sessionId ? maxN(cur.steps[cur.sessionId] ?? []) : -1;
      let r = await api<ChatShowResponse>("chat/" + enc(id) + query({ since, after }));
      if (cur && r.session_id && r.session_id !== cur.sessionId && !(cur.steps[r.session_id]?.length)) {
        r = await api<ChatShowResponse>("chat/" + enc(id) + query({ since, after: -1 }));
      }
      const prev = dRef.current;
      const messages = mergeMessages(prev?.messages ?? [], r.messages);
      const steps = { ...(prev?.steps ?? {}) };
      if (r.session_id) steps[r.session_id] = mergeSteps(steps[r.session_id] ?? [], r.steps);
      const next: Loaded = { conversation: r.conversation, live: r.live, sessionId: r.session_id, messages, steps, artifacts: r.artifacts, changes: r.changes };
      dRef.current = next;
      setD(next);
      setError("");
    } catch (e) {
      const t = errorText(e);
      setError(t.error || "The conversation did not load.");
      if (e instanceof ApiError && e.status === 404) goneRef.current();
    } finally {
      inflight.current = false;
    }
  }, [id]);

  // Steps of an older session arrive on demand, when a past turn is opened.
  const loadSession = useCallback(async (sid: string) => {
    const cur = dRef.current;
    if (!cur || cur.steps[sid]?.length) return;
    try {
      const r = await api<HarnessSessionResponse>("harness/sessions/" + enc(sid));
      const now = dRef.current;
      if (!now) return;
      const next = { ...now, steps: { ...now.steps, [sid]: r.steps } };
      dRef.current = next;
      setD(next);
    } catch {
      // The transcript stays readable without the old steps.
    }
  }, []);

  const status = d?.live.status || d?.conversation.status || seed?.live?.status || seed?.status || "";
  const fast = status === "running" || status === "waiting";
  useEffect(() => {
    let stopped = false;
    const tick = async () => {
      if (stopped) return;
      if (typeof document === "undefined" || !document.hidden) await pull();
      if (stopped) return;
      const s = dRef.current?.live.status || "";
      // A working agent streams; a second apart is what the eye follows.
      timer.current = setTimeout(() => void tick(), s === "running" || s === "waiting" ? 1000 : 5000);
    };
    void tick();
    const onVis = () => {
      if (!document.hidden) void pull();
    };
    document.addEventListener("visibilitychange", onVis);
    return () => {
      stopped = true;
      if (timer.current) clearTimeout(timer.current);
      document.removeEventListener("visibilitychange", onVis);
    };
  }, [pull, fast]);

  // Any write answers with the row; take it and pull right away.
  const apply = useCallback(
    (r: { conversation: Conversation }) => {
      const cur = dRef.current;
      if (cur) {
        const next = { ...cur, conversation: r.conversation, live: r.conversation.live ?? cur.live };
        dRef.current = next;
        setD(next);
      }
      invalidate("chat");
      void pull();
    },
    [pull],
  );

  if (error && !d) {
    return (
      <Split
        list={list}
        main={
          <div className="grid gap-3">
            <BackButton onBack={onBack} />
            <ErrorNotice error={error} onRetry={() => void pull()}>
              This conversation did not load.
            </ErrorNotice>
          </div>
        }
      />
    );
  }
  if (!d) {
    return (
      <Split
        list={list}
        main={
          <div className="grid gap-3">
            <BackButton onBack={onBack} />
            <SkeletonRows n={6} />
          </div>
        }
      />
    );
  }
  return <ConversationBody d={d} projects={projects} list={list} onBack={onBack} apply={apply} pull={pull} loadSession={loadSession} error={error} />;
}

function mergeMessages(cur: ConversationMessage[], next: ConversationMessage[]): ConversationMessage[] {
  if (!next.length) return cur;
  const seen = new Set(cur.map((m) => m.n));
  const add = next.filter((m) => !seen.has(m.n));
  if (!add.length) return cur;
  return [...cur, ...add].sort((a, b) => a.n - b.n);
}

function BackButton({ onBack }: { onBack: () => void }) {
  return (
    <Button type="button" variant="ghost" size="sm" className="-ml-2 justify-self-start lg:hidden" onClick={onBack}>
      <ArrowLeftIcon /> All conversations
    </Button>
  );
}

function ConversationBody({ d, projects, list, onBack, apply, pull, loadSession, error }: { d: Loaded; projects?: ProjectsPayload; list: ReactNode; onBack: () => void; apply: (r: { conversation: Conversation }) => void; pull: () => Promise<void>; loadSession: (sid: string) => Promise<void>; error: string }) {
  const c = d.conversation;
  const live = d.live;
  const mood = moodOf(c, live);
  const running = live.status === "running";
  const waiting = live.status === "waiting" && !!live.pending;
  const archived = c.status === "archived";
  const [busy, setBusy] = useState("");
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(c.title);
  const endRef = useRef<HTMLDivElement | null>(null);
  const [stick, setStick] = useState(true);
  const scroller = useRef<HTMLDivElement | null>(null);
  const models = useApi<HarnessModelsResponse>("harness/models", { refresh: 60000 });

  // A file from the Changes list or an artifact opens in the main area, in
  // place of the transcript; the header and the composer stay, so the owner
  // can keep talking while they read. A question takes the transcript back.
  const [view, setView] = useState<ChatView | null>(null);
  const diff = useChatDiff(c.id, d.changes);
  const shownArtifact = view?.kind === "artifact" ? d.artifacts.find((a) => a.name === view.name) : undefined;
  const shownChanges = view?.kind === "changes" && d.changes ? view : null;
  const inTranscript = !shownArtifact && !shownChanges;
  const [sawWaiting, setSawWaiting] = useState(waiting);
  if (waiting !== sawWaiting) {
    setSawWaiting(waiting);
    if (waiting) setView(null);
  }
  useEffect(() => {
    if (inTranscript) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      setView(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [inTranscript]);
  // Coming back lands where the reader left off, not at the top.
  const savedScroll = useRef<number | null>(null);
  const openView = (v: ChatView | null) => {
    if (v && inTranscript) savedScroll.current = scroller.current?.scrollTop ?? null;
    setView(v);
  };
  useLayoutEffect(() => {
    if (!inTranscript || savedScroll.current === null) return;
    if (scroller.current) scroller.current.scrollTop = savedScroll.current;
    savedScroll.current = null;
  }, [inTranscript]);

  // Follow the transcript while the reader sits at the bottom; stop when
  // they scroll up to read something, like a terminal.
  const lastKey = (d.messages.length ? d.messages[d.messages.length - 1].n : 0) + ":" + live.stream.length + ":" + (live.pending?.id ?? 0) + ":" + (d.steps[d.sessionId]?.length ?? 0);
  useEffect(() => {
    if (stick && inTranscript) endRef.current?.scrollIntoView({ block: "end", behavior: "smooth" });
  }, [lastKey, stick, inTranscript]);

  const act = async (what: string, run: () => Promise<{ conversation: Conversation }>, done?: string) => {
    if (busy) return;
    setBusy(what);
    try {
      apply(await run());
      if (done) toast.success(done);
    } catch (e) {
      fail(e, "That did not go through");
    } finally {
      setBusy("");
    }
  };

  const saveTitle = () => {
    const t = title.trim();
    setEditing(false);
    if (!t || t === c.title) {
      setTitle(c.title);
      return;
    }
    void act("title", () => writes.chatTitle(c.id, t));
  };

  const changeModel = (model: string) => {
    if (model === (c.model || "")) return;
    void act("model", () => writes.chatModel(c.id, model), model ? "Switched to " + (models.data?.models.find((m) => m.id === model)?.name || model) + ". It takes effect on the next turn." : "Back to the provider's default model on the next turn.");
  };

  const modelName = c.model ? models.data?.models.find((m) => m.id === c.model)?.name || c.model : "";
  const changeCount = d.changes ? d.changes.commits + d.changes.dirty.length : 0;

  // The model, the auto switch and Stop sit in the composer's bottom row,
  // where the Copilot app keeps them; the header is the title and the state.
  const tools = (
    <>
      <ModelPicker value={c.model || ""} onChange={changeModel} models={models.data?.models ?? []} disabled={archived || busy === "model"} compact label={modelName ? "Model: " + modelName + " - change it for the next turn" : "Model: provider default - pick one for the next turn"} />
      <label className="flex h-7 shrink-0 items-center gap-1.5 rounded-full border border-border/70 bg-background/60 px-2.5 text-xs text-muted-foreground" title={live.auto_approve ? "Tools run without asking" : "Risky tools wait for your yes"}>
        <Switch size="sm" checked={live.auto_approve} disabled={archived || !!busy} onCheckedChange={(on) => void act("auto", () => writes.chatAuto(c.id, on), on ? "Tools now run without asking." : "The agent will ask before risky tools.")} aria-label="Approve tools automatically" />
        Auto
      </label>
      {running || waiting ? (
        <Button type="button" size="sm" variant="outline" className="h-7 shrink-0 rounded-full" onClick={() => void act("cancel", () => writes.chatCancel(c.id))} disabled={!!busy}>
          <CircleStopIcon className="text-bad" /> Stop
        </Button>
      ) : null}
    </>
  );

  const section = (
    <section className="flex min-h-0 min-w-0 flex-col gap-3 lg:flex-1">
      <BackButton onBack={onBack} />
      <header className={cn("enter grid min-w-0 gap-2 rounded-2xl border bg-card px-4 py-3 shadow-card transition-colors", mood.tone === "wait" && "surface-you", running && "border-primary/40")}>
        <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2">
          <PulseMascot mood={mood.pip} size={44} className={cn("shrink-0", running && "float")} title={mood.word} />
          <div className="grid min-w-[14rem] flex-1 gap-0.5">
            {editing ? (
              <form
                className="flex min-w-0 items-center gap-1.5"
                onSubmit={(e) => {
                  e.preventDefault();
                  saveTitle();
                }}
              >
                <Input value={title} onChange={(e) => setTitle(e.target.value)} onBlur={saveTitle} autoFocus aria-label="Conversation title" className="h-8 max-w-md" maxLength={120} />
                <Button type="submit" size="xs" variant="ghost" aria-label="Save title">
                  <CheckIcon />
                </Button>
              </form>
            ) : (
              <button type="button" onClick={() => setEditing(true)} className="group flex min-w-0 items-center gap-1.5 text-left" title="Rename">
                <span className="truncate text-[17px] font-semibold tracking-tight text-foreground" title={c.title}>
                  {c.title || "Untitled"}
                </span>
                <PencilIcon className="size-3.5 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100" aria-hidden />
              </button>
            )}
            <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
              <span className={cn("inline-flex items-center gap-1 font-medium", mood.tone === "wait" ? "text-you-ink" : TONE_TEXT[mood.tone])}>
                {running ? <LoaderCircleIcon className="size-3 animate-spin" /> : <span className={cn("live-dot size-1.5 rounded-full", mood.dot)} aria-hidden />}
                {mood.word}
                {running ? <TurnClock since={live.turn_started_at} /> : null}
              </span>
              <span aria-hidden>·</span>
              {c.project ? (
                <span className="inline-flex min-w-0 items-center gap-1">
                  <ProjectLink slug={c.project} /> {c.branch ? <code className="truncate rounded bg-muted px-1 py-px font-mono text-[11px]">{c.branch}</code> : null}
                </span>
              ) : (
                <span>no project yet</span>
              )}
              {c.turns ? (
                <>
                  <span aria-hidden>·</span>
                  <span>{plural(c.turns, "turn")}</span>
                </>
              ) : null}
              {c.cost ? (
                <>
                  <span aria-hidden>·</span>
                  <span title="model credits used">{num(Math.round(c.cost * 100) / 100)} credits</span>
                </>
              ) : null}
            </p>
          </div>
          {!archived ? (
            <Button type="button" size="sm" variant="ghost" className="h-7 shrink-0 rounded-full" onClick={() => void act("archive", () => writes.chatArchive(c.id), "Archived. The branch stays until you land or delete it.")} disabled={!!busy} title="Archive this conversation">
              <ArchiveIcon />
              <span className="sr-only sm:not-sr-only">Archive</span>
            </Button>
          ) : null}
        </div>
      </header>
      {error ? <Notice tone="warn">{"Live updates paused: " + error}</Notice> : null}
      {archived ? <Notice tone="info">This conversation is archived. Its branch and files stay where they were; start a new conversation to carry on.</Notice> : null}

      {shownArtifact ? (
        <ArtifactViewer id={c.id} artifact={shownArtifact} onClose={() => setView(null)} />
      ) : shownChanges && d.changes ? (
        <ChangesViewer diff={diff} changes={d.changes} view={shownChanges} onClose={() => setView(null)} />
      ) : (
        <div className="relative flex min-h-0 min-w-0 flex-col overflow-hidden rounded-2xl border bg-card shadow-card lg:flex-1">
          <div aria-hidden className="pointer-events-none absolute inset-x-0 top-0 z-10 h-8 bg-gradient-to-b from-card to-transparent" />
          <div
            ref={scroller}
            onScroll={(e) => {
              const el = e.currentTarget;
              setStick(el.scrollHeight - el.scrollTop - el.clientHeight < 80);
            }}
            className="relative max-h-[calc(100dvh-24rem)] min-h-[20rem] min-w-0 flex-1 overflow-y-auto px-3 py-5 sm:px-6 sm:py-6 lg:max-h-none lg:min-h-0"
            role="log"
            aria-live="polite"
            aria-label="Transcript"
          >
            <Transcript d={d} loadSession={loadSession} />
            {running || live.status === "waiting" ? <LiveTurn d={d} waiting={waiting} /> : null}
            {waiting && live.pending ? <PendingCard id={c.id} pending={live.pending} busy={busy} act={act} /> : null}
            {live.queued.length ? (
              <ul className="mx-auto mt-3 grid w-full max-w-3xl gap-1" aria-label="Queued messages">
                {live.queued.map((q, i) => (
                  <li key={i} className="flex min-w-0 items-center gap-2 rounded-lg border border-dashed px-2.5 py-1.5 text-xs text-muted-foreground">
                    <LoaderCircleIcon className="size-3 shrink-0 animate-spin" /> <span className="shrink-0">Queued:</span> <span className="min-w-0 truncate text-foreground">{q}</span>
                  </li>
                ))}
              </ul>
            ) : null}
            <div ref={endRef} />
          </div>
          {!stick ? (
            <button type="button" onClick={() => setStick(true)} className="pop absolute bottom-3 left-1/2 -translate-x-1/2 rounded-full border bg-card px-3 py-1 text-xs font-medium shadow-pop hover:bg-muted/60">
              <ChevronDownIcon className="mr-1 inline size-3" aria-hidden /> Jump to the latest
            </button>
          ) : null}
        </div>
      )}

      <Composer conversation={c} live={live} disabled={archived} tools={tools} onSent={(r) => apply(r)} />
    </section>
  );

  return <Split list={list} main={section} fill panel={<SidePanel d={d} projects={projects} apply={apply} pull={pull} busy={busy} setBusy={setBusy} changeCount={changeCount} diff={diff} view={view} onView={openView} />} />;
}

function TurnClock({ since }: { since: string }) {
  const now = useNow(1000);
  const t = Date.parse(since);
  if (!Number.isFinite(t)) return null;
  return <span className="tnum text-muted-foreground">{span(Math.max(0, Math.round((now - t) / 1000)))}</span>;
}

// ----- transcript -----------------------------------------------------------

function Transcript({ d, loadSession }: { d: Loaded; loadSession: (sid: string) => Promise<void> }) {
  if (!d.messages.length) {
    return (
      <EmptyState icon={<PulseMascot mood="busy" size={56} decorative />} hint="The first reply appears here.">
        Starting up…
      </EmptyState>
    );
  }
  // While a question waits, its card below carries the words; the transcript
  // row for it comes back with the answer.
  const last = d.messages[d.messages.length - 1];
  const pending = d.live.pending;
  const rows = pending?.kind === "question" && last.role === "question" && last.content === pending.question ? d.messages.slice(0, -1) : d.messages;
  return (
    <ol className="mx-auto grid w-full max-w-3xl min-w-0 gap-6" role="list">
      {rows.map((m, i) => (
        <MessageRow key={m.n} m={m} status={messageStatus(m, rows.slice(i + 1))} d={d} loadSession={loadSession} />
      ))}
    </ol>
  );
}

// The transcript reads like the Copilot app's: what you wrote on the right
// in a quiet bubble, what the agent did and said on the left as plain text
// and one line per tool call - no cards, no avatars, the time on hover.
function MessageRow({ m, status, d, loadSession }: { m: ConversationMessage; status: MessageStatus; d: Loaded; loadSession: (sid: string) => Promise<void> }) {
  const role = m.role;
  const when = (
    <span className="text-[11px] text-muted-foreground opacity-0 transition-opacity group-hover/msg:opacity-100 motion-reduce:transition-none">
      <When d={m.at} />
    </span>
  );
  if (role === "user" || role === "answer") {
    const files = chatAttachments(m);
    return (
      <li className="enter group/msg grid min-w-0 justify-items-end gap-1">
        <div className={cn("grid min-w-0 max-w-[85%] gap-2 rounded-2xl rounded-br-md px-4 py-2.5 text-[15px] leading-relaxed", role === "answer" ? "border border-primary/30 bg-primary/[0.06] text-foreground" : "bg-muted text-foreground")}>
          <div className="whitespace-pre-wrap anywhere">
            {role === "answer" ? <span className="mr-1 text-xs font-medium text-primary">You answered:</span> : null}
            {m.content}
          </div>
          <AttachmentChips id={d.conversation.id} list={files} onUser={false} />
        </div>
        {when}
        <p className="text-[11px] text-muted-foreground">{status === "answered" ? "Answered" : "Sent"}</p>
      </li>
    );
  }
  if (role === "assistant") {
    return (
      <li className="enter group/msg grid min-w-0 gap-3">
        <TurnSteps m={m} d={d} loadSession={loadSession} />
        <div className="min-w-0 text-[15px] leading-relaxed">
          <Markdown source={m.content} className="[&_pre]:whitespace-pre-wrap" />
        </div>
        {when}
      </li>
    );
  }
  if (role === "question") {
    const choices = Array.isArray(m.extra?.choices) ? (m.extra.choices as string[]) : [];
    return (
      <li className="enter grid min-w-0 gap-3">
        <TurnSteps m={m} d={d} loadSession={loadSession} />
        <div className="surface-you max-w-[85%] rounded-2xl rounded-tl-md border px-4 py-2.5 text-[15px] leading-relaxed">
          <span className="mr-1 text-xs font-semibold text-you-ink">Asked you:</span>
          <span className="whitespace-pre-wrap anywhere">{m.content}</span>
          {choices.length ? <span className="mt-1 block text-xs text-muted-foreground">Choices: {choices.join(" / ")}</span> : null}
        </div>
      </li>
    );
  }
  if (role === "approval") {
    const decision = String(m.extra?.decision ?? "");
    const ok = decision === "allow" || decision === "always";
    return (
      <li className="enter flex min-w-0 items-center gap-2.5 px-1.5 text-[13px] text-muted-foreground">
        {ok ? <ShieldCheckIcon className="size-3.5 shrink-0 text-ok" /> : <CircleAlertIcon className="size-3.5 shrink-0 text-bad" />}
        <span className="min-w-0 truncate">
          <span className="font-medium text-foreground">{ok ? (decision === "always" ? "Allowed, always" : "Allowed") : "Denied"}</span> <code className="rounded bg-muted px-1 font-mono text-[11px]">{String(m.extra?.tool ?? "")}</code> {m.content}
          {m.extra?.reason ? <span> - {String(m.extra.reason)}</span> : null}
        </span>
      </li>
    );
  }
  // A note: the hub's own voice (session ended, moved into a project, stopped).
  const moved = m.extra?.project ? String(m.extra.project) : "";
  return (
    <li className="enter grid min-w-0 gap-3">
      <TurnSteps m={m} d={d} loadSession={loadSession} />
      <div className="flex min-w-0 items-start gap-2.5 px-1.5 text-[13px] leading-relaxed text-muted-foreground">
        {moved ? <FolderGitIcon className="mt-0.5 size-3.5 shrink-0 text-primary" /> : <CircleAlertIcon className="mt-0.5 size-3.5 shrink-0" />}
        <span className="min-w-0 whitespace-pre-wrap anywhere">
          {m.content}
          {moved ? (
            <>
              {" "}
              <Link href={hrefs.project(moved)} className="text-primary underline-offset-4 hover:underline">
                Open {moved}
              </Link>
            </>
          ) : null}
        </span>
      </div>
    </li>
  );
}

// What the agent did on the way to this message, in the transcript itself.
// The steps of the session the conversation runs in are already here; an
// older session's load on request, one line until they do.
function TurnSteps({ m, d, loadSession }: { m: ConversationMessage; d: Loaded; loadSession: (sid: string) => Promise<void> }) {
  const r = stepRange(m);
  const [asked, setAsked] = useState(false);
  if (!r) return null;
  const all = d.steps[r.session];
  if (!all) {
    if (asked) return <SkeletonRows n={2} />;
    return (
      <button
        type="button"
        onClick={() => {
          setAsked(true);
          void loadSession(r.session);
        }}
        className="flex w-fit items-center gap-2.5 rounded-md px-1.5 py-1 text-[13px] text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <ChevronRightIcon className="size-3.5 shrink-0" aria-hidden /> Show what it did
      </button>
    );
  }
  const steps = turnSteps(all, r.from, r.to, m.role === "assistant" ? m.content : "");
  if (!steps.length) return null;
  return <Activity steps={steps} />;
}

// The running turn, in the order it happens: what the agent wrote, the
// tools it ran (one line each), more words - and last the text still being
// written, with a caret. When it waits on a question, the card below
// carries the question itself.
function LiveTurn({ d, waiting }: { d: Loaded; waiting: boolean }) {
  const live = d.live;
  const all = d.steps[d.sessionId];
  const question = waiting && live.pending?.kind === "question" ? live.pending.question : "";
  const steps = useMemo(() => {
    const base = turnSteps(all ?? [], live.turn_from, null, "");
    if (!question) return base;
    // The words that led to the question often repeat it; the card below
    // carries the question, so the last segment is cut where it starts.
    for (let i = base.length - 1; i >= 0; i--) {
      const s = base[i];
      if (s.kind !== "assistant" || !s.content.trim()) continue;
      const cut = beforeQuestion(s.content, question);
      if (cut === s.content) break;
      const copy = base.slice();
      copy[i] = { ...s, content: cut };
      return copy;
    }
    return base;
  }, [all, live.turn_from, question]);
  const last = steps.length ? steps[steps.length - 1] : null;
  const tail = streamTail(live.stream, steps);
  const stream = question ? beforeQuestion(tail, question) : tail;
  return (
    <div className="enter mx-auto mt-6 grid w-full max-w-3xl min-w-0 gap-3">
      {steps.length ? <Activity steps={steps} live={!waiting} /> : null}
      {stream.trim() ? (
        <div className="min-w-0 text-[15px] leading-relaxed">
          <Markdown source={stream} className="[&_pre]:whitespace-pre-wrap" />
          {waiting ? null : <span className="ml-0.5 inline-block h-4 w-1.5 translate-y-0.5 animate-pulse rounded-sm bg-primary/70" aria-hidden />}
        </div>
      ) : null}
      <div className={cn("flex min-w-0 items-center gap-2.5 px-1.5 text-[13px]", waiting ? "text-you-ink" : "text-muted-foreground")}>
        {waiting ? <ShieldQuestionIcon className="size-3.5 shrink-0" /> : <TypingDots className="shrink-0 text-primary" />}
        <span className="min-w-0 truncate">{waiting ? "Paused - it needs you (below)" : last && !stream.trim() ? lastStepWords(last) : "Working…"}</span>
        <TurnClock since={live.turn_started_at} />
      </div>
    </div>
  );
}

// What the model said before it asked: a reply that repeats the question
// (and lists the choices after it) would show twice beside the card, so
// the stream is cut where the question starts.
function beforeQuestion(stream: string, question: string): string {
  const q = question.trim();
  if (!q) return stream;
  const i = stream.indexOf(q);
  if (i >= 0) return stream.slice(0, i).trimEnd();
  const first = q.split(/(?<=[.?!])\s/)[0];
  if (first && first.length >= 24) {
    const j = stream.indexOf(first);
    if (j >= 0) return stream.slice(0, j).trimEnd();
  }
  return stream;
}

function lastStepWords(s: HarnessStep): string {
  switch (s.kind) {
    case "command":
      return "Running: " + ((s.title ?? "").trim() || s.command || s.content);
    case "observation":
      return "Reading the result…";
    case "plan":
      return "Planning: " + ((s.content || "").split("\n")[0] || "");
    case "subagent":
      return "Asked a helper: " + (s.command || s.content);
    case "assistant":
      return "Thinking…";
    case "note":
      return s.content;
    default:
      return (s.kind + ": " + (s.command || s.content)).slice(0, 160);
  }
}

// ----- the agent waits ------------------------------------------------------

function PendingCard({ id, pending, busy, act }: { id: string; pending: ChatPending; busy: string; act: (what: string, run: () => Promise<{ conversation: Conversation }>, done?: string) => Promise<void> }) {
  const [text, setText] = useState("");
  const [reason, setReason] = useState("");
  const [denying, setDenying] = useState(false);
  if (pending.kind === "question") {
    return (
      <div className="mx-auto mt-4 w-full max-w-3xl min-w-0">
        <div className="pop surface-you grid min-w-0 gap-3 rounded-2xl border-2 px-4 py-3 shadow-card" role="group" aria-label="The agent asked a question">
          <div className="flex items-start gap-2">
            <ShieldQuestionIcon className="mt-0.5 size-4 shrink-0 text-you-ink" />
            <p className="min-w-0 text-[15px] leading-relaxed font-medium whitespace-pre-wrap text-foreground anywhere">{pending.question}</p>
          </div>
          {pending.choices.length ? (
            <div className="grid min-w-0 gap-1.5" role="group" aria-label="Choices">
              {pending.choices.map((ch) => (
                <Button key={ch} type="button" size="sm" variant="outline" disabled={!!busy} onClick={() => void act("answer", () => writes.chatAnswer(id, ch))} className="pop h-auto min-h-8 w-full min-w-0 justify-start py-1.5 text-left whitespace-normal anywhere">
                  {ch}
                </Button>
              ))}
            </div>
          ) : null}
          <form
            className="flex min-w-0 gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              const t = text.trim();
              if (!t) return;
              void act("answer", () => writes.chatAnswer(id, t)).then(() => setText(""));
            }}
          >
            <Input value={text} onChange={(e) => setText(e.target.value)} placeholder={pending.choices.length ? "Or answer in your own words…" : "Your answer…"} aria-label="Your answer" autoFocus />
            <Button type="submit" size="default" disabled={!text.trim() || !!busy}>
              {busy === "answer" ? <LoaderCircleIcon className="animate-spin" /> : <ArrowUpIcon />} Answer
            </Button>
          </form>
        </div>
      </div>
    );
  }
  return (
    <div className="mx-auto mt-4 w-full max-w-3xl min-w-0">
      <div className="pop surface-you grid min-w-0 gap-3 rounded-2xl border-2 px-4 py-3 shadow-card" role="group" aria-label="The agent needs an approval">
        <div className="flex items-start gap-2">
          <ShieldCheckIcon className="mt-0.5 size-4 shrink-0 text-you-ink" />
          <div className="grid min-w-0 gap-1">
            <p className="text-[15px] leading-relaxed font-medium text-foreground">
              May it run <code className="rounded bg-muted px-1 font-mono text-[13px]">{pending.tool}</code>?
            </p>
            <p className="text-sm leading-relaxed whitespace-pre-wrap text-foreground anywhere">{pending.summary}</p>
            {pending.reason ? <p className="text-[13px] text-muted-foreground">{pending.reason}</p> : null}
          </div>
        </div>
        {pending.detail ? <pre className="max-h-56 min-w-0 overflow-auto rounded-xl bg-zinc-950 px-3 py-2.5 font-mono text-[12.5px] leading-relaxed whitespace-pre-wrap text-zinc-100 anywhere dark:bg-black">{pending.detail}</pre> : null}
        {denying ? (
          <form
            className="flex min-w-0 gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              void act("approve", () => writes.chatApprove(id, "deny", reason.trim())).then(() => {
                setDenying(false);
                setReason("");
              });
            }}
          >
            <Input value={reason} onChange={(e) => setReason(e.target.value)} placeholder="Why not? (the agent reads this)" aria-label="Reason" autoFocus />
            <Button type="submit" variant="destructive" disabled={!!busy}>
              Deny
            </Button>
            <Button type="button" variant="ghost" onClick={() => setDenying(false)}>
              Back
            </Button>
          </form>
        ) : (
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" disabled={!!busy} onClick={() => void act("approve", () => writes.chatApprove(id, "allow"))}>
              <CheckIcon /> Allow once
            </Button>
            {pending.always ? (
              <Button type="button" variant="outline" disabled={!!busy} onClick={() => void act("approve", () => writes.chatApprove(id, "always"), "Allowed for the rest of this conversation.")} title={pending.always}>
                Always allow
              </Button>
            ) : null}
            <Button type="button" variant="ghost" disabled={!!busy} onClick={() => setDenying(true)} className="text-bad hover:text-bad">
              <XIcon /> Deny
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}

// ----- composer -------------------------------------------------------------

// Composer: the text on top, the tools underneath - attach, the model, the
// auto switch and Stop on the left, send on the right - as the Copilot app
// lays it out.
function Composer({ conversation, live, disabled, tools, onSent }: { conversation: Conversation; live: ConversationLive; disabled: boolean; tools?: ReactNode; onSent: (r: { conversation: Conversation }) => void }) {
  const [text, setText] = useState("");
  const [picked, setPicked] = useState<Picked[]>([]);
  const [busy, setBusy] = useState("");
  const [over, setOver] = useState(false);
  const fileInput = useRef<HTMLInputElement | null>(null);
  const running = live.status === "running" || live.status === "waiting";
  const canSend = (!!text.trim() || picked.length > 0) && !busy && !disabled;

  const addFiles = (files: FileList | File[] | null | undefined) => {
    if (disabled) return;
    setPicked((cur) => pickFiles(cur, files));
  };

  // Files go up one at a time, then the message names what the hub kept
  // (a name the hub renumbered comes back renumbered).
  const send = async () => {
    if (!canSend) return;
    const t = text.trim();
    const files = picked;
    setBusy(files.length ? "upload" : "send");
    try {
      const names: string[] = [];
      for (let i = 0; i < files.length; i++) {
        setBusy("upload " + (i + 1) + "/" + files.length);
        const a = await chatUpload(conversation.id, files[i].file);
        names.push(a.name);
      }
      setBusy("send");
      const r = await writes.chatSend(conversation.id, t, names);
      setText("");
      setPicked([]);
      onSent(r);
    } catch (e) {
      fail(e, "The message did not send");
    } finally {
      setBusy("");
    }
  };

  const uploading = busy.startsWith("upload");
  return (
    <form
      className="grid min-w-0 shrink-0 gap-1.5"
      onSubmit={(e) => {
        e.preventDefault();
        void send();
      }}
      onDragOver={(e) => {
        if (disabled || !e.dataTransfer.types.includes("Files")) return;
        e.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        if (disabled || !e.dataTransfer.files.length) return;
        e.preventDefault();
        setOver(false);
        addFiles(e.dataTransfer.files);
      }}
    >
      <div className={cn("grid min-w-0 gap-2 rounded-2xl border bg-card p-2 shadow-card transition-colors focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/25", over && "border-primary ring-3 ring-primary/25")}>
        {picked.length ? (
          <ul className="flex min-w-0 flex-wrap gap-1.5 px-1 pt-1" aria-label="Files to attach">
            {picked.map((p) => {
              const Icon = fileIcon(p.file.name);
              return (
                <li key={p.key} className="pop flex min-w-0 max-w-64 items-center gap-1.5 rounded-lg border bg-muted/40 py-1 pr-1 pl-2 text-xs">
                  <Icon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
                  <span className="min-w-0 truncate font-medium text-foreground" title={p.file.name}>
                    {p.file.name}
                  </span>
                  <span className="shrink-0 text-muted-foreground tnum">{bytesWord(p.file.size)}</span>
                  <button type="button" onClick={() => setPicked((cur) => cur.filter((x) => x.key !== p.key))} disabled={!!busy} className="rounded-md p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground" aria-label={"Remove " + p.file.name}>
                    <XIcon className="size-3" />
                  </button>
                </li>
              );
            })}
          </ul>
        ) : null}
        <Textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
          onPaste={(e) => {
            const files = Array.from(e.clipboardData?.files ?? []);
            if (!files.length) return;
            e.preventDefault();
            addFiles(files);
          }}
          placeholder={disabled ? "This conversation is archived." : running ? "Type the next thing; it goes after this turn." : picked.length ? "Say what to do with the files, or just send them." : "Tell the agent what to do next…"}
          disabled={disabled}
          rows={1}
          className="min-h-10 max-h-48 resize-none border-0 bg-transparent px-2.5 py-2 text-[15px] shadow-none focus-visible:ring-0 dark:bg-transparent"
          aria-label="Message"
        />
        <div className="flex min-w-0 items-center gap-1.5">
          <input
            ref={fileInput}
            type="file"
            multiple
            className="sr-only"
            tabIndex={-1}
            aria-hidden
            onChange={(e) => {
              addFiles(e.target.files);
              e.target.value = "";
            }}
          />
          <Button type="button" size="icon-sm" variant="ghost" onClick={() => fileInput.current?.click()} disabled={disabled || !!busy} aria-label="Attach files" title="Attach files (up to 8 MB each). You can also drop or paste them." className="shrink-0 rounded-full text-muted-foreground hover:text-foreground">
            <PaperclipIcon />
          </Button>
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">{tools}</div>
          <Button type="submit" size="icon" disabled={!canSend} aria-label={running ? "Queue message" : "Send"} className="shrink-0 rounded-xl">
            {busy ? <LoaderCircleIcon className="animate-spin" /> : <ArrowUpIcon />}
          </Button>
        </div>
      </div>
      <p className="flex min-w-0 flex-wrap items-center gap-x-2 px-2 text-[11px] text-muted-foreground">
        {uploading ? (
          <span className="text-primary">Uploading {busy.slice("upload ".length) || "…"}</span>
        ) : over ? (
          <span className="text-primary">Drop to attach</span>
        ) : (
          <span>Enter sends, Shift+Enter for a new line. Drop or paste files to attach them.{running ? " The agent is busy: your message waits in line." : ""}</span>
        )}
        {conversation.cwd ? (
          <span className="min-w-0 truncate" title={conversation.cwd}>
            Working in <code className="font-mono">{conversation.cwd}</code>.
          </span>
        ) : null}
      </p>
    </form>
  );
}

// ----- side panel: project, changes, artifacts ------------------------------

function SidePanel({ d, projects, apply, pull, busy, setBusy, changeCount, diff, view, onView }: { d: Loaded; projects?: ProjectsPayload; apply: (r: { conversation: Conversation }) => void; pull: () => Promise<void>; busy: string; setBusy: (s: string) => void; changeCount: number; diff: ChatDiff; view: ChatView | null; onView: (v: ChatView | null) => void }) {
  const c = d.conversation;
  const ch = d.changes;
  const [attachTo, setAttachTo] = useState("");
  const [tab, setTab] = useState<string>(() => (ch && changeCount ? "changes" : c.project ? "artifacts" : "project"));
  const list = (projects?.projects ?? []).filter((p) => p.status !== "inactive");
  const archived = c.status === "archived";

  const attach = async () => {
    if (!attachTo || busy) return;
    setBusy("attach");
    try {
      const r = await writes.chatAttach(c.id, attachTo);
      apply({ conversation: r.conversation });
      toast.success(r.attached.created ? "Attached. The agent moves into " + attachTo + " after this turn." : "Attached to " + attachTo + ".");
      setAttachTo("");
      setTab("changes");
    } catch (e) {
      fail(e, "Could not attach the project");
    } finally {
      setBusy("");
    }
  };

  const askToLand = async () => {
    if (!ch || busy) return;
    setBusy("land");
    try {
      apply(await writes.chatSend(c.id, "Please land this branch now: run `pulse land --project " + ch.project + " --branch " + ch.branch + "` and tell me what happened. If the verify step fails, fix it first and try again."));
      toast.success("Asked the agent to land the branch.");
    } catch (e) {
      fail(e, "The request did not send");
    } finally {
      setBusy("");
    }
  };

  const count = (n: number) => (n ? <span className="ml-1 hidden rounded-full bg-muted px-1.5 text-[10px] font-semibold text-muted-foreground tnum group-data-[state=active]/tab:bg-primary/10 group-data-[state=active]/tab:text-primary xl:inline">{num(n)}</span> : null);
  const plan = latestPlan(d);

  return (
    <section className="enter grid min-w-0 content-start gap-3 rounded-2xl border bg-card px-3 py-3 shadow-card" style={{ animationDelay: "60ms" }} aria-label="About this conversation">
      {plan ? <PlanList step={plan} compact /> : null}
      <Tabs value={tab} onValueChange={setTab} className="min-w-0 gap-3">
        <TabsList className="w-full">
          {/* Three tabs in a side column (14-22rem): tight padding, small icons, nothing wraps. */}
          <TabsTrigger value="changes" className="group/tab min-w-0 flex-auto gap-1 px-1.5 text-[13px] [&_svg]:size-3.5" disabled={!ch}>
            <GitBranchIcon /> Changes{count(changeCount)}
          </TabsTrigger>
          <TabsTrigger value="artifacts" className="group/tab min-w-0 flex-auto gap-1 px-1.5 text-[13px] [&_svg]:size-3.5">
            <PaperclipIcon /> Artifacts{count(d.artifacts.length)}
          </TabsTrigger>
          <TabsTrigger value="project" className="group/tab min-w-0 flex-auto gap-1 px-1.5 text-[13px] [&_svg]:size-3.5">
            <FolderGitIcon /> Project
          </TabsTrigger>
        </TabsList>

        <TabsContent value="changes" className="grid min-w-0 content-start gap-2.5">
          {ch ? (
            <>
              <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 px-1 text-xs text-muted-foreground">
                <code className="min-w-0 truncate rounded bg-muted px-1 py-px font-mono text-[11px] text-foreground" title={ch.branch}>
                  {ch.branch}
                </code>
                <span>{ch.commits ? plural(ch.commits, "commit") : "no commits yet"}</span>
                {ch.dirty.length ? <span className="font-medium text-you-ink">{plural(ch.dirty.length, "file")} not committed</span> : null}
              </div>
              {!ch.commits && !ch.dirty.length ? (
                <EmptyState icon={<GitBranchIcon className="size-[18px]" />} hint="The agent's edits and commits on its branch show up here, file by file; pick one to read the diff.">
                  Nothing changed yet
                </EmptyState>
              ) : (
                <ChangeList diff={diff} view={view} onView={onView} />
              )}
              {(ch.commits || ch.dirty.length) && !archived ? (
                <div className="flex flex-wrap items-center gap-1.5">
                  <ReviewAllButton active={view?.kind === "changes" && view.file === undefined} onClick={() => onView({ kind: "changes" })} />
                  {ch.commits ? (
                    <Button type="button" size="sm" onClick={() => void askToLand()} disabled={!!busy} title={"Asks the agent to run pulse land --project " + ch.project + " --branch " + ch.branch}>
                      {busy === "land" ? <LoaderCircleIcon className="animate-spin" /> : <CheckIcon />} Ask to land
                    </Button>
                  ) : null}
                </div>
              ) : null}
            </>
          ) : (
            <p className="px-1 text-[13px] leading-relaxed text-muted-foreground">Changes appear once the conversation has a project and a branch.</p>
          )}
        </TabsContent>

        <TabsContent value="artifacts" className="grid min-w-0 content-start gap-2">
          {d.artifacts.length ? (
            <ArtifactList id={c.id} artifacts={d.artifacts} view={view} onView={onView} />
          ) : (
            <EmptyState icon={<PaperclipIcon className="size-[18px]" />} hint="It knows the folder as $PULSE_ARTIFACTS.">
              Files the agent saves for you - screenshots, reports, pages - show up here.
            </EmptyState>
          )}
          <button type="button" onClick={() => void pull()} className="flex items-center gap-1 justify-self-start px-1 text-[11px] text-muted-foreground underline-offset-4 hover:underline">
            <RefreshCwIcon className="size-3" /> Refresh
          </button>
        </TabsContent>

        <TabsContent value="project" className="grid min-w-0 content-start gap-2 px-1">
          {c.project ? (
            <div className="grid gap-1.5 text-sm">
              <ProjectLink slug={c.project} />
              {c.branch ? (
                <span className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
                  <GitBranchIcon className="size-3.5 shrink-0" /> <code className="truncate font-mono">{c.branch}</code>
                </span>
              ) : null}
              {c.cwd ? (
                <span className="truncate text-xs text-muted-foreground" title={c.cwd}>
                  {c.cwd}
                </span>
              ) : null}
              <span className="text-xs text-muted-foreground">
                Model: <span className="text-foreground">{c.model || "provider default"}</span>
              </span>
            </div>
          ) : (
            <div className="grid gap-2">
              <p className="text-[13px] leading-relaxed text-muted-foreground">No project yet. The agent reads your projects and attaches one itself, or pick it here.</p>
              {!archived ? (
                <div className="flex min-w-0 gap-1.5">
                  <Select value={attachTo || undefined} onValueChange={setAttachTo}>
                    <SelectTrigger className="min-w-0 flex-1" aria-label="Project to attach">
                      <SelectValue placeholder="Pick a project" />
                    </SelectTrigger>
                    <SelectContent>
                      {list.map((p) => (
                        <SelectItem key={p.slug} value={p.slug}>
                          {p.slug}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button type="button" size="default" variant="outline" disabled={!attachTo || !!busy} onClick={() => void attach()}>
                    {busy === "attach" ? <LoaderCircleIcon className="animate-spin" /> : "Attach"}
                  </Button>
                </div>
              ) : null}
            </div>
          )}
        </TabsContent>
      </Tabs>
    </section>
  );
}
