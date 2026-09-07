"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, CopyIcon, ExternalLinkIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "./item-card";
import { Notice, Panel } from "./panel";
import { Mono, ProjectLink, ToneBadge, When, Who } from "./who";
import { cn } from "@/lib/utils";
import { num, plural } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import type { EvolveFixture, EvolveGate, EvolveGates, EvolveLevel } from "@/lib/api";

// The self-improvement surfaces (ADR-029). A project's loop opens only with
// enabled public fixtures and a recorded baseline; the console shows both,
// lists the fixtures and names the one human step that opens the next gate
// as a command to copy. Nothing here starts a measurement or edits a
// fixture - those stay in the terminal on purpose.

const LEVELS = "A observe · B record · C rehearse · D evolve";
const LEVEL_TITLE = "highest self-improvement level allowed right now (" + LEVELS + ")";

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
    <div className={cn("flex items-start gap-2 rounded-md border bg-muted/40 px-2.5 py-1.5", className)}>
      <code className="min-w-0 flex-1 whitespace-pre-wrap font-mono text-[11.5px] leading-relaxed anywhere">{command}</code>
      {copy ? <CopyButton text={command} size="xs" variant="outline" className="-my-0.5 shrink-0" /> : null}
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

// One line under a project card's tiles: how far the loop may go and why.
export function EvolveBadges({ g, className }: { g: EvolveGate; className?: string }) {
  return (
    <div className={cn("flex flex-wrap items-center gap-1.5 text-xs", className)}>
      <ToneBadge tone={g.fixtures_enabled ? "ok" : "neutral"} className={g.fixtures_enabled ? undefined : "text-muted-foreground"} title="enabled public fixtures - the checks agents may see; every experiment is measured against them">
        {plural(g.fixtures_enabled, "fixture")}
      </ToneBadge>
      <ToneBadge tone={g.baseline ? "ok" : "warn"} title={g.baseline ? "a kept experiment recorded the baseline" : "no kept experiment has recorded the baseline yet - the loop stays closed"}>
        {g.baseline ? "baseline" : "no baseline"}
      </ToneBadge>
      <LevelBadge level={g.evolve_level} />
    </div>
  );
}

function GateFlag({ on, label, title }: { on: boolean; label: string; title?: string }) {
  return (
    <ToneBadge tone={on ? "ok" : "warn"} title={title}>
      {on ? <CheckIcon className="size-3" aria-label="yes" /> : <XIcon className="size-3" aria-label="no" />}
      {label}
    </ToneBadge>
  );
}

// ----- the inbox card ----------------------------------------------------------

export type GateKind = "baseline" | "seed" | "constitution";

export interface GateStep {
  kind: GateKind;
  slug: string;
  level: EvolveLevel;
  title: string;
  body: string;
  command: string;
}

const GATE_LABEL: Record<GateKind, string> = { baseline: "no baseline", seed: "no fixtures", constitution: "no constitution" };

const baselineCommand = (slug: string) => "pulse job add --project " + slug + " --kind experiment --title 'Establish baseline' --brief 'Hypothesis: ONE_VARIABLE'";

// Which human step, if any, a project waits for. One card per project: the
// hub's `evolve_next` names the step; the fixtures/baseline pair is the gate
// ADR-029 is about, so it gets its card whenever fixtures exist and no
// baseline does.
export function gateStep(slug: string, g: EvolveGate | null): GateStep | null {
  if (!g) return null;
  const next = g.evolve_next.trim();
  const base = { slug, level: g.evolve_level };
  if (next.startsWith("pulse record new evolution"))
    return {
      ...base,
      kind: "constitution",
      title: "Give " + slug + " a constitution",
      body: "The loop reads its bounds from the constitution record and stays at level B (record only) without one. Write it in your words, then index it with the command below.",
      command: next,
    };
  if (next.startsWith("pulse evolve fixture seed"))
    return {
      ...base,
      kind: "seed",
      title: "Seed the starter fixtures for " + slug,
      body: "No enabled public fixture yet, so nothing can be measured. The starter set gives the loop six small checks to begin with; your own fixtures can join them later.",
      command: next,
    };
  if (g.fixtures_enabled > 0 && !g.baseline)
    return {
      ...base,
      kind: "baseline",
      title: "Record the baseline for " + slug,
      body: (g.fixtures_enabled === 1 ? "One public fixture is ready" : num(g.fixtures_enabled) + " public fixtures are ready") + "; the loop stays closed until a kept experiment records the baseline. This runs real agent runs (3 per fixture) - queue it when you are ready.",
      command: next.startsWith("pulse job add") ? next : baselineCommand(slug),
    };
  return null;
}

