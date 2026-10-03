"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, CopyIcon, ExternalLinkIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "./item-card";
import { DecideButtons } from "./decide-buttons";
import { HeldoutButton } from "./heldout";
import { evolvePlanningView } from "./evolve-planning";
import { Notice, Panel } from "./panel";
import { Mono, ProjectLink, ToneBadge, When, Who } from "./who";
import { cn } from "@/lib/utils";
import { andList, fmtDate, num, parseDate, plural, rel } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useApi, useNow } from "@/lib/use-api";
import { projectPath, type EvolutionDoc, type EvolveFixture, type EvolveGate, type EvolveGates, type EvolveLevel, type EvolveRecordRef, type OwnerQuestion } from "@/lib/api";

// The self-improvement surfaces (ADR-029, ADR-030, ADR-031, ADR-032). A
// project's loop opens with enabled public fixtures and a recorded baseline;
// the console shows both and lists the fixtures. The loop's two records
// (constitution, evaluators) are seeded at onboarding from the template's
// text; the console shows their revision and whether they are still that
// text. The baseline is the daemon's to measure (ADR-031): the console
// reports where that stands and asks nothing. The two private held-out
// fixtures (ADR-012) are the daemon's too since ADR-033: under
// evolve.heldout auto it writes them sealed and the hub counts them as
// generated, so no step is named; under owner they are the one human step
// left, named as such and taken here (ADR-032): a dialog has the hub write
// the two files, never read back, with the command kept as the terminal
// alternative. Nothing here starts a measurement or edits a record.

const LEVELS = "A observe · B record · C rehearse · D evolve";
const LEVEL_TITLE = "highest self-improvement level allowed right now (" + LEVELS + ")";
const LEVEL_PROGRESS: Record<string, number> = { A: 25, B: 50, C: 75, D: 100 };
const score = (n: number) => (Number.isFinite(n) ? n.toFixed(3).replace(/\.?0+$/, "") : "—");

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

export function CopyButton({ text, label = "Copy", size = "sm", variant = "default", className }: { text: string; label?: string; size?: "xs" | "sm"; variant?: "default" | "outline" | "ghost"; className?: string }) {
  const [copied, setCopied] = useState(false);
  const cmd = commandOnly(text);
  const onClick = () => {
    void copyText(cmd)
      .then(() => {
        setCopied(true);
        toast.success("Copied - paste it in a terminal on the hub machine.");
        setTimeout(() => setCopied(false), 1800);
      })
      .catch(() => toast.error("Could not copy - select the command and copy it yourself."));
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
    <div className={cn("flex items-start gap-2 rounded-lg border bg-muted/60 p-3", className)}>
      <code className="min-w-0 flex-1 whitespace-pre-wrap font-mono text-[12.5px] leading-5 anywhere">{command}</code>
      {copy ? <CopyButton text={command} size="xs" variant="outline" className="shrink-0" /> : null}
    </div>
  );
}

// The same step as a command, kept under the console's own button (ADR-032):
// quieter, still copyable, so the terminal stays the owner's other way.
export function TerminalAlternative({ command, className }: { command: string; className?: string }) {
  return (
    <div className={cn("grid gap-1", className)}>
      <span className="text-[11px] text-muted-foreground">or in a terminal:</span>
      <CommandBox command={command} copy className="border-dashed bg-transparent text-muted-foreground" />
    </div>
  );
}

// ----- badges -----------------------------------------------------------------

export function LevelBadge({ level, className }: { level: EvolveLevel | string; className?: string }) {
  if (!level) return <ToneBadge tone="warn" title="the hub could not evaluate the self-improvement gates" className={className}>level ?</ToneBadge>;
  return (
    <ToneBadge tone={level === "D" ? "ok" : "neutral"} title={LEVEL_TITLE} className={className}>
      {"level " + level}
    </ToneBadge>
  );
}

// `amistio-template/TPL-evolution-constitution@r1` reads as its record name.
export const seedName = (from: string) => from.split("/").pop() || from;
// The one way a record's text changes: the owner amends it in a terminal.
export const amendCommand = (id: string) => "pulse record amend " + id + " --body-file FILE";

const seededTitle = (from: string) => "still the template's text" + (from ? " (seeded from " + from + ")" : "") + " - the daemon has an agent draft it for you to adopt, or edit it yourself";
const ownTitle = (r: EvolveRecordRef, what: string) => "the " + r.id + " record, revision " + num(r.rev) + (r.seeded_from ? " - started as a copy of " + r.seeded_from + ", edited since" : " - " + what + " in your own words");

