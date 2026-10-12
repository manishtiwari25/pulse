"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, CopyIcon, ListChecksIcon, SparklesIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle } from "./item-card";
import { DecideButtons } from "./decide-buttons";
import { HeldoutButton } from "./heldout";
import { evolvePlanningView } from "./evolve-planning";
import { Disclosure, Kv, Notice, Panel } from "./panel";
import { Mono, ProjectLink, ToneBadge, When, Who } from "./who";
import { cn } from "@/lib/utils";
import { andList, fmtDate, num, parseDate, plural, rel, sentence } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useApi, useNow } from "@/lib/use-api";
import { projectPath, type EvolutionDoc, type EvolveFixture, type EvolveGate, type EvolveGates, type EvolveLevel, type EvolveRecordRef, type OwnerQuestion } from "@/lib/api";

// The self-improvement surfaces (ADR-029 to ADR-033). Agents try small
// changes, measure them against test cases (public fixtures) and keep only
// what measures better. The console says where a project stands in plain
// words - how far it may go (the level), how many test cases and private
// tests (held-out fixtures) it has, and the starting score (baseline) the
// runner measured - and names the one step that is the owner's, if any. The
// runner (daemon) measures the starting score itself (ADR-031) and, under
// evolve.heldout auto, writes the private tests too (ADR-033); under owner
// they are the owner's one step, taken in the private-tests dialog
// (ADR-032), with the command kept as the terminal alternative. Nothing
// here starts a measurement or edits a record. Technical names (level
// letters, record ids, setting keys) stay in tooltips and "Show details".

// The four levels, A to D, as the owner reads them.
const LEVELS: Record<string, { word: string; now: string }> = {
  A: { word: "Watching only", now: "Agents only watch for now; nothing is measured or changed yet." },
  B: { word: "Measuring only", now: "Agents measure and record results for now, but don't try changes yet." },
  C: { word: "Trial runs", now: "Agents try changes in trial runs for now; none are kept yet." },
  D: { word: "Improving", now: "Agents may try changes and keep the ones that measure better." },
};
const LEVEL_ORDER = ["A", "B", "C", "D"] as const;
const LEVEL_PROGRESS: Record<string, number> = { A: 25, B: 50, C: 75, D: 100 };
const levelWord = (level: string) => LEVELS[level]?.word ?? "Not known";
const levelTitle = (level: string) => (LEVELS[level] ? "Self-improvement level " + level + " of A to D: " + LEVELS[level].now : "PULSE could not work out how far self-improvement may go");
const score = (n: number) => (Number.isFinite(n) ? n.toFixed(3).replace(/\.?0+$/, "") : "—");
const INTRO = "Agents try small changes, measure them against test cases, and keep only what makes the project measurably better.";

// ----- copy ------------------------------------------------------------------

async function copyText(text: string): Promise<void> {
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) return navigator.clipboard.writeText(text);
  // No clipboard API (an insecure origin): the old textarea trick.
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  const ok = document.execCommand("copy");
  document.body.removeChild(ta);
  if (!ok) throw new Error("the browser refused");
}

// The CLI writes a command and its note as `cmd  (note), or alt ...`; only
// the command is worth copying.
export function commandOnly(s: string): string {
  return s.trim().split(/\s{2,}/)[0] ?? "";
}

export function CopyButton({ text, label = "Copy", size = "sm", variant = "default", className }: { text: string; label?: string; size?: "xs" | "sm" | "default"; variant?: "default" | "outline" | "ghost"; className?: string }) {
  const [copied, setCopied] = useState(false);
  const cmd = commandOnly(text);
  const onClick = () => {
    void copyText(cmd)
      .then(() => {
        setCopied(true);
        toast.success("Copied. Paste it in a terminal on this machine.");
        setTimeout(() => setCopied(false), 1800);
      })
      .catch(() => toast.error("Couldn't copy. Select the command and copy it yourself."));
  };
  return (
    <Button size={size} variant={variant} onClick={onClick} aria-label={label + ": " + cmd} title={cmd} className={className}>
      {copied ? <CheckIcon /> : <CopyIcon />} {copied ? "Copied" : label}
    </Button>
  );
}

// A command the owner runs in a terminal. The hub never runs it for them -
// that is the point: the console names the step, the human takes it.
export function CommandBox({ command, copy, className }: { command: string; copy?: boolean; className?: string }) {
  return (
    <div className={cn("flex items-start gap-2 rounded-lg border bg-muted/60 p-2.5 pl-3", className)}>
      <code className="min-w-0 flex-1 self-center whitespace-pre-wrap font-mono text-[12.5px] leading-5 text-foreground anywhere">{command}</code>
      {copy ? <CopyButton text={command} size="xs" variant="outline" className="shrink-0" /> : null}
    </div>
  );
}

// The same step as a command, folded under the console's own button
// (ADR-032): the terminal stays the owner's other way, never the first one.
// `also` is a second, optional way to the same step ("Or write your own text").
export function TerminalAlternative({ command, className, summary = "Prefer the terminal?", also }: { command: string; className?: string; summary?: string; also?: { label: string; command: string } }) {
  return (
    // rotate-0 while closed: an open Disclosure around this one (Show details) would otherwise turn its chevron too.
    <Disclosure summary={summary} className={cn("text-[13px] [&:not([open])>summary>svg]:rotate-0", className)} contentClassName="grid gap-1.5">
      <span className="text-xs text-muted-foreground">Run this in a terminal on this machine:</span>
      <CommandBox command={command} copy />
      {also ? (
        <>
          <span className="mt-1 text-xs text-muted-foreground">{also.label}</span>
          <CommandBox command={also.command} copy />
        </>
      ) : null}
    </Disclosure>
  );
}