// One waiting human step as an inbox card: what, where, why, the command to
// copy and the way to the project's self-improvement view.
export function GateCard({ step }: { step: GateStep }) {
  return (
    <ItemCard highlight ariaLabel={step.title}>
      <ItemCardHeader right={<LevelBadge level={step.level} />}>
        <ToneBadge tone="accent">self-improvement</ToneBadge>
        <ToneBadge tone="warn">{GATE_LABEL[step.kind]}</ToneBadge>
        <ProjectLink slug={step.slug} className="text-xs" />
      </ItemCardHeader>
      <ItemCardTitle>{step.title}</ItemCardTitle>
      <ItemCardBody clamp={3}>{step.body}</ItemCardBody>
      <CommandBox command={step.command} />
      <ItemCardActions>
        <Button asChild variant="ghost" size="xs" className="text-muted-foreground">
          <Link href={hrefs.project(step.slug, "evolution")}>
            self-improvement <ExternalLinkIcon />
          </Link>
        </Button>
        <span className="ml-auto shrink-0">
          <CopyButton text={step.command} />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}

// ----- the project's panel -----------------------------------------------------

const score = (n: number) => (Number.isFinite(n) ? n.toFixed(3).replace(/\.?0+$/, "") : "—");

// Level, reason and next step, every gate as a badge, then the fixtures.
// `gates` null means the hub could not evaluate them: the error shows, the
// fixtures still list.
export function SelfImprovementPanel({ gates, error, fixtures, className }: { gates: EvolveGates | null | undefined; error?: string; fixtures: EvolveFixture[]; className?: string }) {
  const next = (gates?.next_human_step ?? "").trim();
  const waits = !!next && next !== "none";
  const h = gates?.heldout;
  return (
    <Panel title="Self-improvement" actions={gates ? <LevelBadge level={gates.level} /> : undefined} className={className}>
      {gates ? (
        <div className="grid gap-3">
          <p className="text-[13px] leading-snug anywhere">
            <b>{"Level " + (gates.level || "?")}</b>
            {gates.reason ? <span className="text-muted-foreground">{" · " + gates.reason}</span> : null}
          </p>
          <div className="flex flex-wrap items-center gap-1.5 text-xs">
            {gates.stop ? <ToneBadge tone="bad" title="STOP is set - the loop halts until you clear it">STOP</ToneBadge> : null}
            <ToneBadge tone={gates.loop ? "ok" : "neutral"} dot title="evolve.loop - whether cycles may run back to back">
              {gates.loop ? "loop on" : "loop off"}
            </ToneBadge>
            <GateFlag on={gates.constitution} label="constitution" title="the EVO-constitution record is in the store" />
            <GateFlag on={gates.evaluators} label="evaluators" title="the EVO-evaluators record is in the store" />
            <ToneBadge tone={gates.public_fixtures ? "ok" : "warn"} title="enabled public fixtures - the checks agents may see">
              {plural(gates.public_fixtures, "fixture")}
            </ToneBadge>
            <ToneBadge tone={gates.baseline !== null && gates.baseline !== undefined ? "ok" : "warn"} title={gates.baseline_id ? "recorded by " + gates.baseline_id : "a kept experiment records the baseline score every variant is measured against"}>
              {gates.baseline !== null && gates.baseline !== undefined ? "baseline " + score(gates.baseline) : "no baseline"}
            </ToneBadge>
            {h ? (
              <ToneBadge tone={h.problem ? "warn" : "neutral"} title={h.problem || "your held-out evaluators - counted here, never named"}>
                {"held-out " + num(h.count) + (h.pending ? " · " + num(h.pending) + " pending" : "")}
              </ToneBadge>
            ) : null}
            {gates.product_scope ? (
              <ToneBadge tone={gates.product_scope.allowed ? "ok" : "neutral"} title={gates.product_scope.reason || undefined}>
                {gates.product_scope.allowed ? "product changes allowed" : "operating layer only"}
              </ToneBadge>
            ) : null}
            {gates.cap_reached ? <ToneBadge tone="bad" title={gates.limit_problem || "a cap the constitution writes is reached"}>cap reached</ToneBadge> : null}
          </div>
          <ItemCardMeta>
            <span className="tnum">{plural(gates.cycles, "cycle") + (gates.hours ? " in " + num(Math.round(gates.hours)) + " h" : "")}</span>
            <span className="tnum">{num(gates.kept) + " kept · " + num(gates.discarded) + " discarded"}</span>
            {gates.voted?.tried ? <span className="tnum" title="suggestions the board voted for that were tried">{"voted " + num(gates.voted.kept) + "/" + num(gates.voted.tried) + " kept"}</span> : null}
            {gates.signal?.tried ? <span className="tnum" title="signal fixtures tried">{"signal " + num(gates.signal.kept) + "/" + num(gates.signal.tried) + " kept"}</span> : null}
            {gates.limits && gates.limits !== "none" ? <Mono>{"limits " + gates.limits}</Mono> : null}
            {gates.tolerance ? <span className="tnum">{"tolerance " + gates.tolerance}</span> : null}
          </ItemCardMeta>
          <div>
            <div className="mb-1 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">Next human step</div>
            {waits ? (
              <>
                <CommandBox command={next} copy />
                <p className="mt-1 text-[11.5px] text-muted-foreground">Yours to run in a terminal on the hub machine. The console names the step and never starts a measurement.</p>
              </>
            ) : (
              <p className="rounded-md border border-dashed px-3 py-2 text-xs text-muted-foreground">{"Nothing waits for you - every gate is open" + (gates.loop ? " and the loop is on." : "; cycles run once evolve.loop is on in Settings.")}</p>
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

// The fixtures as compact rows: name, origin, kind, state, weight and who
// added it when. Never the request or the scorer, never an edit control.
export function FixtureList({ fixtures, className }: { fixtures: EvolveFixture[]; className?: string }) {
  const enabled = fixtures.filter((f) => f.enabled).length;
  return (
    <section className={className} aria-label="Fixtures">
      <div className="mb-1.5 flex items-center justify-between gap-2 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">
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