// The constitution as one chip (ADR-030): its revision, and whether the text is
// still the seeded copy. Nothing when the hub does not report the record.
export function RecordChip({ r, what, className }: { r: EvolveRecordRef | null | undefined; what: "constitution" | "evaluators"; className?: string }) {
  if (r === undefined) return null;
  if (r === null)
    return (
      <ToneBadge tone="warn" title={"no " + what + " record - " + (what === "constitution" ? "the loop has no bounds to read" : "the loop has no checks to read") + "; seed it or write your own"} className={className}>
        {"no " + what}
      </ToneBadge>
    );
  // ADR-046: a seeded record says what waits on it - a draft to adopt, a drafting job, or nothing yet.
  if (r.draft)
    return (
      <ToneBadge tone="info" title={"an agent (" + r.draft.author + ") drafted the " + what + " from the repository - read and adopt it on the Evolution tab"} className={className}>
        {what + " r" + num(r.rev) + " · draft ready"}
      </ToneBadge>
    );
  if (r.seeded && r.draft_job)
    return (
      <ToneBadge tone="info" title={"an agent is drafting the " + what + " from the repository (job " + r.draft_job.id + ", " + r.draft_job.status + ")"} className={className}>
        {what + " r" + num(r.rev) + " · drafting…"}
      </ToneBadge>
    );
  if (r.seeded)
    return (
      <ToneBadge tone="warn" title={seededTitle(r.seeded_from)} className={className}>
        {what + " r" + num(r.rev) + " · seeded"}
      </ToneBadge>
    );
  return (
    <ToneBadge tone="ok" title={ownTitle(r, what)} className={className}>
      {what + " r" + num(r.rev)}
    </ToneBadge>
  );
}

// One line under a project card's tiles: how far the loop may go and why.
// The constitution leads (it is the first gate); the evaluators only speak
// up when they alone are missing - one seed fills both - so the line stays short.
// The baseline chip reports the daemon's progress (ADR-031), never a step.
export function EvolveBadges({ g, className }: { g: EvolveGate; className?: string }) {
  return (
    <div className={cn("flex flex-wrap items-center gap-1.5 text-xs", className)}>
      <RecordChip r={g.constitution} what="constitution" />
      {g.evaluators === null && g.constitution ? <RecordChip r={null} what="evaluators" /> : null}
      <ToneBadge tone={g.fixtures_enabled ? "ok" : "neutral"} className={g.fixtures_enabled ? undefined : "text-muted-foreground"} title="enabled public fixtures - the checks agents may see; every experiment is measured against them">
        {plural(g.fixtures_enabled, "fixture")}
      </ToneBadge>
      <BaselineChip g={g} />
      <LevelBadge level={g.evolve_level} />
    </div>
  );
}

// ----- the baseline (ADR-031) --------------------------------------------------

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

const baselineJobWord = (status: string) => (status === "leased" ? "measuring" : "queued");
// A job id already reads as one (`job-2026...`); a bare id gets the word.
const jobRef = (id: string) => (/^job/i.test(id) ? id : "job " + id);