// ----- badges -----------------------------------------------------------------

export function LevelBadge({ level, className }: { level: EvolveLevel | string; className?: string }) {
  if (!level) return <ToneBadge tone="warn" title={levelTitle("")} className={className}>Level unknown</ToneBadge>;
  return (
    <ToneBadge tone={level === "D" ? "ok" : "neutral"} dot title={levelTitle(level)} className={className}>
      {levelWord(level)}
    </ToneBadge>
  );
}

// `amistio-template/TPL-evolution-constitution@r1` reads as its record name.
export const seedName = (from: string) => from.split("/").pop() || from;
// The one way a record's text changes: the owner amends it in a terminal.
export const amendCommand = (id: string) => "pulse record amend " + id + " --body-file FILE";

// The two records self-improvement reads, by what they are for.
const RECORD_WORD = { constitution: "Constitution", evaluators: "Scoring rules" } as const;
const RECORD_HINT = { constitution: "the rules self-improvement follows: what agents may change and how far", evaluators: "how each experiment is judged" } as const;
const seededTitle = (from: string) => "Still the template's text" + (from ? " (copied from " + seedName(from) + ")" : "") + ". An agent drafts a version for this project for you to adopt, or you can edit it yourself.";
const ownTitle = (r: EvolveRecordRef, what: "constitution" | "evaluators") => r.id + ", version " + num(r.rev) + (r.seeded_from ? ": started as the template's text and edited since" : ": " + RECORD_HINT[what]);

// A record as one chip (ADR-030): its version and whether the text is still
// the template's. Nothing when the hub does not report the record.
export function RecordChip({ r, what, className }: { r: EvolveRecordRef | null | undefined; what: "constitution" | "evaluators"; className?: string }) {
  if (r === undefined) return null;
  const word = RECORD_WORD[what];
  if (r === null)
    return (
      <ToneBadge tone="warn" title={"No " + word.toLowerCase() + " yet. " + sentence(RECORD_HINT[what]) + "."} className={className}>
        {"No " + word.toLowerCase()}
      </ToneBadge>
    );
  // ADR-046: a seeded record says what waits on it - a draft to adopt, a drafting job, or nothing yet.
  if (r.draft)
    return (
      <ToneBadge tone="info" title={r.draft.author + " drafted it from the repository. Read and adopt it on the Self-improvement tab."} className={className}>
        {word + ": draft to review"}
      </ToneBadge>
    );
  if (r.seeded && r.draft_job)
    return (
      <ToneBadge tone="info" title={"An agent is drafting it from the repository (job " + r.draft_job.id + ")"} className={className}>
        {word + ": being drafted"}
      </ToneBadge>
    );
  if (r.seeded)
    return (
      <ToneBadge tone="warn" title={seededTitle(r.seeded_from)} className={className}>
        {word + ": template text"}
      </ToneBadge>
    );
  return (
    <ToneBadge tone="ok" title={ownTitle(r, what)} className={className}>
      {word + " v" + num(r.rev)}
    </ToneBadge>
  );
}

// One line under a project card's tiles: how far the loop may go and why.
// The constitution leads (it is the first gate); the scoring rules only speak
// up when they alone are missing - one seed fills both - so the line stays short.
// The starting-score chip reports the runner's progress (ADR-031), never a step.
export function EvolveBadges({ g, className }: { g: EvolveGate; className?: string }) {
  return (
    <div className={cn("flex flex-wrap items-center gap-1.5 text-xs", className)}>
      <RecordChip r={g.constitution} what="constitution" />
      {g.evaluators === null && g.constitution ? <RecordChip r={null} what="evaluators" /> : null}
      <ToneBadge tone={g.fixtures_enabled ? "ok" : "neutral"} className={g.fixtures_enabled ? undefined : "text-muted-foreground"} title="Test cases agents may see; every experiment is measured against them">
        {plural(g.fixtures_enabled, "test case")}
      </ToneBadge>
      <BaselineChip g={g} />
      <LevelBadge level={g.evolve_level} />
    </div>
  );
}

// ----- the starting score (baseline, ADR-031) ----------------------------------

// A next step that asks nothing of the owner: "none", or "none - <what the
// daemon does instead>". The part after the dash, when there is one.
export const noneStep = (next: string): { none: boolean; why: string } => {
  const m = /^none\b\s*(?:[-–:]\s*)?([^]*)$/.exec(next.trim());
  return m ? { none: true, why: (m[1] ?? "").trim() } : { none: false, why: "" };
};

// Where the baseline stands for a project, from the row's gate fields. The
// daemon measures it (ADR-031): a job in flight, a failure it retries after
// 24 hours, or - fixtures ready and no step named for the owner - its next
// tick; with evolve.loop off it waits for the loop instead. An older hub's
// `pulse job add ... experiment` step counts as the daemon's turn too: the
// console never asks the owner to queue the baseline.
export type BaselineState = { kind: "recorded" } | { kind: "measuring"; id: string; status: string } | { kind: "failed"; at: string } | { kind: "waiting"; on: "daemon" | "loop" } | { kind: "none" };

