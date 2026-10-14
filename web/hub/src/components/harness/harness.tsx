"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import {
  ActivityIcon,
  ArrowLeftIcon,
  BanIcon,
  BookOpenIcon,
  BotIcon,
  CheckIcon,
  CircleAlertIcon,
  CircleCheckIcon,
  CircleHelpIcon,
  CircleIcon,
  CircleXIcon,
  ClipboardIcon,
  CopyIcon,
  DownloadIcon,
  FileCodeIcon,
  FilePenIcon,
  FilePlusIcon,
  FilesIcon,
  GitForkIcon,
  GlobeIcon,
  InfoIcon,
  ListChecksIcon,
  LoaderCircleIcon,
  PlayIcon,
  PlugIcon,
  ScrollTextIcon,
  SearchIcon,
  TerminalIcon,
  TimerOffIcon,
  TriangleAlertIcon,
  WebhookIcon,
  XIcon,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Markdown } from "@/components/pulse/markdown";
import { Disclosure, EmptyState, ErrorNotice, Kv, Notice, PageTitle, SkeletonPage, SkeletonRows } from "@/components/pulse/panel";
import { ProjectLink, When } from "@/components/pulse/who";
import type { HarnessModel, HarnessModelsResponse, HarnessSession, HarnessSessionResponse, HarnessSessionStatus, HarnessSessionsResponse, HarnessStep, ProjectsPayload } from "@/lib/api";
import { api, apiHref, enc, errorText, query, writes } from "@/lib/api";
import { num, parseDate, plural, sentence, span } from "@/lib/format";
import { statusWord } from "@/lib/words";
import { hrefs } from "@/lib/nav";
import { invalidate, useApi, useLiveEvents, useNow } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// Runs: every agent run as the harness saw it. The list on the left says
// what each run was asked and how it ended; the transcript on the right
// (sticky on desktop, in place of the list on a phone) reads the run step
// by step - what the model said, the commands it ran, what came back.

// Run states statusWord does not know yet (see the report's requests).
const RUN_WORD: Partial<Record<HarnessSessionStatus, string>> = { format_error: "Kept answering in the wrong format", not_logged_in: "Not signed in" };

const STATUSES: { value: string; label: string }[] = [{ value: "all", label: "All statuses" }, ...(["running", "submitted", "step_limit", "time_limit", "format_error", "provider_error", "not_logged_in", "cancelled", "lost"] as HarnessSessionStatus[]).map((s) => ({ value: s, label: runWord(s) }))];

function runWord(status: string): string {
  return RUN_WORD[status as HarnessSessionStatus] ?? statusWord(status);
}

function compact(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1).replace(/\.0$/, "") + "m";
  if (n >= 1_000) return (n / 1_000).toFixed(n >= 10_000 ? 0 : 1).replace(/\.0$/, "") + "k";
  return num(n);
}

function seconds(start?: string, end?: string, now = Date.now()): number {
  const s = parseDate(start)?.getTime();
  if (!s) return 0;
  const e = parseDate(end)?.getTime() || now;
  return Math.max(1, (e - s) / 1000);
}

// How long a run took, or has been going. A run PULSE lost has no end; it
// ends where it was last heard from.
function runTime(s: HarnessSession, now: number): string {
  if (!s.started_at) return "";
  if (s.status === "running") return "Running for " + span(seconds(s.started_at, "", now));
  return "Took " + span(seconds(s.started_at, s.ended_at || s.updated_at, now));
}

function exitKey(session: Pick<HarnessSession, "status" | "exit_status">): string {
  return session.status === "running" ? "running" : session.exit_status || session.status;
}

type RunTone = "run" | "ok" | "warn" | "bad" | "neutral";

const RUN_ICON_TONE: Record<RunTone, string> = { run: "text-primary", ok: "text-ok", warn: "text-warn", bad: "text-bad", neutral: "text-muted-foreground" };

function runState(session: Pick<HarnessSession, "status" | "exit_status" | "cancel_requested_at">): { word: string; tone: RunTone; Icon: React.ComponentType<{ className?: string }>; spin?: boolean; raw: string } {
  const key = exitKey(session);
  if (key === "running" && session.cancel_requested_at) return { word: "Cancelling", tone: "warn", Icon: LoaderCircleIcon, spin: true, raw: "cancelling" };
  const word = runWord(key);
  switch (key) {
    case "running":
      return { word, tone: "run", Icon: LoaderCircleIcon, spin: true, raw: key };
    case "submitted":
      return { word, tone: "ok", Icon: CircleCheckIcon, raw: key };
    case "step_limit":
    case "format_error":
      return { word, tone: "warn", Icon: CircleAlertIcon, raw: key };
    case "time_limit":
      return { word, tone: "warn", Icon: TimerOffIcon, raw: key };
    case "provider_error":
    case "not_logged_in":
      return { word, tone: "bad", Icon: CircleXIcon, raw: key };
    case "cancelled":
      return { word, tone: "neutral", Icon: BanIcon, raw: key };
    case "lost":
      return { word, tone: "neutral", Icon: CircleHelpIcon, raw: key };
    default:
      return { word, tone: "neutral", Icon: InfoIcon, raw: key };
  }
}

function RunStatus({ session, className }: { session: Pick<HarnessSession, "status" | "exit_status" | "cancel_requested_at">; className?: string }) {
  const st = runState(session);
  return (
    <span className={cn("inline-flex min-w-0 items-center gap-1.5 font-medium text-foreground", className)} title={st.word.toLowerCase() !== st.raw.replace(/_/g, " ") ? st.raw : undefined}>
      <st.Icon className={cn("size-4 shrink-0", RUN_ICON_TONE[st.tone], st.spin && "motion-safe:animate-spin")} aria-hidden />
      <span className="min-w-0 anywhere">{st.word}</span>
    </span>
  );
}