// The baseline as one quiet chip: recorded (with the score when the panel
// knows it), measuring, failed and retrying, waiting for the daemon, or not
// yet - nothing at this gate is the owner's to do, so the chip never warns
// louder than a failure.
export function BaselineChip({ g, score: s, recordedBy, className }: { g: EvolveGate; score?: number | null; recordedBy?: string; className?: string }) {
  const now = useNow();
  const st = baselineState(g);
  switch (st.kind) {
    case "recorded":
      return (
        <ToneBadge tone="ok" title={recordedBy ? "recorded by " + recordedBy + " - every variant is measured against it" : "the baseline is recorded - every variant is measured against it"} className={className}>
          {typeof s === "number" && Number.isFinite(s) ? "baseline " + score(s) : "baseline"}
        </ToneBadge>
      );
    case "measuring":
      return (
        <ToneBadge tone="info" dot title={"the daemon's baseline job is " + st.status + " - pulse job show " + st.id} className={className}>
          {"baseline: " + baselineJobWord(st.status) + " (" + jobRef(st.id) + ")"}
        </ToneBadge>
      );
    case "failed": {
      const h = retryHours(st.at, now);
      return (
        <ToneBadge tone="warn" title={"the daemon's baseline job failed at " + fmtDate(st.at) + "; it retries after 24 hours - nothing for you to do"} className={className}>
          {"baseline failed " + rel(st.at, now) + "; " + (h ? "retry in " + num(h) + " h" : "retry due")}
        </ToneBadge>
      );
    }
    case "waiting":
      return st.on === "loop" ? (
        <ToneBadge tone="neutral" title="fixtures are ready; the daemon records the baseline once evolve.loop is on (Settings, or pulse settings set evolve.loop 1)" className={cn("text-muted-foreground", className)}>
          baseline: once the loop is on
        </ToneBadge>
      ) : (
        <ToneBadge tone="neutral" title="fixtures are ready; the daemon plans the baseline job on its next tick (pulse daemon) - nothing for you to do" className={cn("text-muted-foreground", className)}>
          baseline: waiting for the daemon
        </ToneBadge>
      );
    default:
      return (
        <ToneBadge tone="neutral" title="the daemon records the baseline once the loop's records and fixtures are in place" className={cn("text-muted-foreground", className)}>
          no baseline
        </ToneBadge>
      );
  }
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
// carries its revision, and a seeded copy says so instead of a plain check.
function GateFlag({ on, label, title, doc }: { on: boolean; label: string; title?: string; doc?: EvolutionDoc | null }) {
  const rev = doc?.record.current_rev;
  const seeded = on && !!doc?.seeded;
  return (
    <ToneBadge tone={!on || seeded ? "warn" : "ok"} title={seeded ? seededTitle(doc?.seeded_from ?? "") : title}>
      {on ? <CheckIcon className="size-3" aria-label="yes" /> : <XIcon className="size-3" aria-label="no" />}
      {label + (on && rev ? " r" + num(rev) : "") + (seeded ? " · seeded" : "")}
    </ToneBadge>
  );
}

// ----- the inbox card ----------------------------------------------------------

// The steps only the owner can take. The baseline is not one of them since
// ADR-031: the daemon measures it, so no gate card ever asks for it.
export type GateKind = "seed" | "constitution" | "records" | "heldout";

export interface GateStep {
  kind: GateKind;
  label: string; // the short "what is missing" badge
  slug: string;
  slugs?: string[]; // every project one shared step serves, when the inbox folds them (the held-out set is one per machine)
  level: EvolveLevel;
  title: string;
  body: string;
  command: string;
}

const GATE_LABEL: Record<GateKind, string> = { seed: "no fixtures", constitution: "no constitution", records: "records missing", heldout: "your one step" };

// Which of the loop's two records a project lacks: what the row says first
// (null is "none"), the title the seed command names when the row is silent.
function missingRecords(g: EvolveGate, next: string): string[] {
  const fromRow = [g.constitution === null ? "constitution" : "", g.evaluators === null ? "evaluators" : ""].filter(Boolean);
  return fromRow.length ? fromRow : [/--title Evaluators\b/.test(next) ? "evaluators" : "constitution"];
}

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
    return {
      ...base,
      kind: "records",
      label: one ? "no " + one : GATE_LABEL.records,
      title: "Seed the loop's records for " + slug,
      body: "The loop reads its bounds from the constitution and evaluators records. Onboarding seeds a copy of the template's text; this project has " + (one ? "no " + one + " record" : "neither") + " yet, so seed it with the command below and make the text yours from the Evolution tab.",
      command: next,
    };
  }
  if (next.startsWith("pulse record new evolution"))
    return {
      ...base,
      kind: "constitution",
      label: GATE_LABEL.constitution,
      title: "Give " + slug + " a constitution",
      body: "The loop reads its bounds from the constitution record and stays at level B (record only) without one. Write it in your words, then index it with the command below.",
      command: next,
    };
  if (next.startsWith("pulse evolve fixture seed"))
    return {
      ...base,
      kind: "seed",
      label: GATE_LABEL.seed,
      title: "Seed the starter fixtures for " + slug,
      body: "No enabled public fixture yet, so nothing can be measured. The starter set gives the loop six small checks to begin with; your own fixtures can join them later.",
      command: next,
    };
  if (next.startsWith("pulse evolve setup"))
    return {
      ...base,
      kind: "heldout",
      label: GATE_LABEL.heldout,
      title: "Your one step: two private held-out fixtures",
      body: "The daemon does everything else - it measured the baseline and runs each cycle. Two fixtures only you can see are your proof the loop is not grading itself (ADR-012). evolve.heldout is on owner, so they are yours: write them here, outside every checkout - the hub writes the files and never reads them back - or with the command; the loop for " + slug + " opens from there. On auto the daemon writes them itself (ADR-033).",
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
    body: "The daemon does everything else - it measured each baseline and runs each cycle. Two fixtures only you can see are your proof the loop is not grading itself (ADR-012). Write them here, outside every checkout - the hub writes the files and never reads them back - or with the command; one set serves every project, so the loops for " + andList(slugs) + " open from there.",
  };
  const rest = steps.filter((s) => s.kind !== "heldout");
  const at = steps.indexOf(shared[0]);
  return [...rest.slice(0, at), one, ...rest.slice(at)];
}