export function baselineState(g: EvolveGate): BaselineState {
  if (g.baseline) return { kind: "recorded" };
  if (g.baseline_job) return { kind: "measuring", id: g.baseline_job.id, status: g.baseline_job.status };
  if (g.baseline_failed_at) return { kind: "failed", at: g.baseline_failed_at };
  const next = g.evolve_next.trim();
  if (g.fixtures_enabled > 0 && (next === "" || noneStep(next).none || next.startsWith("pulse job add"))) return { kind: "waiting", on: "daemon" };
  if (g.fixtures_enabled > 0 && next.startsWith("pulse settings set evolve.loop")) return { kind: "waiting", on: "loop" };
  return { kind: "none" };
}

// Hours until the daemon retries a failed baseline: 24 from the failure, never below 0.
export const retryHours = (failedAt: string, now: number): number => {
  const t = parseDate(failedAt);
  return t ? Math.max(0, Math.round(24 - (now - t.getTime()) / 3600000)) : 24;
};

// The starting score in words, for a chip or a sentence.
function baselineWords(st: BaselineState, now: number, s?: number | null): { text: string; title: string; tone: "ok" | "info" | "warn" | "neutral" } {
  switch (st.kind) {
    case "recorded":
      return { text: typeof s === "number" && Number.isFinite(s) ? "Starting score " + score(s) : "Starting score measured", title: "Measured by the runner. Every experiment is compared with it.", tone: "ok" };
    case "measuring":
      return { text: st.status === "leased" ? "Measuring the starting score" : "Starting score: about to be measured", title: "The runner's measuring job (" + st.id + ")", tone: "info" };
    case "failed": {
      const h = retryHours(st.at, now);
      return { text: "Starting score failed " + rel(st.at, now) + ", " + (h ? "retry in " + plural(h, "hour") : "retrying soon"), title: "The runner's measurement failed at " + fmtDate(st.at) + ". It tries again after 24 hours. Nothing for you to do.", tone: "warn" };
    }
    case "waiting":
      return st.on === "loop"
        ? { text: "Starting score: once self-improvement is on", title: "Test cases are ready. The runner measures the starting score once self-improvement is switched on in Settings (evolve.loop).", tone: "neutral" }
        : { text: "Starting score: the runner measures it next", title: "Test cases are ready. The runner measures the starting score on its next round. Nothing for you to do.", tone: "neutral" };
    default:
      return { text: "No starting score yet", title: "The runner measures it once the rules and test cases are in place.", tone: "neutral" };
  }
}

// The baseline as one quiet chip: recorded (with the score when the panel
// knows it), measuring, failed and retrying, waiting for the daemon, or not
// yet - nothing at this gate is the owner's to do, so the chip never warns
// louder than a failure.
export function BaselineChip({ g, score: s, recordedBy, className }: { g: EvolveGate; score?: number | null; recordedBy?: string; className?: string }) {
  const now = useNow();
  const w = baselineWords(baselineState(g), now, s);
  return (
    <ToneBadge tone={w.tone} dot={w.tone === "info"} title={recordedBy ? w.title + " (" + recordedBy + ")" : w.title} className={cn(w.tone === "neutral" && "text-muted-foreground", className)}>
      {w.text}
    </ToneBadge>
  );
}

// The project-row view of the loop's status, so the panel derives the same
// step and chip from the evolution payload as the cards do from a row.
export function gateOfStatus(s: EvolveGates, docs?: { constitution: EvolutionDoc | null; evaluators: EvolutionDoc | null }): EvolveGate {
  const ref = (d: EvolutionDoc | null): EvolveRecordRef | null => (d ? { id: d.record.id, rev: d.record.current_rev, seeded: d.seeded === true, seeded_from: d.seeded_from ?? "" } : null);
  const g: EvolveGate = {
    fixtures_enabled: s.public_fixtures,
    baseline: s.baseline !== null && s.baseline !== undefined,
    evolve_level: s.level === "A" || s.level === "B" || s.level === "C" || s.level === "D" ? s.level : "",
    evolve_next: s.next_human_step ?? "",
  };
  if (s.baseline_job !== undefined) g.baseline_job = s.baseline_job;
  if (typeof s.baseline_failed_at === "string") g.baseline_failed_at = s.baseline_failed_at;
  if (docs) {
    g.constitution = ref(docs.constitution);
    g.evaluators = ref(docs.evaluators);
  }
  return g;
}

// A gate as a flag: open or not. With the record's doc (ADR-030) the flag
// carries its version, and a template copy says so instead of a plain check.
function GateFlag({ on, what, doc }: { on: boolean; what: "constitution" | "evaluators"; doc?: EvolutionDoc | null }) {
  const rev = doc?.record.current_rev;
  const seeded = on && !!doc?.seeded;
  const word = RECORD_WORD[what];
  return (
    <ToneBadge tone={!on || seeded ? "warn" : "ok"} title={seeded ? seededTitle(doc?.seeded_from ?? "") : sentence(RECORD_HINT[what])}>
      {!on ? "No " + word.toLowerCase() : seeded ? word + ": template text" : word + (rev ? " v" + num(rev) : "")}
    </ToneBadge>
  );
}