// The sentence the last step of a transcript ends on.
function endSentence(session: HarnessSession, fallback: string): string {
  switch (exitKey(session)) {
    case "submitted":
      return "Finished and handed in its work.";
    case "step_limit":
      return "Stopped early: it used every step it was allowed.";
    case "time_limit":
      return "Stopped early: it ran out of time.";
    case "format_error":
      return "Stopped early: the model kept answering in the wrong format.";
    case "provider_error":
      return "Stopped early: the model provider returned an error.";
    case "not_logged_in":
      return "Stopped early: PULSE is not signed in to the model provider.";
    case "cancelled":
      return "Cancelled before it finished.";
    case "lost":
      return "PULSE lost track of this run before it ended.";
    default:
      return fallback ? fallback.charAt(0).toUpperCase() + fallback.slice(1) : "Ended.";
  }
}

function executorName(executor: string | undefined): string {
  return (executor || "").trim();
}

// The fact names remote machines only; every run has some executor.
function remoteExecutor(executor: string | undefined): string {
  const e = executorName(executor);
  return e && e !== "local" ? e : "";
}

const machineWord = (e: string) => (e === "local" ? "This machine" : e);

function updateSearch(pathname: string, router: ReturnType<typeof useRouter>, next: Record<string, string>) {
  const clean = Object.fromEntries(Object.entries(next).filter(([, v]) => v && v !== "all"));
  router.replace(pathname + query(clean), { scroll: false });
}

// The model by its display name; the facts a curious owner may want sit in the tooltip.
function modelInfo(model: string, models: HarnessModel[]): { name: string; tip: string } {
  const m = models.find((x) => x.id === model);
  if (!m) return { name: model || "Unknown model", tip: model ? "Model " + model : "" };
  const context = m.context_window >= 1000 ? Math.round(m.context_window / 1000) + "k" : String(m.context_window);
  const tip = [
    m.id + (m.vendor ? ", by " + m.vendor : ""),
    m.premium ? "Each request counts as " + (m.premium === 1 ? "1 premium request" : num(m.premium) + " premium requests") : "No premium requests",
    m.context_window ? "Reads up to " + context + " tokens at once" : "",
    m.tool_calls ? "" : "Cannot call tools",
    m.preview ? "Preview model" : "",
  ].filter(Boolean);
  return { name: m.name || m.id, tip: tip.join(". ") + "." };
}

function jobHref(s: Pick<HarnessSession, "project" | "job_id">): string {
  if (s.project && /^TASK-/.test(s.job_id)) return hrefs.task(s.project, s.job_id);
  return hrefs.tasks(s.project ? { project: s.project } : {});
}

// ----- filters ---------------------------------------------------------------