// One waiting human step as an inbox card: what, where, why, the way to take
// it, and the way to the project's self-improvement view. The held-out card
// (ADR-012) says it is the one step that is the owner's and opens the
// Held-out fixtures dialog (ADR-032), the command kept as the terminal
// alternative; the rest name what is missing with the command to copy. A
// folded held-out card links every project it serves.
export function GateCard({ step }: { step: GateStep }) {
  const heldout = step.kind === "heldout";
  const slugs = step.slugs && step.slugs.length > 1 ? step.slugs : [step.slug];
  return (
    <ItemCard highlight ariaLabel={step.title}>
      <ItemCardHeader right={<LevelBadge level={step.level} />}>
        <ToneBadge tone="accent">self-improvement</ToneBadge>
        <ToneBadge tone="warn">{step.label}</ToneBadge>
        {slugs.length > 1 ? <span className="text-xs text-muted-foreground">{plural(slugs.length, "project")}</span> : <ProjectLink slug={step.slug} className="text-xs" />}
      </ItemCardHeader>
      <ItemCardTitle>{step.title}</ItemCardTitle>
      <ItemCardBody clamp={3}>{step.body}</ItemCardBody>
      {heldout ? <TerminalAlternative command={step.command} /> : <CommandBox command={step.command} />}
      <ItemCardActions>
        <span className="flex min-w-0 flex-wrap items-center gap-x-1 gap-y-0.5">
          {slugs.map((slug) => (
            <Button key={slug} asChild variant="ghost" size="xs" className="text-muted-foreground">
              <Link href={hrefs.project(slug, "evolution")} title={"self-improvement for " + slug}>
                {slugs.length > 1 ? slug : "self-improvement"} <ExternalLinkIcon />
              </Link>
            </Button>
          ))}
        </span>
        <span className="ml-auto shrink-0">{heldout ? <HeldoutButton projects={slugs} variant="outline" /> : <CopyButton text={step.command} variant="outline" />}</span>
      </ItemCardActions>
    </ItemCard>
  );
}

// ----- the project's panel -----------------------------------------------------

// The named step (title, why, the way to take it) the way the inbox card says
// it, for the panel's "next human step": the same words in both places. The
// held-out step has its button here too, the command under it.
function StepBlock({ step }: { step: GateStep }) {
  const heldout = step.kind === "heldout";
  return (
    <div className={cn("rounded-md border px-3 py-2.5", heldout && "border-primary/40 bg-primary/[0.03]")}>
      <div className="flex flex-wrap items-center gap-1.5">
        <ToneBadge tone={heldout ? "ok" : "warn"}>{step.label}</ToneBadge>
        <span className="text-[13px] font-medium leading-snug">{step.title}</span>
      </div>
      <p className="mt-1 text-xs leading-snug text-muted-foreground anywhere">{step.body}</p>
      {heldout ? (
        <>
          <div className="mt-2">
            <HeldoutButton projects={[step.slug]} />
          </div>
          <TerminalAlternative command={step.command} className="mt-2" />
        </>
      ) : (
        <CommandBox command={step.command} copy className="mt-2 bg-background/60" />
      )}
    </div>
  );
}

// What the panel says under a named step: the held-out step is taken here or
// in a terminal (ADR-032); every other step is the owner's to run in a terminal.
const stepFootnote = (kind: GateKind | null) => (kind === "heldout" ? "Done here or in a terminal; the console writes the two files and never reads them back." : "Yours to run in a terminal on the hub machine. The console names the step and never starts a measurement.");