// ----- the inbox card ----------------------------------------------------------

// The steps only the owner can take. The baseline is not one of them since
// ADR-031: the daemon measures it, so no gate card ever asks for it.
export type GateKind = "seed" | "constitution" | "records" | "heldout";

export interface GateStep {
  kind: GateKind;
  label: string; // the short "what is missing" phrase ("Next step: Set up private tests")
  slug: string;
  slugs?: string[]; // every project one shared step serves, when the inbox folds them (the held-out set is one per machine)
  level: EvolveLevel;
  title: string;
  body: string;
  command: string; // one command, ready to paste
  alt?: string; // the other way to the same step, when the hub names one ("or pulse record new ...")
}

// The hub writes the seed step as `seed-cmd  (template copy), or write-cmd`:
// two commands joined by prose. Split it so each copies on its own.
export function splitAlternative(next: string): { command: string; alt: string } {
  const [command, alt = ""] = next.trim().split(/\s+\(template copy\),?\s+or\s+/, 2);
  return { command: command.trim(), alt: alt.trim() };
}

const GATE_LABEL: Record<GateKind, string> = { seed: "add test cases", constitution: "write a constitution", records: "add the self-improvement rules", heldout: "set up private tests" };

// Which of the loop's two records a project lacks: what the row says first
// (null is "none"), the title the seed command names when the row is silent.
function missingRecords(g: EvolveGate, next: string): ("constitution" | "evaluators")[] {
  const fromRow = ([g.constitution === null ? "constitution" : "", g.evaluators === null ? "evaluators" : ""] as const).filter((x): x is "constitution" | "evaluators" => !!x);
  return fromRow.length ? fromRow : [/--title Evaluators\b/.test(next) ? "evaluators" : "constitution"];
}

const HELDOUT_WHY = "Private tests are two test cases agents never see, so self-improvement can't tune itself to pass them.";

// Which human step, if any, a project waits for. One card per project: the
// hub's `evolve_next` names the step. A project the record seed did not reach
// (ADR-030) gets the seeding step; an older hub still names `pulse record new
// evolution`. The baseline gate has no card (ADR-031): with a baseline job
// open, a failure the daemon retries, or a step that starts with `none`, the
// row asks nothing - and an older hub's `pulse job add ... experiment` is the
// daemon's job now, so it asks nothing either. The held-out fixtures
// (ADR-012) are the one step left that is the owner's, and the card says so.
export function gateStep(slug: string, g: EvolveGate | null): GateStep | null {
  if (!g) return null;
  const next = g.evolve_next.trim();
  if (!next || noneStep(next).none || (!g.baseline && (g.baseline_job || g.baseline_failed_at)) || next.startsWith("pulse job add")) return null;
  const base = { slug, level: g.evolve_level };
  if (next.startsWith("pulse evolve seed")) {
    const missing = missingRecords(g, next);
    const one = missing.length === 1 ? missing[0] : undefined;
    const { command, alt } = splitAlternative(next);
    return {
      ...base,
      kind: "records",
      label: one ? "add " + (one === "constitution" ? "a constitution" : "scoring rules") : GATE_LABEL.records,
      title: one ? "Add " + (one === "constitution" ? "a constitution" : "scoring rules") + " for " + slug : "Add the self-improvement rules for " + slug,
      body: "Self-improvement follows two short records: a constitution (what agents may change) and scoring rules (how a change is judged). This project has " + (one ? "no " + RECORD_WORD[one].toLowerCase() : "neither") + " yet. Run the command once to start from the template, then make the text yours under the project's Self-improvement tab.",
      command,
      ...(alt ? { alt } : {}),
    };
  }
  if (next.startsWith("pulse record new evolution"))
    return {
      ...base,
      kind: "constitution",
      label: GATE_LABEL.constitution,
      title: "Write a constitution for " + slug,
      body: "The constitution holds the rules self-improvement follows. Without one, agents only measure and never try a change. Write it in your own words, then add it with the command.",
      command: next,
    };
  if (next.startsWith("pulse evolve fixture seed"))
    return {
      ...base,
      kind: "seed",
      label: GATE_LABEL.seed,
      title: "Add starter test cases for " + slug,
      body: "There are no test cases yet, so nothing can be measured. The starter set adds six small ones to begin with; you can add your own later.",
      command: next,
    };
  if (next.startsWith("pulse evolve setup"))
    return {
      ...base,
      kind: "heldout",
      label: GATE_LABEL.heldout,
      title: "Set up two private tests",
      body: HELDOUT_WHY + " You write them once, PULSE keeps them outside every project, and the runner does everything else. Self-improvement for " + slug + " starts once they're in.",
      command: next,
    };
  return null;
}

// The held-out set is one per machine (evolve.heldout_dir), so when several
// projects wait for it the inbox shows one card that names them all, not one
// card per project saying the same thing. Every other step stays per project.
export function foldSteps(steps: GateStep[]): GateStep[] {
  const shared = steps.filter((s) => s.kind === "heldout");
  if (shared.length < 2) return steps;
  const slugs = shared.map((s) => s.slug);
  const one: GateStep = {
    ...shared[0],
    slugs,
    body: HELDOUT_WHY + " You write them once and the runner does everything else. One set on this machine serves " + andList(slugs) + "; self-improvement starts once they're in.",
  };
  const rest = steps.filter((s) => s.kind !== "heldout");
  const at = steps.indexOf(shared[0]);
  return [...rest.slice(0, at), one, ...rest.slice(at)];
}