function Filters({
  project,
  status,
  agent,
  executor,
  executors,
  projects,
  onChange,
  onExecutorChange,
  onClear,
}: {
  project: string;
  status: string;
  agent: string;
  executor: string;
  executors: string[];
  projects: ProjectsPayload | undefined;
  onChange: (next: { project: string; status: string; agent: string }) => void;
  onExecutorChange: (next: string) => void;
  onClear: () => void;
}) {
  const [agentState, setAgentState] = useState({ value: agent, draft: agent });
  const agentDraft = agentState.value === agent ? agentState.draft : agent;
  const setAgentDraft = (draft: string) => setAgentState({ value: agent, draft });
  const commitAgent = () => onChange({ project, status, agent: agentDraft.trim() });
  const hasFilters = !!(project || status || agent || executor);
  return (
    <div className="grid min-w-0 grid-cols-2 items-center gap-2 sm:flex sm:flex-wrap">
      <Select value={project || "all"} onValueChange={(v) => onChange({ project: v === "all" ? "" : v, status, agent })}>
        <SelectTrigger className="w-full sm:w-44" aria-label="Project">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all">All projects</SelectItem>
          {(projects?.projects ?? []).map((p) => (
            <SelectItem key={p.slug} value={p.slug}>
              {p.slug}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Select value={status || "all"} onValueChange={(v) => onChange({ project, status: v === "all" ? "" : v, agent })}>
        <SelectTrigger className="w-full sm:w-52" aria-label="Status">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {STATUSES.map((s) => (
            <SelectItem key={s.value} value={s.value}>
              {s.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <div className="relative col-span-2 w-full sm:w-48">
        <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" aria-hidden />
        <Input
          id="harness-agent"
          aria-label="Agent"
          value={agentDraft}
          placeholder="Search by agent"
          onChange={(e) => setAgentDraft(e.target.value)}
          onBlur={commitAgent}
          onKeyDown={(e) => {
            if (e.key === "Enter") commitAgent();
            if (e.key === "Escape") setAgentDraft(agent);
          }}
          className="pl-8"
        />
      </div>
      {executors.length ? (
        <Select value={executor || "all"} onValueChange={(v) => onExecutorChange(v === "all" ? "" : v)}>
          <SelectTrigger className="w-full sm:w-44" aria-label="Machine" title="The machine (executor) the run happened on">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All machines</SelectItem>
            {executors.map((e) => (
              <SelectItem key={e} value={e}>
                {machineWord(e)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : null}
      {hasFilters ? (
        <Button type="button" variant="ghost" size="sm" onClick={onClear}>
          <XIcon /> Clear filters
        </Button>
      ) : null}
    </div>
  );
}

// ----- the list --------------------------------------------------------------

function RunRow({ r, selected, onSelect, models }: { r: HarnessSession; selected: boolean; onSelect: (id: string) => void; models: HarnessModel[] }) {
  const model = modelInfo(r.model, models);
  const remote = remoteExecutor(r.executor);
  return (
    <li>
      <button
        type="button"
        onClick={() => onSelect(r.id)}
        aria-current={selected ? "true" : undefined}
        title={"Run " + r.id + ", job " + r.job_id}
        className={cn(
          "flex w-full min-w-0 flex-col gap-1 px-4 py-3.5 text-left transition-colors hover:bg-muted/50 focus-visible:relative focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset motion-reduce:transition-none sm:px-5",
          selected && "bg-primary/[0.07] shadow-[inset_3px_0_0_var(--primary)] hover:bg-primary/[0.09]",
        )}
      >
        <span className="flex min-w-0 items-center justify-between gap-3 text-[13px] leading-relaxed">
          <RunStatus session={r} />
          <When d={r.started_at} className="shrink-0 text-muted-foreground" />
        </span>
        <span className="min-w-0 text-[15px] font-semibold leading-snug text-foreground anywhere">{r.job_title || r.job_id}</span>
        <span className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-0.5 text-[13px] leading-relaxed text-muted-foreground">
          {r.project ? <span>{r.project}</span> : null}
          {r.agent ? <span>{r.agent}</span> : null}
          <span title={model.tip}>{model.name}</span>
          <span title={plural(r.steps, "step") + ", " + plural(r.calls, "model call")}>{plural(r.steps, "step")}</span>
          {remote ? <span title="The machine (executor) the run happened on">{"On " + remote}</span> : null}
        </span>
      </button>
    </li>
  );
}

function RunList({ rows, selected, onSelect, models }: { rows: HarnessSession[]; selected: string; onSelect: (id: string) => void; models: HarnessModel[] }) {
  // Most recently active first, as the old table opened.
  const sorted = useMemo(() => [...rows].sort((a, b) => (a.updated_at < b.updated_at ? 1 : a.updated_at > b.updated_at ? -1 : 0)), [rows]);
  return (
    <ul aria-label="Runs" className="divide-y divide-border/70">
      {sorted.map((r) => (
        <RunRow key={r.id} r={r} selected={r.id === selected} onSelect={onSelect} models={models} />
      ))}
    </ul>
  );
}

// ----- the transcript --------------------------------------------------------

const REPORT_WORD: Record<string, string> = { SUMMARY: "Summary", FINDING: "Finding", SUGGESTION: "Suggestion", QUESTION: "Question", VOTE: "Vote" };

function ReportLines({ text }: { text: string }) {
  const keys = /^(SUMMARY|FINDING|SUGGESTION|QUESTION|VOTE):/;
  return (
    <div className="grid gap-2">
      {text.split("\n").map((line, i) => {
        const hit = keys.exec(line);
        if (!hit && !line.trim()) return null;
        return (
          <div key={i} className="min-w-0 text-sm leading-relaxed">
            {hit ? <span className="mr-1.5 font-semibold text-foreground">{REPORT_WORD[hit[1]] + ":"}</span> : null}
            <span className="whitespace-pre-wrap text-foreground/90 anywhere">{hit ? line.slice(hit[0].length).trimStart() : line}</span>
          </div>
        );
      })}
    </div>
  );
}

function exitWords(code: number | null): { text: string; bad: boolean } | null {
  if (code === null || code === undefined) return null;
  return code === 0 ? { text: "Succeeded", bad: false } : { text: "Failed (exit code " + code + ")", bad: true };
}

function Observation({ s }: { s: HarnessStep }) {
  const lines = (s.content || "").split("\n");
  const long = lines.length > 30;
  const [open, setOpen] = useState(lines.length <= 12);
  const [all, setAll] = useState(!long);
  const shown = all ? s.content : lines.slice(0, 30).join("\n");
  const exit = exitWords(s.exit_code);
  return (
    <Disclosure
      variant="row"
      open={open}
      onToggle={setOpen}
      className="rounded-xl border bg-muted/30 px-2"
      contentClassName="-mx-2 mt-0 px-0 pb-0"
      summary={
        <span className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] font-normal text-muted-foreground" title={s.command || undefined}>
          <span className="font-medium text-foreground">{open ? "Output" : "Show the output"}</span>
          <span>{plural(lines.length, "line")}</span>
          {exit ? <span className={cn(exit.bad ? "text-bad" : "text-ok")}>{exit.text}</span> : null}
          {s.duration_ms ? <span>{"Took " + (s.duration_ms < 1000 ? s.duration_ms + " ms" : span(s.duration_ms / 1000))}</span> : null}
        </span>
      }
    >
      <pre className="max-h-[52vh] overflow-auto border-t px-3 py-2.5 font-mono text-xs leading-relaxed whitespace-pre-wrap text-foreground/85 anywhere">{shown || "(no output)"}</pre>
      {long ? (
        <button type="button" className="w-full border-t px-3 py-2 text-left text-[13px] font-medium text-primary hover:underline" onClick={() => setAll((v) => !v)}>
          {all ? "Show only the first 30 lines" : "Show all " + num(lines.length) + " lines"}
        </button>
      ) : null}
    </Disclosure>
  );
}

const STEP_DOT: Record<string, string> = {
  assistant: "bg-primary/10 text-primary",
  command: "bg-muted text-foreground",
  read: "bg-muted text-foreground",
  grep: "bg-muted text-foreground",
  glob: "bg-muted text-foreground",
  edit: "bg-muted text-foreground",
  write: "bg-muted text-foreground",
  plan: "bg-primary/10 text-primary",
  mcp: "bg-muted text-foreground",
  skill: "bg-muted text-foreground",
  fetch: "bg-muted text-foreground",
  subagent: "bg-primary/10 text-primary",
  hook: "bg-muted text-muted-foreground",
  observation: "bg-muted text-muted-foreground",
  note: "bg-muted text-muted-foreground",
  error: "bg-warn/15 text-warn",
  exit: "bg-card text-foreground ring-1 ring-border",
};

const STEP_LABEL: Record<string, string> = {
  assistant: "The agent",
  command: "Ran a command",
  read: "Read a file",
  grep: "Searched the files",
  glob: "Listed files by name",
  edit: "Edited a file",
  write: "Wrote a file",
  plan: "Updated its plan",
  mcp: "Called an MCP tool",
  skill: "Loaded a skill",
  fetch: "Read a web page",
  subagent: "Handed a sub-task to a helper",
  hook: "Ran one of your hooks",
  observation: "What came back",
  note: "Note from PULSE",
  error: "Something went wrong",
  exit: "How it ended",
};

// The typed tools of the harness (read, grep, glob, edit, write, mcp, skill,
// fetch, process, hook) show their request the way a command shows its line; the
// reply follows as an observation like any command's output.
const TOOL_STEP_KINDS: string[] = ["read", "grep", "glob", "edit", "write", "mcp", "skill", "fetch", "process", "hook"];

const STEP_ICON: Record<string, typeof InfoIcon> = {
  assistant: BotIcon,
  command: TerminalIcon,
  observation: TerminalIcon,
  read: FileCodeIcon,
  grep: SearchIcon,
  glob: FilesIcon,
  edit: FilePenIcon,
  write: FilePlusIcon,
  plan: ListChecksIcon,
  mcp: PlugIcon,
  skill: BookOpenIcon,
  fetch: GlobeIcon,
  process: ActivityIcon,
  subagent: GitForkIcon,
  hook: WebhookIcon,
  error: TriangleAlertIcon,
};

function StepIcon({ s, session }: { s: HarnessStep; session: HarnessSession }) {
  // A helper's exit is its own, not the run's.
  if (s.kind === "exit" && !(s.depth ?? 0)) {
    const st = runState(session);
    return <st.Icon className={cn("size-3.5", RUN_ICON_TONE[st.tone])} />;
  }
  if (s.kind === "exit") return s.content === "submitted" ? <CircleCheckIcon className="size-3.5 text-ok" /> : <CircleAlertIcon className="size-3.5 text-warn" />;
  const Icon = STEP_ICON[s.kind] ?? InfoIcon;
  return <Icon className="size-3.5" />;
}

// A plan step holds the checklist the agent keeps: "plan: 1/3 done" then
// one "- [ ]", "- [~]" or "- [x]" line per item. It reads as a list.
export function planItems(content: string): { text: string; status: "todo" | "doing" | "done" }[] {
  const items: { text: string; status: "todo" | "doing" | "done" }[] = [];
  for (const line of (content || "").split("\n")) {
    const m = /^-\s*\[([ x~])\]\s*(.*)$/.exec(line.trim());
    if (!m) continue;
    items.push({ text: m[2], status: m[1] === "x" ? "done" : m[1] === "~" ? "doing" : "todo" });
  }
  return items;
}

function PlanCard({ s }: { s: HarnessStep }) {
  const items = planItems(s.content);
  const head = (s.content || "").split("\n")[0] || "";
  if (!items.length) return <p className="text-sm leading-relaxed whitespace-pre-wrap text-foreground anywhere">{s.content}</p>;
  return (
    <div className="grid min-w-0 gap-1.5 rounded-xl border bg-muted/30 px-4 py-3">
      <p className="text-[13px] font-medium text-muted-foreground">{head}</p>
      <ul className="grid gap-1 text-sm leading-relaxed">
        {items.map((it, i) => (
          <li key={i} className="flex min-w-0 items-start gap-2">
            {it.status === "done" ? <CircleCheckIcon className="mt-1 size-3.5 shrink-0 text-ok" aria-label="done" /> : it.status === "doing" ? <LoaderCircleIcon className="mt-1 size-3.5 shrink-0 animate-spin text-primary" aria-label="in progress" /> : <CircleIcon className="mt-1 size-3.5 shrink-0 text-muted-foreground" aria-label="to do" />}
            <span className={cn("min-w-0 anywhere", it.status === "done" && "text-muted-foreground line-through")}>{it.text}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

// Steps of one run are minutes apart: the clock time says more than "1mo ago".
function StepTime({ d }: { d: string }) {
  const t = parseDate(d);
  if (!t) return <When d={d} />;
  return (
    <time dateTime={d} title={d.replace("T", " ").replace("Z", " UTC")} className="tnum whitespace-nowrap">
      {t.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
    </time>
  );
}

function StepItem({ s, session, last }: { s: HarnessStep; session: HarnessSession; last: boolean }) {
  const tokens = (s.prompt_tokens || 0) + (s.completion_tokens || 0);
  // A helper's steps (depth 1) sit one indent in under the subagent step.
  const depth = Math.min(s.depth ?? 0, 3);
  const tip = ["Step " + s.n, depth ? "helper, depth " + depth : "", s.model ? "model " + s.model : "", tokens ? compact(tokens) + " tokens" : ""].filter(Boolean).join(", ");
  // An observation reads as the second half of the command above it.
  const quiet = s.kind === "observation";
  return (
    <li className={cn("relative min-w-0 pl-9", !last && "pb-5")} style={depth ? { marginLeft: depth * 28 } : undefined} data-depth={depth || undefined}>
      {!last ? <span className="absolute top-7 bottom-0 left-[13px] w-px bg-border" aria-hidden /> : null}
      <span className={cn("absolute top-0 left-0 flex size-7 items-center justify-center rounded-full", STEP_DOT[s.kind] ?? STEP_DOT.note)} aria-hidden>
        <StepIcon s={s} session={session} />
      </span>
      <div className={cn("flex min-h-7 min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-0.5 text-[13px] leading-relaxed", quiet && "sr-only")}>
        <span className="font-medium text-foreground">{s.kind === "exit" && depth ? "How the helper ended" : (STEP_LABEL[s.kind] ?? "Step " + s.n)}</span>
        <span className="text-muted-foreground" title={tip}>
          {s.at ? <StepTime d={s.at} /> : "Step " + s.n}
        </span>
      </div>
      <div className={cn("min-w-0", !quiet && "mt-1")}>
        {s.kind === "assistant" ? (
          <div className="min-w-0 text-[15px] leading-relaxed">
            <Markdown source={s.content} className="[&_pre]:whitespace-pre-wrap" />
          </div>
        ) : s.kind === "command" ? (
          <pre className="min-w-0 rounded-xl bg-zinc-950 px-3 py-2.5 font-mono text-[12.5px] leading-relaxed whitespace-pre-wrap text-zinc-100 anywhere dark:bg-black">
            <span className="text-emerald-300 select-none">$ </span>
            {s.command}
          </pre>
        ) : TOOL_STEP_KINDS.includes(s.kind) ? (
          <pre className="min-w-0 rounded-xl border bg-muted/30 px-3 py-2.5 font-mono text-[12.5px] leading-relaxed whitespace-pre-wrap text-foreground/90 anywhere">{s.content || s.command}</pre>
        ) : s.kind === "plan" ? (
          <PlanCard s={s} />
        ) : s.kind === "subagent" ? (
          <div className="grid min-w-0 gap-1 rounded-xl border border-primary/30 bg-primary/[0.05] px-4 py-3">
            <p className="text-[13px] font-medium text-muted-foreground">{s.command || "subagent"}</p>
            <p className="text-sm leading-relaxed whitespace-pre-wrap text-foreground anywhere">{s.content}</p>
          </div>
        ) : s.kind === "observation" ? (
          <Observation s={s} />
        ) : s.kind === "note" ? (
          <p className="text-sm leading-relaxed whitespace-pre-wrap text-muted-foreground anywhere">{sentence(s.content)}</p>
        ) : s.kind === "error" ? (
          <p className="text-sm leading-relaxed whitespace-pre-wrap text-foreground anywhere">{s.content}</p>
        ) : depth ? (
          <p className="text-sm leading-relaxed text-muted-foreground">{s.content === "submitted" ? "The helper reported back." : "The helper stopped: " + (s.content || "unknown") + "."}</p>
        ) : (
          <EndCard s={s} session={session} />
        )}
      </div>
    </li>
  );
}

const END_TONE: Record<RunTone, string> = {
  run: "border-primary/30 bg-primary/[0.05]",
  ok: "border-ok/35 bg-ok/[0.06]",
  warn: "border-warn/40 bg-warn/[0.07]",
  bad: "border-bad/30 bg-bad/[0.05]",
  neutral: "border-border bg-muted/40",
};

function EndCard({ s, session }: { s: HarnessStep; session: HarnessSession }) {
  const st = runState(session);
  return (
    <div className={cn("grid min-w-0 gap-2.5 rounded-xl border px-4 py-3", END_TONE[st.tone])}>
      <p className="text-[15px] font-semibold leading-snug text-foreground" title={s.content || session.exit_status || undefined}>
        {endSentence(session, s.content || session.exit_status)}
      </p>
      {session.status === "submitted" && session.submission ? <ReportLines text={session.submission} /> : null}
    </div>
  );
}

function transcriptText(d: HarnessSessionResponse): string {
  const lines = [`${d.session.id} ${d.session.status}`, `${d.session.project} ${d.session.job_id} ${d.session.job_title}`, `${d.session.agent} ${d.session.provider}/${d.session.model}`, ""];
  for (const s of d.steps) {
    lines.push(`## ${s.n}${s.depth ? " >" + s.depth : ""} ${s.kind} ${s.at || ""}`.trim());
    if (s.command) lines.push("$ " + s.command);
    if (s.content) lines.push(s.content);
    if (s.exit_code !== null) lines.push("exit " + s.exit_code);
    lines.push("");
  }
  if (d.session.submission) lines.push("## submission", d.session.submission);
  return lines.join("\n");
}

function CancelRunButton({ session, onDone }: { session: HarnessSession; onDone: () => Promise<void> }) {
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  if (session.status !== "running") return null;
  const name = session.job_title || session.job_id;
  const cancel = () => {
    setBusy(true);
    setError("");
    writes
      .harnessCancel(session.id)
      .then(() => {
        setConfirm(false);
        invalidate("harness/sessions");
        return onDone();
      })
      .catch((e: unknown) => {
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
      })
      .finally(() => setBusy(false));
  };
  return (
    <span className="inline-flex flex-wrap items-center gap-1.5">
      {confirm ? (
        <>
          <span className="text-[13px] text-foreground">Stop this run?</span>
          <Button type="button" size="sm" variant="destructive" onClick={cancel} disabled={busy} aria-label={"Yes, cancel the run: " + name}>
            <CheckIcon /> Yes, cancel it
          </Button>
          <Button type="button" size="sm" variant="ghost" onClick={() => setConfirm(false)} disabled={busy} aria-label={"No, keep running: " + name}>
            Keep running
          </Button>
        </>
      ) : (
        <Button type="button" size="sm" variant="outline" className="text-bad hover:text-bad" onClick={() => setConfirm(true)} disabled={busy || !!session.cancel_requested_at} aria-label={"Cancel the run: " + name}>
          <BanIcon /> {session.cancel_requested_at ? "Cancelling…" : "Cancel run"}
        </Button>
      )}
      {error ? <span className="basis-full text-[13px] text-bad">{error}</span> : null}
    </span>
  );
}

function mergeSteps(current: HarnessStep[], next: HarnessStep[]): HarnessStep[] {
  const seen = new Map<number, HarnessStep>();
  for (const s of current) seen.set(s.n, s);
  for (const s of next) seen.set(s.n, s);
  return [...seen.values()].sort((a, b) => a.n - b.n);
}

function dedupeSessions(rows: HarnessSession[]): HarnessSession[] {
  const seen = new Set<string>();
  const out: HarnessSession[] = [];
  for (const row of rows) {
    if (seen.has(row.id)) continue;
    seen.add(row.id);
    out.push(row);
  }
  return out;
}

function LiveState({ state }: { state: "connecting" | "live" | "polling" }) {
  const text = state === "live" ? "Live" : state === "polling" ? "Checking every few seconds" : "Connecting";
  return (
    <span aria-live="polite" className="inline-flex items-center gap-1.5 text-[13px] text-muted-foreground" title={state === "live" ? "New steps appear as they happen." : state === "polling" ? "The live stream is down; the page checks for new steps every 5 seconds." : undefined}>
      <span className={cn("size-2 rounded-full", state === "live" ? "bg-ok" : state === "polling" ? "bg-warn" : "bg-muted-foreground/40")} aria-hidden />
      {text}
    </span>
  );
}

function Transcript({ id, models, onBack }: { id: string; models: HarnessModel[]; onBack: () => void }) {
  const live = useLiveEvents();
  const q = useApi<HarnessSessionResponse>(id ? "harness/sessions/" + enc(id) : null, { refresh: live.state === "polling" ? 5000 : 30000 });
  const [extra, setExtra] = useState<{ id: string; steps: HarnessStep[] }>({ id: "", steps: [] });
  const [followState, setFollowState] = useState({ id, follow: true, atBottom: true });
  const [scrolledState, setScrolledState] = useState({ id, away: false });
  const scrolledAway = scrolledState.id === id && scrolledState.away;
  const lastEvent = useRef({ id, eventId: 0 });
  const scroll = useRef<HTMLDivElement | null>(null);
  const bottom = useRef<HTMLDivElement | null>(null);
  const follow = followState.id === id ? followState.follow : true;
  const atBottom = followState.id === id ? followState.atBottom : true;
  const d = useMemo(() => {
    if (!q.data) return undefined;
    return { ...q.data, steps: mergeSteps(q.data.steps, extra.id === id ? extra.steps : []) };
  }, [extra, id, q.data]);
  const running = d?.session.status === "running";
  const now = useNow();

  useEffect(() => {
    const timer = setTimeout(() => {
      setExtra((cur) => {
        if (cur.id !== id) return { id, steps: [] };
        const full = q.data?.steps;
        if (!full?.length || !cur.steps.length) return cur;
        const fullNs = new Set(full.map((s) => s.n));
        const steps = cur.steps.filter((s) => !fullNs.has(s.n));
        return steps.length === cur.steps.length ? cur : { id, steps };
      });
    }, 0);
    return () => clearTimeout(timer);
  }, [id, q.data?.steps, q.updatedAt]);

  useEffect(() => {
    if (!(follow && atBottom && running)) return;
    // On desktop the transcript scrolls inside its own card: follow there and
    // leave the page where it is. On a phone the page itself is the scroller.
    const el = scroll.current;
    if (el && el.scrollHeight > el.clientHeight + 1 && getComputedStyle(el).overflowY !== "visible") el.scrollTop = el.scrollHeight;
    else bottom.current?.scrollIntoView({ block: "end" });
  }, [d?.steps.length, follow, atBottom, running]);

  const onScroll = () => {
    const el = scroll.current;
    if (!el) return;
    setFollowState((cur) => ({ id, follow: cur.id === id ? cur.follow : true, atBottom: el.scrollHeight - el.scrollTop - el.clientHeight < 96 }));
    const away = el.scrollTop > 24;
    setScrolledState((cur) => (cur.id === id && cur.away === away ? cur : { id, away }));
  };

  useEffect(() => {
    if (!id || !d) return;
    const lastSeen = lastEvent.current.id === id ? lastEvent.current.eventId : 0;
    const hit = live.events.filter((e) => e.id > lastSeen && e.subject === id && (e.action === "harness.step" || e.action === "harness.finish" || e.action === "harness.cancel")).sort((a, b) => a.id - b.id);
    if (!hit.length) return;
    lastEvent.current = { id, eventId: hit[hit.length - 1].id };
    const lastN = d.steps.reduce((max, s) => Math.max(max, s.n), -1);
    const path = "harness/sessions/" + enc(id);
    const finished = hit.some((e) => e.action === "harness.finish" || e.action === "harness.cancel");
    api<HarnessSessionResponse>(path + "?after=" + lastN)
      .then((r) => {
        setExtra((cur) => ({ id, steps: mergeSteps(cur.id === id ? cur.steps : [], r.steps) }));
        if (finished) void q.reload();
      })
      .catch(() => {
        void q.reload();
      });
  }, [d, id, live.events, q]);

  const back = (
    <Button type="button" variant="ghost" size="sm" className="-ml-2 justify-self-start lg:hidden" onClick={onBack}>
      <ArrowLeftIcon /> All runs
    </Button>
  );
  if (q.error && !d)
    return (
      <div>
        {back}
        <ErrorNotice error={q.error} onRetry={() => void q.reload()}>
          {"This run's transcript didn't load (" + q.error + ")."}
        </ErrorNotice>
      </div>
    );
  if (!d)
    return (
      <div className="rounded-2xl border bg-card p-5" aria-busy="true">
        {back}
        <SkeletonRows n={5} />
      </div>
    );
  const s = d.session;
  const totalTokens = s.prompt_tokens + s.completion_tokens;
  const trajectoryHref = s.trajectory ? apiHref("harness/sessions/" + enc(s.id) + "/trajectory") : "";
  const model = modelInfo(s.model, models);
  const remote = remoteExecutor(s.executor);
  const errorTone = runState(s).tone === "bad" ? "bad" : "warn";
  return (
    <section aria-label={"Transcript: " + (s.job_title || s.job_id)} className="flex min-w-0 flex-col rounded-2xl border bg-card lg:max-h-[calc(100dvh-3rem)]">
      <header className="grid min-w-0 gap-3 border-b border-border/70 px-4 pt-4 pb-4 sm:px-5" aria-live={running ? "polite" : undefined}>
        {back}
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1 text-[13px]">
          <RunStatus session={s} />
          {running ? <LiveState state={live.state} /> : <When d={s.started_at} className="text-muted-foreground" />}
        </div>
        <h2 className="min-w-0 text-lg font-semibold leading-snug tracking-tight anywhere">
          <Link href={jobHref(s)} className="underline-offset-2 hover:underline" title={"Open the job " + s.job_id}>
            {s.job_title || s.job_id}
          </Link>
        </h2>
        <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1 text-[13px] leading-relaxed text-muted-foreground">
          {s.project ? <ProjectLink slug={s.project} className="font-normal text-muted-foreground" /> : null}
          {s.agent ? <span>{s.agent}</span> : null}
          <span title={model.tip}>{model.name}</span>
          {remote ? <span title="The machine (executor) the run happened on">{"On " + remote}</span> : null}
          <span title={plural(s.steps, "step") + ", " + plural(s.calls, "model call")}>{plural(s.steps, "step")}</span>
          {s.started_at ? <span title={s.started_at ? "Started " + s.started_at.replace("T", " ").replace("Z", " UTC") : undefined}>{runTime(s, now)}</span> : null}
          {totalTokens ? <span title={compact(s.prompt_tokens) + " read, " + compact(s.completion_tokens) + " written"}>{compact(totalTokens) + " tokens"}</span> : null}
          {s.cost ? <span title="What the model provider charged. For GitHub Copilot, premium requests.">{plural(s.cost, "model credit")}</span> : null}
        </div>
        {s.cancel_requested_at && s.status === "running" ? <Notice tone="warn">You asked to cancel this run. The agent stops after the step it is on.</Notice> : null}
        {s.error ? <Notice tone={errorTone}>{s.error}</Notice> : null}
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          {running ? (
            <Button size="sm" variant={follow ? "secondary" : "outline"} aria-pressed={follow} onClick={() => setFollowState((cur) => ({ id, follow: cur.id === id ? !cur.follow : false, atBottom: cur.id === id ? cur.atBottom : true }))} title="Keep the newest step in view">
              <PlayIcon /> {follow ? "Following new steps" : "Follow new steps"}
            </Button>
          ) : null}
          <CancelRunButton session={s} onDone={q.reload} />
          <Button
            size="sm"
            variant="ghost"
            onClick={() =>
              navigator.clipboard
                .writeText(transcriptText(d))
                .then(() => toast.success("Transcript copied."))
                .catch(() => toast.error("Could not copy the transcript."))
            }
          >
            <CopyIcon /> Copy as text
          </Button>
          {trajectoryHref ? (
            <Button size="sm" variant="ghost" asChild>
              <a href={trajectoryHref} download title="Download the full record of the run (trajectory) as a file">
                <DownloadIcon /> Download
              </a>
            </Button>
          ) : null}
        </div>
      </header>
      <div ref={scroll} onScroll={onScroll} className="min-h-0 min-w-0 flex-1 px-4 py-5 sm:px-5 lg:overflow-y-auto">
        {scrolledAway ? (
          // Only on desktop, where the transcript scrolls inside its card: a
          // hairline edge says the steps go on above, with a way back up.
          <div className="sticky -top-5 z-10 -mx-4 h-0 sm:-mx-5">
            <div className="flex min-w-0 items-center justify-between gap-3 border-b border-border/70 bg-card px-4 py-2 text-[13px] text-muted-foreground sm:px-5">
              <span>Earlier steps are above.</span>
              <button type="button" className="rounded-md font-medium text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={() => scroll.current?.scrollTo({ top: 0 })}>
                Go to the first step
              </button>
            </div>
          </div>
        ) : null}
        {d.steps.length ? (
          <ol aria-label="Steps" className="min-w-0">
            {d.steps.map((step, i) => (
              <StepItem key={step.n} s={step} session={s} last={i === d.steps.length - 1} />
            ))}
          </ol>
        ) : (
          <p className="text-sm text-muted-foreground">{running ? "The agent is getting started. Its first step shows up here." : "This run has no recorded steps."}</p>
        )}
        <Disclosure summary="Technical details" className="mt-6">
          <div className="rounded-xl border bg-muted/30 px-3 py-3">
            <Kv
              className="text-[13px] [&_dd]:font-mono [&_dd]:text-[12.5px]"
              rows={[
                ["Run", s.id],
                ["Job", s.job_id],
                ["Model", s.provider + "/" + s.model],
                ...(s.executor ? [["Machine", s.executor] as [React.ReactNode, React.ReactNode]] : []),
                ["Folder", s.cwd || "—"],
                [
                  "Record file",
                  <span key="t" className="inline-flex min-w-0 flex-wrap items-center gap-1.5">
                    <span className="anywhere">{s.trajectory || "not recorded"}</span>
                    {s.trajectory ? (
                      <Button size="icon-xs" variant="ghost" aria-label="Copy trajectory path" onClick={() => navigator.clipboard.writeText(s.trajectory).then(() => toast.success("Path copied."))}>
                        <ClipboardIcon />
                      </Button>
                    ) : null}
                  </span>,
                ],
                ["Tokens", compact(s.prompt_tokens) + " prompt, " + compact(s.completion_tokens) + " completion"],
              ]}
            />
          </div>
        </Disclosure>
        <div ref={bottom} />
      </div>
    </section>
  );
}

// ----- the page --------------------------------------------------------------

function Summary({ counts }: { counts: HarnessSessionsResponse["counts"] | undefined }) {
  if (!counts) return null;
  const running = counts.running ? num(counts.running) + " running." : "Nothing running right now.";
  const today = counts.today ? plural(counts.today, "run") + " today: " + num(counts.submitted_today) + " finished, " + num(counts.failed_today) + " stopped early." : "No runs today yet.";
  return (
    <p className="mb-5 max-w-3xl text-[15px] leading-relaxed text-foreground">
      {running} {today}{" "}
      {counts.cost_today ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <span tabIndex={0} className="underline decoration-muted-foreground/50 decoration-dotted underline-offset-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
              {plural(counts.cost_today, "model credit") + " used today."}
            </span>
          </TooltipTrigger>
          <TooltipContent>What the model provider charged (provider units). For GitHub Copilot, premium requests.</TooltipContent>
        </Tooltip>
      ) : null}
    </p>
  );
}

export function HarnessPageClient() {
  const sp = useSearchParams();
  const router = useRouter();
  const pathname = usePathname();
  const project = sp.get("project") || "";
  const status = sp.get("status") || "";
  const agent = sp.get("agent") || "";
  const selected = sp.get("id") || "";
  const limit = 50;
  const [executor, setExecutor] = useState("");
  const [olderState, setOlderState] = useState<{ key: string; pages: HarnessSessionsResponse[]; loading: boolean; error: string }>({ key: "", pages: [], loading: false, error: "" });
  const filterKey = [project, status, agent].join("\n");
  const older = useMemo(() => (olderState.key === filterKey ? olderState.pages : []), [filterKey, olderState.key, olderState.pages]);
  const olderLoading = olderState.key === filterKey && olderState.loading;
  const olderError = olderState.key === filterKey ? olderState.error : "";
  const path = "harness/sessions" + query({ project, status, agent, limit });
  const sessions = useApi<HarnessSessionsResponse>(path, { refresh: 5000 });
  const models = useApi<HarnessModelsResponse>("harness/models", { refresh: 60000 });
  const projects = useApi<ProjectsPayload>("projects", { refresh: 60000 });
  const loadedPages = useMemo(() => (sessions.data ? [sessions.data, ...older] : older), [older, sessions.data]);
  const loadedRows = useMemo(() => dedupeSessions(loadedPages.flatMap((p) => p.sessions)), [loadedPages]);
  const counts = sessions.data?.counts;
  const live = useLiveEvents();
  const now = useNow();
  const lastRunEvent = useMemo(() => live.events.reduce((at, e) => (e.action.startsWith("harness.") && e.at > at ? e.at : at), ""), [live.events]);
  const lastRunEventAt = parseDate(lastRunEvent)?.getTime() ?? 0;
  const liveNow = live.state === "live" && lastRunEventAt > 0 && now - lastRunEventAt < 2 * 60_000;
  const executors = useMemo(() => Array.from(new Set(loadedRows.map((r) => executorName(r.executor)).filter(Boolean))).sort(), [loadedRows]);
  const rows = useMemo(() => (executor ? loadedRows.filter((r) => executorName(r.executor) === executor) : loadedRows), [executor, loadedRows]);
  const choose = useCallback(
    (id: string) => {
      updateSearch(pathname, router, { project, status, agent, id });
      // On a phone the transcript takes the list's place: start reading it from its top.
      if (id && typeof window !== "undefined" && window.matchMedia("(max-width: 1023px)").matches) window.scrollTo({ top: 0 });
    },
    [pathname, router, project, status, agent],
  );
  const selectedKnown = selected && rows.some((r) => r.id === selected);
  const showTranscript = !!selected && (selectedKnown || !rows.length);
  const lastPage = loadedPages[loadedPages.length - 1];
  const canLoadMore = !!lastPage?.next_before;
  const modelRows = models.data?.models ?? [];
  const hasFilters = !!(project || status || agent || executor);
  const clear = () => {
    setExecutor("");
    updateSearch(pathname, router, { id: selected });
  };
  const loadOlder = () => {
    const before = lastPage?.next_before;
    if (!before || olderLoading) return;
    setOlderState((cur) => ({ key: filterKey, pages: cur.key === filterKey ? cur.pages : [], loading: true, error: "" }));
    api<HarnessSessionsResponse>("harness/sessions" + query({ project, status, agent, limit, before }))
      .then((page) => setOlderState((cur) => ({ key: filterKey, pages: [...(cur.key === filterKey ? cur.pages : []), page], loading: false, error: "" })))
      .catch((e: unknown) => {
        const t = errorText(e);
        setOlderState((cur) => ({ key: filterKey, pages: cur.key === filterKey ? cur.pages : [], loading: false, error: [t.error, t.why, t.fix].filter(Boolean).join(" ") }));
      });
  };
  return (
    <>
      <PageTitle
        icon={ScrollTextIcon}
        sub="Every agent run, step by step: what it was asked, what it did and how it ended."
        actions={
          <Button asChild variant="outline">
            <Link href={hrefs.settings("harness")} title="Settings, Harness: sign in to the model provider and pick models">
              Sign-in &amp; models
            </Link>
          </Button>
        }
        bar={<Filters project={project} status={status} agent={agent} executor={executor} executors={executors} projects={projects.data} onChange={(next) => updateSearch(pathname, router, { ...next, id: selected })} onExecutorChange={setExecutor} onClear={clear} />}
      >
        Runs
      </PageTitle>
      {sessions.error && !sessions.data ? (
        <ErrorNotice error={sessions.error} onRetry={() => void sessions.reload()}>
          {"The list of runs didn't load (" + sessions.error + "). Check that PULSE is running, then try again."}
        </ErrorNotice>
      ) : null}
      {!sessions.data ? (
        sessions.error ? null : (
          <SkeletonPage rows={3} />
        )
      ) : rows.length === 0 ? (
        hasFilters ? (
          <EmptyState
            title="No runs match these filters"
            icon={<SearchIcon className="size-[18px]" />}
            action={
              <Button size="sm" variant="outline" onClick={clear}>
                <XIcon /> Clear filters
              </Button>
            }
          >
            Try another project or status, or clear the filters to see every run.
          </EmptyState>
        ) : (
          <EmptyState
            title="No runs yet"
            icon={<BotIcon className="size-[18px]" />}
            action={
              <Button asChild size="sm" variant="outline">
                <Link href={hrefs.settings("harness")}>Open sign-in &amp; models</Link>
              </Button>
            }
          >
            Each time an agent starts a job, its run shows up here with a step-by-step transcript. Sign in to a model provider and pick models so agents can start.
          </EmptyState>
        )
      ) : (
        <>
          <Summary counts={counts} />
          <div className="grid min-w-0 items-start gap-5 lg:grid-cols-[minmax(0,45fr)_minmax(0,55fr)]">
            <div className={cn("min-w-0", showTranscript && "hidden lg:block")}>
              <div className="overflow-hidden rounded-2xl border bg-card">
                <RunList rows={rows} selected={selected} onSelect={choose} models={modelRows} />
              </div>
              <div className="mt-3 flex min-w-0 flex-wrap items-center justify-between gap-2 px-1 text-[13px] text-muted-foreground">
                <span className="inline-flex flex-wrap items-center gap-x-3 gap-y-1">
                  <span>{rows.length === loadedRows.length ? "Showing " + plural(rows.length, "run") : "Showing " + num(rows.length) + " of " + plural(loadedRows.length, "loaded run")}</span>
                  {liveNow ? (
                    <span className="inline-flex items-center gap-1.5" title="A run changed in the last two minutes. The list keeps itself up to date.">
                      <span className="size-2 rounded-full bg-ok" aria-hidden /> Live
                    </span>
                  ) : null}
                </span>
                {olderError ? <span className="text-bad">{olderError}</span> : null}
                {canLoadMore ? (
                  <Button type="button" size="sm" variant="outline" onClick={loadOlder} disabled={olderLoading}>
                    {olderLoading ? "Loading…" : "Load older runs"}
                  </Button>
                ) : null}
              </div>
            </div>
            <div className={cn("min-w-0 lg:sticky lg:top-6", !showTranscript && "hidden lg:block")}>
              {showTranscript ? (
                <Transcript id={selected} models={modelRows} onBack={() => choose("")} />
              ) : (
                <EmptyState icon={<ScrollTextIcon className="size-[18px]" />} className="py-14">
                  Pick a run to read what the agent did, step by step.
                </EmptyState>
              )}
            </div>
          </div>
        </>
      )}
    </>
  );
}