// The held-out badge: counts only, and since ADR-033 how many of the ready
// fixtures the daemon wrote sealed - weaker evidence than yours, so it is
// said where the count is said. An older hub sends no `generated`.
type HeldoutCounts = { count: number; pending: number; generated?: number };
export function heldoutLabel(h: HeldoutCounts): string {
  const gen = h.generated ?? 0;
  return "held-out " + num(h.count) + (gen ? " (" + (gen === h.count ? "generated" : num(gen) + " generated") + ")" : "") + (h.pending ? " · " + num(h.pending) + " pending" : "");
}
export function heldoutTitle(h: HeldoutCounts): string {
  const gen = h.generated ?? 0;
  if (!gen) return "your held-out evaluators - counted here, never named";
  return (gen === h.count ? "all " : num(gen) + " of ") + plural(h.count, "held-out fixture") + " written by the daemon, sealed (ADR-033) - rewrite any from the dialog to make it yours; a fixture you write is never overwritten";
}

// Level, reason and next step, every gate as a badge, then the fixtures.
// `gates` null means the hub could not evaluate them: the error shows, the
// fixtures still list. `docs` (the constitution and evaluators as the
// evolution payload carries them) lets the record flags show revision and seed state.
export function SelfImprovementPanel({ gates, error, fixtures, docs, className, canDecide = true }: { gates: EvolveGates | null | undefined; error?: string; fixtures: EvolveFixture[]; docs?: { constitution: EvolutionDoc | null; evaluators: EvolutionDoc | null }; className?: string; canDecide?: boolean }) {
  const now = useNow();
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
      return "Nothing waits for you - the baseline measurement failed " + rel(bl.at, now) + "; the daemon retries " + (hrs ? "in about " + num(hrs) + " h" : "on its next tick") + ".";
    }
    if (bl?.kind === "measuring") return "Nothing waits for you - the daemon is " + baselineJobWord(bl.status) + (bl.status === "leased" ? " the baseline" : " to measure the baseline") + " (" + jobRef(bl.id) + ").";
    if (none.why && !optional) return "Nothing waits for you - " + none.why.replace(/\.$/, "") + ".";
    if (gates?.level === "D") return planning.next;
    return "Nothing waits for you - every gate is open" + (gates?.loop ? " and the loop is on." : "; cycles run once evolve.loop is on in Settings.");
  };
  return (
    <Panel title="Self-improvement" actions={gates ? <LevelBadge level={gates.level} /> : undefined} className={className}>
      {gates && g ? (
        <div className="grid gap-3">
          <p className="text-[13px] leading-snug anywhere">
            <b>{"Level " + (gates.level || "?")}</b>
            {gates.reason ? <span className="text-muted-foreground">{" · " + gates.reason}</span> : null}
          </p>
          <div className="grid gap-1.5 rounded-lg border bg-muted/30 px-3 py-2">
            <div className="flex items-center justify-between gap-2 text-xs">
              <span className="text-muted-foreground">{LEVELS}</span>
              <LevelBadge level={gates.level} />
            </div>
            <Progress value={LEVEL_PROGRESS[gates.level] ?? 0} className="h-1.5" />
            <p className="text-[11px] text-muted-foreground">Gate readiness, not measured improvement. A baseline is not a completed cycle.</p>
          </div>
          <div className="flex flex-wrap items-center gap-1.5 text-xs">
            {gates.stop ? <ToneBadge tone="bad" title="STOP is set - the loop halts until you clear it">STOP</ToneBadge> : null}
            <ToneBadge tone={gates.loop ? "ok" : "neutral"} dot title="evolve.loop - whether cycles may run back to back">
              {gates.loop ? "loop on" : "loop off"}
            </ToneBadge>
            <GateFlag on={gates.constitution} label="constitution" title="the EVO-constitution record is in the store" doc={docs?.constitution} />
            <GateFlag on={gates.evaluators} label="evaluators" title="the EVO-evaluators record is in the store" doc={docs?.evaluators} />
            <ToneBadge tone={gates.public_fixtures ? "ok" : "warn"} title="enabled public fixtures - the checks agents may see">
              {plural(gates.public_fixtures, "fixture")}
            </ToneBadge>
            <BaselineChip g={g} score={gates.baseline} recordedBy={gates.baseline_id} />
            {h ? (
              <ToneBadge tone={h.problem ? "warn" : "neutral"} title={h.problem || heldoutTitle(h)}>
                {heldoutLabel(h)}
              </ToneBadge>
            ) : null}
            {/* The set exists and the gate has moved on: the dialog's done state (add one, rewrite one) stays reachable from here (ADR-032). */}
            {h && h.count > 0 && step?.kind !== "heldout" ? <HeldoutButton projects={[gates.project]} size="xs" variant="ghost" label="Add or rewrite…" title="Held-out fixtures: add another or rewrite one - the console never reads them back" className="h-5 px-1.5 text-[11px] text-muted-foreground" /> : null}
            {gates.product_scope ? (
              <ToneBadge tone={gates.product_scope.allowed ? "ok" : "neutral"} title={gates.product_scope.reason || undefined}>
                {gates.product_scope.allowed ? "product changes allowed" : "operating layer only"}
              </ToneBadge>
            ) : null}
            {gates.scope ? (
              <ToneBadge tone={gates.scope.editable.length ? "ok" : "warn"} title={gates.scope.reason || undefined}>
                {gates.scope.editable.length
                  ? plural(gates.scope.editable.length, "editable class") + " of " + num(gates.scope.classes)
                  : "no editable file class"}
              </ToneBadge>
            ) : null}
            {gates.cap_reached ? <ToneBadge tone="bad" title={gates.limit_problem || "a cap the constitution writes is reached"}>cap reached</ToneBadge> : null}
          </div>
          <ItemCardMeta>
            <span className="tnum" title="Non-baseline measured archives since the current loop-enable event. Landing may still be pending or fail.">{plural(gates.cycles, "experiment archive") + " since loop enabled" + (gates.hours ? " (" + num(Math.round(gates.hours)) + " h)" : "")}</span>
            <span className="tnum" title="All non-baseline archive history. Kept does not mean landed or published.">{num(gates.kept) + " kept · " + num(gates.discarded) + " discarded (all history)"}</span>
            {gates.voted?.tried ? <span className="tnum" title="suggestions the board voted for that were tried">{"voted " + num(gates.voted.kept) + "/" + num(gates.voted.tried) + " kept"}</span> : null}
            {gates.signal?.tried ? <span className="tnum" title="signal fixtures tried">{"signal " + num(gates.signal.kept) + "/" + num(gates.signal.tried) + " kept"}</span> : null}
            {gates.limits && gates.limits !== "none" ? <Mono>{"limits " + gates.limits}</Mono> : null}
            {gates.tolerance ? <span className="tnum">{"tolerance " + gates.tolerance}</span> : null}
          </ItemCardMeta>
          <div className="grid gap-1.5 rounded-md border px-3 py-2.5">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">Experiment planning</span>
              <ToneBadge tone={planning.tone}>{planning.label}</ToneBadge>
            </div>
            <p className="max-h-40 overflow-auto whitespace-pre-wrap text-xs text-muted-foreground anywhere">{planning.reason}</p>
            {openQuestion ? (
              <div className="flex flex-wrap items-center gap-2 rounded-md border border-warn/40 bg-warn/5 px-2.5 py-2 text-xs">
                <span className="min-w-0 flex-1 anywhere">
                  <b>Waiting for your yes or no:</b> {openQuestion.summary}{" "}
                  <Link href={hrefs.post(gates.project, openQuestion.id)} className="underline underline-offset-2">
                    open the post
                  </Link>
                </span>
                <DecideButtons id={openQuestion.id} summary={openQuestion.summary} stop={openQuestion.stop} canDecide={canDecide} size="xs" />
              </div>
            ) : questionId && questions.data && !openQuestion ? (
              <p className="text-[11.5px] text-muted-foreground">Question {questionId} is already decided or closed; the loop plans past it on the daemon&apos;s next tick.</p>
            ) : null}
            {gates.planning?.job || gates.planning?.suggestion ? (
              <ItemCardMeta>
                {gates.planning.job ? <Mono>{gates.planning.job}</Mono> : null}
                {gates.planning.suggestion ? <Link href={hrefs.post(gates.project, gates.planning.suggestion)} className="hover:underline">View suggestion</Link> : null}
              </ItemCardMeta>
            ) : null}
          </div>
          <div>
            <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">Next human step</div>
            {waits && step ? (
              <>
                <StepBlock step={step} />
                <p className="mt-1 text-[11.5px] text-muted-foreground">{stepFootnote(step.kind)}</p>
              </>
            ) : waits ? (
              <>
                <CommandBox command={next} copy />
                <p className="mt-1 text-[11.5px] text-muted-foreground">{stepFootnote(null)}</p>
              </>
            ) : (
              <>
                <p className="rounded-md border border-dashed px-3 py-2 text-xs text-muted-foreground anywhere">{idle()}</p>
                {optional ? (
                  <>
                    <CommandBox command={optional} copy className="mt-1.5" />
                    <p className="mt-1 text-[11.5px] text-muted-foreground">Optional: queues the measurement now instead of waiting for the daemon&apos;s retry.</p>
                  </>
                ) : null}
              </>
            )}
          </div>
        </div>
      ) : (
        <Notice tone="warn" className="text-[13px]">
          <div className="font-medium">The hub could not evaluate the self-improvement gates.</div>
          <div className="mt-0.5 text-muted-foreground">{error || "It gave no reason."}</div>
        </Notice>
      )}
      <FixtureList fixtures={fixtures} className="mt-4" />
    </Panel>
  );
}