// One waiting human step as an ask card: what, where, why, the button that
// takes it, and the way to the project's self-improvement view. The private
// tests card (ADR-012) is the owner's one step and opens the private-tests
// dialog (ADR-032); the rest copy the command. Either way the command itself
// folds under "Prefer the terminal?". A folded card links every project it serves.
export function GateCard({ step }: { step: GateStep }) {
  const heldout = step.kind === "heldout";
  const slugs = step.slugs && step.slugs.length > 1 ? step.slugs : [step.slug];
  return (
    <ItemCard highlight ariaLabel={(heldout ? "Your one step: " : "Setup step: ") + step.title + (slugs.length > 1 ? " (" + andList(slugs) + ")" : " (" + step.slug + ")")}>
      <ItemCardHeader>
        <ItemCardKind icon={ListChecksIcon} tone="you">{heldout ? "Your one step" : "Setup step"}</ItemCardKind>
        {slugs.length > 1 ? <span title={andList(slugs)}>{plural(slugs.length, "project")}</span> : <ProjectLink slug={step.slug} className="font-normal text-muted-foreground" />}
      </ItemCardHeader>
      <ItemCardTitle>{step.title}</ItemCardTitle>
      <ItemCardBody clamp={0}>{step.body}</ItemCardBody>
      <TerminalAlternative command={step.command} summary={heldout ? "Prefer the terminal?" : "Show the command"} also={step.alt ? { label: "Or write your own text and add it:", command: step.alt } : undefined} />
      <ItemCardActions>
        <span className="flex min-w-0 flex-wrap items-center gap-2">
          {slugs.map((slug) => (
            <Button key={slug} asChild variant="outline">
              <Link href={hrefs.project(slug, "evolution")} title={"Self-improvement for " + slug}>
                {slugs.length > 1 ? slug : "Self-improvement"}
              </Link>
            </Button>
          ))}
        </span>
        <span className="flex w-full justify-start sm:ml-auto sm:w-auto sm:justify-end">
          {heldout ? <HeldoutButton projects={slugs} size="default" label="Set up private tests" /> : <CopyButton text={step.command} size="default" label="Copy command" />}
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}

// ----- the project's panel -----------------------------------------------------

// The named step (title, why, the way to take it) the way the inbox card says
// it, for the panel: the same words in both places, marked as the owner's.
function StepBlock({ step }: { step: GateStep }) {
  const heldout = step.kind === "heldout";
  return (
    <ItemCard highlight ariaLabel={(heldout ? "Your one step: " : "Setup step: ") + step.title}>
      <ItemCardHeader>
        <ItemCardKind icon={ListChecksIcon} tone="you">{heldout ? "Your one step" : "Setup step"}</ItemCardKind>
      </ItemCardHeader>
      <ItemCardTitle>{step.title}</ItemCardTitle>
      <ItemCardBody clamp={0} className="max-w-3xl">{step.body}</ItemCardBody>
      <TerminalAlternative command={step.command} summary={heldout ? "Prefer the terminal?" : "Show the command"} also={step.alt ? { label: "Or write your own text and add it:", command: step.alt } : undefined} />
      <ItemCardActions>
        {heldout ? <HeldoutButton projects={[step.slug]} size="default" label="Set up private tests" /> : <CopyButton text={step.command} size="default" label="Copy command" />}
      </ItemCardActions>
    </ItemCard>
  );
}

// The held-out count in words: "2 private tests, written by the runner".
// Since ADR-033 the runner may write them sealed - weaker evidence than the
// owner's own, so it is said where the count is said. An older hub sends no `generated`.
type HeldoutCounts = { count: number; pending: number; generated?: number };
export function heldoutLabel(h: HeldoutCounts): string {
  const gen = h.generated ?? 0;
  const who = gen ? (gen === h.count ? ", written by the runner" : ", " + num(gen) + " written by the runner") : h.count ? ", written by you" : "";
  return (h.count ? plural(h.count, "private test") : "No private tests yet") + who + (h.pending ? " (" + num(h.pending) + " still to write)" : "");
}
export function heldoutTitle(h: HeldoutCounts): string {
  const gen = h.generated ?? 0;
  if (!gen) return "Test cases agents never see. Only their number shows here.";
  return (gen === h.count ? "All " : num(gen) + " of ") + plural(h.count, "private test") + " were written by the runner and sealed (ADR-033). Rewrite any of them to make it yours; the runner never overwrites a test you wrote.";
}

// Where self-improvement stands in one sentence, then the step that is the
// owner's (if any), the facts in words, and every technical gate, count and
// planning reason under "Show details". `gates` null means the hub could not
// evaluate them: the error shows, the test cases still list. `docs` (the
// constitution and evaluators as the evolution payload carries them) lets
// the record flags show version and template state.
export function SelfImprovementPanel({ gates, error, fixtures, docs, className, canDecide = true }: { gates: EvolveGates | null | undefined; error?: string; fixtures: EvolveFixture[]; docs?: { constitution: EvolutionDoc | null; evaluators: EvolutionDoc | null }; className?: string; canDecide?: boolean }) {
  const now = useNow();
  const [details, setDetails] = useState(false);
  // Planning stopped at a question for the owner: the Yes / No belong right here, next to the reason. Only an open question gets the buttons - one already decided clears on the daemon's next tick.
  const questionId = gates?.planning?.question ?? "";
  const questions = useApi<OwnerQuestion[]>(questionId && gates?.project ? projectPath(gates.project, "questions") : null, { refresh: 60000 });
  const openQuestion = questionId ? (questions.data ?? []).find((q) => q.id === questionId) : undefined;
  const next = (gates?.next_human_step ?? "").trim();
  const none = noneStep(next);
  const waits = !!next && !none.none;
  const g = gates ? gateOfStatus(gates, docs) : null;
  const step = gates && g ? gateStep(gates.project, g) : null;
  const bl = g ? baselineState(g) : null;
  const h = gates?.heldout;
  const planning = evolvePlanningView(gates?.planning);
  // "none - or <command>": the daemon retries on its own; the command is the owner's shortcut, offered, never asked.
  const optional = none.none && /^or\s+pulse\b/.test(none.why) ? none.why.replace(/^or\s+/, "") : "";
  const idle = (): string => {
    if (bl?.kind === "failed") {
      const hrs = retryHours(bl.at, now);
      return "Nothing waits for you. Measuring the starting score failed " + rel(bl.at, now) + "; the runner tries again " + (hrs ? "in about " + plural(hrs, "hour") : "on its next round") + ".";
    }
    if (bl?.kind === "measuring") return "Nothing waits for you. The runner is " + (bl.status === "leased" ? "measuring" : "about to measure") + " the starting score.";
    if (none.why && !optional) return "Nothing waits for you: " + none.why.replace(/\.$/, "") + ".";
    if (gates?.level === "D") return planning.next;
    return "Nothing waits for you" + (gates?.loop ? " and self-improvement is on." : ". Experiments start once self-improvement is switched on in Settings.");
  };
  const stateLine = gates ? selfImprovementState(gates, planning, step, openQuestion?.summary) : "PULSE couldn't read where self-improvement stands.";
  const tried = gates ? gates.kept + gates.discarded : 0;
  const recordedBy = gates?.baseline_id;
  const blWords = bl ? baselineWords(bl, now, gates?.baseline) : null;
  return (
    <Panel title="Self-improvement" icon={SparklesIcon} actions={gates ? <LevelBadge level={gates.level} /> : undefined} className={className}>
      <p className="mb-3 max-w-3xl text-sm leading-relaxed text-muted-foreground">{INTRO}</p>
      {gates && g ? (
        <div className="grid gap-4">
          <p className="max-w-3xl text-base font-semibold leading-snug anywhere">{stateLine}</p>
          {openQuestion ? (
            <div className="surface-you flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border px-4 py-3 text-sm">
              <span className="min-w-0 flex-1 basis-60 anywhere">
                {sentence(openQuestion.summary)}{" "}
                <Link href={hrefs.post(gates.project, openQuestion.id)} className="text-muted-foreground underline underline-offset-2 hover:text-foreground">Read the post</Link>
              </span>
              <DecideButtons id={openQuestion.id} summary={openQuestion.summary} stop={openQuestion.stop} canDecide={canDecide} yesLabel={openQuestion.stop ? "Yes, continue" : "Yes"} noLabel={openQuestion.stop ? "No, keep it stopped" : "No"} />
            </div>
          ) : null}
          {waits && step ? <StepBlock step={step} /> : null}

          <Kv
            className="max-w-3xl gap-y-2"
            rows={[
              ["How far it may go", <span key="lvl" title={levelTitle(gates.level)}>{levelWord(gates.level)}{LEVELS[gates.level] ? <span className="text-muted-foreground">{". " + LEVELS[gates.level].now.replace(/ for now/, "")}</span> : null}</span>],
              ["Test cases", gates.public_fixtures ? plural(gates.public_fixtures, "test case") + " agents can see" : "None yet"],
              ["Private tests", h ? <span key="ho" title={h.problem || heldoutTitle(h)}>{heldoutLabel(h)}{h.problem ? <span className="text-warn">{h.count ? ". Not ready yet." : ""}</span> : null}</span> : "Not reported"],
              ["Starting score", blWords ? <span key="bl" title={blWords.title}>{gates.baseline !== null && gates.baseline !== undefined ? "Starting score " + score(gates.baseline) + ", measured by the runner" : blWords.text}</span> : "—"],
              ["Results so far", tried ? num(gates.kept) + " of " + plural(tried, "change") + " kept" : "No experiments yet"],
            ]}
          />

          <Disclosure summary="Show details" open={details} onToggle={setDetails} contentClassName="mt-3 grid gap-4 rounded-xl border bg-muted/30 p-4">
              <div className="grid gap-2">
                <div className="flex flex-wrap items-center justify-between gap-2 text-[13px]">
                  <span className="font-medium">{"Level " + (gates.level || "?") + " of A to D"}</span>
                  <span className="text-muted-foreground">{LEVEL_ORDER.map((l) => l + " " + levelWord(l).toLowerCase()).join(", ")}</span>
                </div>
                <Progress value={LEVEL_PROGRESS[gates.level] ?? 0} className="h-1.5" aria-label={"Level " + (gates.level || "unknown") + " of A to D"} />
                <p className="text-xs leading-relaxed text-muted-foreground">How far self-improvement may go right now, not how much it has improved. A starting score is not a finished experiment.</p>
                {gates.reason ? <p className="text-[13px] leading-relaxed anywhere"><span className="text-muted-foreground">Why: </span>{gates.reason}</p> : null}
              </div>

              <div className="flex flex-wrap items-center gap-1.5 text-xs">
                {gates.stop ? <ToneBadge tone="bad" title="Self-improvement is stopped until you clear the stop">Stopped</ToneBadge> : null}
                <ToneBadge tone={gates.loop ? "ok" : "neutral"} dot title="evolve.loop: whether experiments may run back to back">
                  {gates.loop ? "Switched on" : "Switched off"}
                </ToneBadge>
                <GateFlag on={gates.constitution} what="constitution" doc={docs?.constitution} />
                <GateFlag on={gates.evaluators} what="evaluators" doc={docs?.evaluators} />
                <ToneBadge tone={gates.public_fixtures ? "ok" : "warn"} title="Test cases agents may see (public fixtures)">
                  {plural(gates.public_fixtures, "test case")}
                </ToneBadge>
                <BaselineChip g={g} score={gates.baseline} recordedBy={recordedBy} />
                {h ? (
                  <ToneBadge tone={h.problem ? "warn" : "neutral"} title={h.problem || heldoutTitle(h)}>
                    {heldoutLabel(h)}
                  </ToneBadge>
                ) : null}
                {gates.product_scope ? (
                  <ToneBadge tone={gates.product_scope.allowed ? "ok" : "neutral"} title={gates.product_scope.reason || undefined}>
                    {gates.product_scope.allowed ? "May change product code" : "May change agent setup only"}
                  </ToneBadge>
                ) : null}
                {gates.scope ? (
                  <ToneBadge tone={gates.scope.editable.length ? "ok" : "warn"} title={gates.scope.reason || undefined}>
                    {gates.scope.editable.length ? num(gates.scope.editable.length) + " of " + num(gates.scope.classes) + " file groups may change" : "No file group may change"}
                  </ToneBadge>
                ) : null}
                {gates.cap_reached ? <ToneBadge tone="bad" title={gates.limit_problem || "A limit the constitution sets is reached"}>Limit reached</ToneBadge> : null}
              </div>
              {/* The set exists and the gate has moved on: the dialog's done state (add one, rewrite one) stays reachable from here (ADR-032). */}
              {h && h.count > 0 && step?.kind !== "heldout" ? (
                <div>
                  <HeldoutButton projects={[gates.project]} size="xs" variant="outline" label="Add or rewrite private tests" title="Add another private test or rewrite one. The console never shows them." />
                </div>
              ) : null}

              <ItemCardMeta>
                <span title="Measured experiments since self-improvement was last switched on. Merging may still be pending or fail.">{plural(gates.cycles, "experiment") + " since it was switched on" + (gates.hours ? " (" + plural(Math.round(gates.hours), "hour") + ")" : "")}</span>
                <span title="All experiments ever. Kept does not mean merged or published.">{num(gates.kept) + " kept, " + num(gates.discarded) + " dropped in all"}</span>
                {gates.voted?.tried ? <span title="Suggestions the board voted for that were tried">{"Voted suggestions: " + num(gates.voted.kept) + " of " + num(gates.voted.tried) + " kept"}</span> : null}
                {gates.signal?.tried ? <span title="Hint test cases tried">{"Hints: " + num(gates.signal.kept) + " of " + num(gates.signal.tried) + " kept"}</span> : null}
                {gates.limits && gates.limits !== "none" ? <span>Limits <Mono>{gates.limits}</Mono></span> : null}
                {gates.tolerance ? <span title="How much worse a score may be and still count as no change">{"Tolerance " + gates.tolerance}</span> : null}
              </ItemCardMeta>

              <div className="grid gap-1.5">
                <div className="flex flex-wrap items-center gap-2 text-[13px]">
                  <span className="font-medium">Next experiment</span>
                  <ToneBadge tone={planning.tone}>{planning.label}</ToneBadge>
                </div>
                <p className="text-[13px] leading-relaxed text-muted-foreground anywhere">{planning.next}</p>
                <p className="max-h-40 overflow-auto whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground anywhere"><span className="font-medium">Planning note: </span>{planning.reason}</p>
                {questionId && questions.data && !openQuestion ? (
                  <p className="text-xs text-muted-foreground">That question is already answered or closed; planning moves past it on the runner&apos;s next round.</p>
                ) : null}
                {gates.planning?.job || gates.planning?.suggestion ? (
                  <ItemCardMeta>
                    {gates.planning.job ? <span title={gates.planning.job}>Job <Mono>{gates.planning.job}</Mono></span> : null}
                    {gates.planning.suggestion ? <Link href={hrefs.post(gates.project, gates.planning.suggestion)} className="underline-offset-2 hover:underline">Read the suggestion</Link> : null}
                  </ItemCardMeta>
                ) : null}
              </div>

              <div className="grid gap-1.5">
                <div className="text-[13px] font-medium">Your next step</div>
                {waits && step ? (
                  <p className="text-[13px] text-muted-foreground">{sentence(step.label)}: see the setup step above.</p>
                ) : waits ? (
                  <>
                    <p className="text-[13px] text-muted-foreground">Run this in a terminal on this machine. The console names the step and never starts a measurement.</p>
                    <CommandBox command={next} copy />
                  </>
                ) : (
                  <>
                    <p className="text-[13px] leading-relaxed text-muted-foreground anywhere">{idle()}</p>
                    {optional ? <TerminalAlternative command={optional} summary="Measure it now instead of waiting" /> : null}
                  </>
                )}
              </div>
          </Disclosure>
        </div>
      ) : (
        <Notice tone="warn" className="text-[13px]">
          <div className="font-medium">PULSE couldn&apos;t check where self-improvement stands.</div>
          <div className="mt-0.5 text-muted-foreground">{error || "It gave no reason."} Try again in a moment.</div>
        </Notice>
      )}
      <FixtureList fixtures={fixtures} className="mt-5 border-t pt-4" />
    </Panel>
  );
}

function selfImprovementState(gates: EvolveGates, planning: ReturnType<typeof evolvePlanningView>, step: GateStep | null, question?: string): string {
  const state = gates.planning?.state ?? "";
  const results = num(gates.kept) + " of " + plural(gates.kept + gates.discarded, "change") + " kept so far.";
  if (question) return "Self-improvement stopped and waits for your yes or no.";
  if (step) return "Waiting for one step from you: " + step.label + ".";
  if (gates.stop) return "Self-improvement is stopped until you lift the stop.";
  if (gates.level !== "D") return LEVELS[gates.level]?.now ?? "PULSE couldn't work out how far self-improvement may go.";
  if (!gates.loop) return "Ready, but switched off. Turn self-improvement on in Settings to start.";
  if (state === "leased" || state === "finalizing" || state === "queued") return planning.label + ". " + results;
  if (state === "waiting") return "On. Waiting for the board to agree on a suggestion to try.";
  if (state === "") return "On. " + results;
  return planning.label + ". " + planning.next;
}

// ----- the record's provenance (ADR-030) --------------------------------------

// The version and record of a constitution or scoring-rules record, for a
// panel's header: "Version 1, template text" warns that the text is still the template's.
export function DocRev({ doc, slug }: { doc: EvolutionDoc; slug: string }) {
  const r = doc.record;
  const waiting = doc.draft ? "draft to review" : doc.seeded && doc.draft_job ? "being drafted" : doc.seeded ? "template text" : "";
  return (
    <>
      <span title={doc.draft ? "An agent drafted the text. Read it and adopt it below." : doc.seeded ? seededTitle(doc.seeded_from ?? "") : doc.seeded_from ? "Started as the template's text, edited since" : "Version " + num(r.current_rev)}>
        {"Version " + num(r.current_rev) + (waiting ? ", " + waiting : "")}
      </span>
      <Link href={hrefs.record(r.id, slug)} title={r.id} className="underline-offset-2 hover:text-foreground hover:underline">
        Open the record
      </Link>
    </>
  );
}

// The test cases as compact rows: name, origin, who may see it, state, weight
// and who added it when. Never the request or the scorer, never an edit control.
export function FixtureList({ fixtures, className }: { fixtures: EvolveFixture[]; className?: string }) {
  const enabled = fixtures.filter((f) => f.enabled).length;
  return (
    <section className={className} aria-label="Test cases">
      <div className="mb-2 flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <h3 className="text-[15px] font-semibold">Test cases</h3>
        {fixtures.length ? <span className="text-[13px] text-muted-foreground">{enabled === fixtures.length ? plural(fixtures.length, "test case") + ", all on" : num(enabled) + " of " + plural(fixtures.length, "test case") + " on"}</span> : null}
      </div>
      {fixtures.length ? (
        <ul className="grid gap-1.5">
          {fixtures.map((f) => (
            <li key={f.kind + "/" + f.name} title={f.kind === "public" ? "Agents may see it; it counts in the score" : undefined} className={cn("flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-lg border bg-card px-3 py-2 text-[13px]", !f.enabled && "opacity-60")}>
              <span className="min-w-0 font-mono text-[12.5px] font-medium anywhere">{f.name}</span>
              {f.starter ? <ToneBadge tone="info" title="Part of the starter set added when the project was set up">Starter</ToneBadge> : null}
              {f.kind !== "public" ? (
                <ToneBadge tone="neutral" title={f.kind === "signal" ? "A hint agents read; it does not count in the score" : f.kind}>
                  {f.kind === "signal" ? "Hint only" : sentence(f.kind)}
                </ToneBadge>
              ) : null}
              {!f.enabled ? <ToneBadge tone="neutral">Off</ToneBadge> : null}
              <span className="ml-auto flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground">
                <span title="How much it counts in the score">{"Weight " + f.weight}</span>
                <Who row={{ agent: f.created_by }} compact />
                <When d={f.at} />
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <div className="grid gap-2 rounded-xl border border-dashed px-4 py-3 text-[13px] leading-relaxed text-muted-foreground">
          <p>No test cases yet, so there&apos;s nothing to measure against. The starter set adds six small ones.</p>
          <TerminalAlternative command="pulse evolve fixture seed" summary="Show the command" />
        </div>
      )}
      <p className="mt-2 text-xs leading-relaxed text-muted-foreground">Only names show here. What each test case asks and how it&apos;s scored stays inside PULSE; add or change test cases from a terminal.</p>
    </section>
  );
}