// ----- the record's provenance (ADR-030) --------------------------------------

// The revision and id of a constitution or evaluators record, for a panel's
// header: `r1 · seeded` warns that the text is still the template's.
export function DocRev({ doc, slug }: { doc: EvolutionDoc; slug: string }) {
  const r = doc.record;
  const waiting = doc.draft ? " · draft ready" : doc.seeded && doc.draft_job ? " · drafting…" : doc.seeded ? " · seeded" : "";
  return (
    <>
      <span className="tnum" title={doc.draft ? "an agent drafted the text - read and adopt it below" : doc.seeded ? seededTitle(doc.seeded_from ?? "") : doc.seeded_from ? "started as a copy of " + doc.seeded_from + ", edited since" : "revision " + num(r.current_rev)}>
        {"r" + num(r.current_rev) + waiting}
      </span>
      <Link href={hrefs.record(r.id, slug)} className="hover:underline">
        {r.id}
      </Link>
    </>
  );
}

// The fixtures as compact rows: name, origin, kind, state, weight and who
// added it when. Never the request or the scorer, never an edit control.
export function FixtureList({ fixtures, className }: { fixtures: EvolveFixture[]; className?: string }) {
  const enabled = fixtures.filter((f) => f.enabled).length;
  return (
    <section className={className} aria-label="Fixtures">
      <div className="mb-1.5 flex items-center justify-between gap-2 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
        <span>Fixtures</span>
        <span className="tnum normal-case tracking-normal">{fixtures.length ? num(enabled) + " enabled of " + num(fixtures.length) : ""}</span>
      </div>
      {fixtures.length ? (
        <ul className="grid gap-1.5">
          {fixtures.map((f) => (
            <li key={f.kind + "/" + f.name} className={cn("flex flex-wrap items-center gap-x-2 gap-y-1 rounded-md border px-2.5 py-1.5 text-xs", !f.enabled && "opacity-60")}>
              <Mono className="font-medium">{f.name}</Mono>
              {f.starter ? <ToneBadge tone="info" title="part of the starter set seeded at onboarding">starter</ToneBadge> : null}
              <ToneBadge tone={f.kind === "public" ? "accent" : "neutral"} title={f.kind === "public" ? "public: agents may see it; it counts in the score" : "signal: a hint the loop reads; it does not count in the score"}>
                {f.kind}
              </ToneBadge>
              <ToneBadge tone={f.enabled ? "ok" : "neutral"} dot>
                {f.enabled ? "enabled" : "disabled"}
              </ToneBadge>
              <span className="ml-auto flex flex-wrap items-center gap-x-2 gap-y-1 text-muted-foreground">
                <span className="tnum" title="weight in the score">{"weight " + f.weight}</span>
                <Who row={{ agent: f.created_by }} compact />
                <When d={f.at} />
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="rounded-md border border-dashed px-3 py-2 text-xs text-muted-foreground anywhere">
          No fixture yet, so the loop has nothing to measure against. <Mono>pulse evolve fixture seed</Mono> gives it the starter set of six small checks.
        </p>
      )}
      <p className="mt-1.5 text-[11.5px] text-muted-foreground">Names and provenance only: what a fixture asks and how it scores stay in the store, and fixtures are added or changed from the terminal.</p>
    </section>
  );
}
