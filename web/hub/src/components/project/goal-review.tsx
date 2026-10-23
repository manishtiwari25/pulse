"use client";

import Link from "next/link";
import { GaugeIcon, LightbulbIcon, MessageSquareQuoteIcon, PencilIcon, PlusIcon, SparklesIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DecideButtons } from "@/components/pulse/decide-buttons";
import { PulseMascot, type PipMood } from "@/components/pulse/mascots";
import { ErrorNotice, SkeletonRows } from "@/components/pulse/panel";
import { ToneBadge, When } from "@/components/pulse/who";
import { projectPath, type GoalAdvice, type GoalReview, type GoalReviewPayload, type GoalSignal, type GoalVerdict } from "@/lib/api";
import type { Tone } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

const VERDICT: Record<GoalVerdict, { word: string; tone: Tone; mood: PipMood; blurb: string }> = {
  "on-track": { word: "On track", tone: "ok", mood: "happy", blurb: "Recent work lands, passes its checks and speaks to the goal." },
  slipping: { word: "Slipping", tone: "warn", mood: "worried", blurb: "Less lands than is started, or the work drifts from the goal." },
  stalled: { word: "Stalled", tone: "bad", mood: "lost", blurb: "Little or nothing reached the goal lately." },
  quiet: { word: "Too quiet to score", tone: "neutral", mood: "asleep", blurb: "Not enough happened in the window to say." },
};

const SIGNAL_HINT: Record<string, string> = {
  delivery: "Work finished and landed, against what failed or still waits.",
  reliability: "Landings that passed their checks, against those that failed.",
  direction: "Recent tasks, jobs and posts that speak to the goal's own words.",
  momentum: "Moves in this window against the one before.",
};

// The review card under the goal: a score ring with the verdict, one bar per
// signal, the advice with its action, the lead's last reading and the goal
// the lead proposes (taken or dismissed with the board's decide).
export function GoalReviewCard({ slug, canEdit, onEdit }: { slug: string; canEdit: boolean; onEdit: () => void }) {
  const q = useApi<GoalReviewPayload>(projectPath(slug, "goal/review"), { refresh: 60_000 });
  const r = q.data?.review;
  if (q.error) {
    // An older hub has no review route: the goal panel still works without it.
    if (/not found|HTTP 404/i.test(q.error)) return null;
    return <ErrorNotice error={q.error} onRetry={() => void q.reload()} />;
  }
  if (!r) return q.loading ? <SkeletonRows n={3} /> : null;
  const v = VERDICT[r.verdict] ?? VERDICT.quiet;
  // An on-track score with a failing signal underneath should not read as all good.
  const caveat = r.advice.some((a) => a.kind === "unreliable" || a.kind === "drift");
  const blurb = r.verdict === "on-track" && caveat ? "Work reaches the goal, with something to fix below." : v.blurb;
  const leadScore = r.lead && r.lead.fresh && r.lead.score !== null ? r.lead.score : null;
  return (
    <section aria-label="Goal review" className="enter grid gap-4 rounded-2xl border border-border/70 bg-card/60 p-4 sm:p-5">
      <div className="flex flex-wrap items-start gap-4">
        <ScoreRing score={r.scored ? r.score : null} tone={v.tone} />
        <div className="grid min-w-0 flex-1 basis-56 gap-1.5">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="flex items-center gap-1.5 text-[15px] font-semibold tracking-tight">
              <GaugeIcon className="size-4 text-muted-foreground" aria-hidden /> Goal review
            </h3>
            <ToneBadge tone={v.tone} dot>{v.word}</ToneBadge>
            {r.review_due ? <ToneBadge tone="neutral" title={"The lead posts a reading every " + r.review_days + " days; none is that recent"}>Lead review due</ToneBadge> : null}
          </div>
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            {r.scored ? (
              <>
                <span className="font-medium text-foreground tnum">{r.score} of 100</span> over the last {r.window_days} days
                {leadScore !== null ? <> - the store says <span className="tnum">{r.measured_score}</span>, the lead says <span className="tnum">{leadScore}</span></> : r.measurable < 100 ? <> ({r.measurable} of 100 measurable)</> : null}. {blurb}
              </>
            ) : (
              <>
                {v.blurb} Only {r.measurable} of 100 points could be measured over the last {r.window_days} days; the agents need to land something first.
              </>
            )}
          </p>
          <ul className="mt-1 grid gap-2">
            {r.signals.map((s) => <SignalBar key={s.name} s={s} />)}
          </ul>
        </div>
        <PulseMascot mood={v.mood} size={64} className="float hidden shrink-0 self-center sm:block" title={"Pip is " + v.word.toLowerCase()} />
      </div>

      {r.advice.length ? (
        <div className="grid gap-2">
          <h4 className="flex items-center gap-1.5 text-[13px] font-semibold uppercase tracking-wide text-muted-foreground">
            <LightbulbIcon className="size-3.5" aria-hidden /> What this says
          </h4>
          <ul className="grid gap-1.5">
            {r.advice.map((a) => (
              <AdviceRow key={a.kind} a={a} slug={slug} canEdit={canEdit} onEdit={onEdit} />
            ))}
          </ul>
        </div>
      ) : null}

      {r.lead ? (
        <div className="grid gap-1 rounded-xl bg-muted/40 px-3 py-2.5 text-[13px]">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-muted-foreground">
            <MessageSquareQuoteIcon className="size-3.5" aria-hidden />
            <span className="font-medium text-foreground">The lead&apos;s reading</span>
            <span>{r.lead.agent}</span>
            <span aria-hidden>·</span>
            <When d={r.lead.date} />
            {r.lead.score !== null ? <ToneBadge tone="neutral" className="tnum">score {r.lead.score}</ToneBadge> : null}
            {!r.lead.fresh ? <ToneBadge tone="neutral" title="Older than the review window; not blended into the score">older</ToneBadge> : null}
            <Link href={hrefs.post(slug, r.lead.post)} className="ml-auto underline underline-offset-2">Open the post</Link>
          </div>
          <p className="leading-relaxed anywhere">{r.lead.summary}</p>
        </div>
      ) : null}

      {r.proposal ? (
        <ProposalCard slug={slug} r={r} canDecide={q.data?.can_decide === true} />
      ) : null}
    </section>
  );
}

function ScoreRing({ score, tone }: { score: number | null; tone: Tone }) {
  const size = 92;
  const stroke = 8;
  const radius = (size - stroke) / 2;
  const circumference = 2 * Math.PI * radius;
  const pct = score === null ? 0 : Math.max(0, Math.min(100, score));
  const color = tone === "ok" ? "var(--ok)" : tone === "warn" ? "var(--warn)" : tone === "bad" ? "var(--bad)" : "var(--muted-foreground)";
  return (
    <div className="pop relative shrink-0" role="img" aria-label={score === null ? "No score yet" : "Goal score " + score + " of 100"} style={{ width: size, height: size }}>
      <svg viewBox={"0 0 " + size + " " + size} width={size} height={size} className="-rotate-90">
        <circle cx={size / 2} cy={size / 2} r={radius} fill="none" stroke="var(--muted)" strokeWidth={stroke} />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill="none"
          stroke={color}
          strokeWidth={stroke}
          strokeLinecap="round"
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - pct / 100)}
          className="transition-[stroke-dashoffset] duration-700 ease-out"
        />
      </svg>
      <div className="absolute inset-0 grid place-items-center">
        {score === null ? (
          <span className="text-lg font-semibold text-muted-foreground">–</span>
        ) : (
          <span className="text-2xl font-semibold tracking-tight tnum">{score}</span>
        )}
      </div>
    </div>
  );
}

function SignalBar({ s }: { s: GoalSignal }) {
  const pct = s.measured && s.weight > 0 ? Math.round((100 * s.earned) / s.weight) : 0;
  const tone = !s.measured ? "bg-muted-foreground/30" : pct >= 65 ? "bg-ok" : pct >= 40 ? "bg-warn" : "bg-bad";
  return (
    <li className="grid grid-cols-[6.5rem_1fr_auto] items-center gap-x-3 gap-y-0.5 text-[13px]" title={SIGNAL_HINT[s.name] ?? s.label}>
      <span className="font-medium">{s.label}</span>
      <span className="relative flex h-2 w-full items-center overflow-hidden rounded-full bg-muted ring-1 ring-border/60 ring-inset" role="meter" aria-valuemin={0} aria-valuemax={s.weight} aria-valuenow={s.measured ? s.earned : undefined} aria-label={s.label + (s.measured ? " " + s.earned + " of " + s.weight : " not measured")}>
        <span className={cn("h-full rounded-full transition-[width] duration-700 ease-out", tone)} style={{ width: s.measured ? Math.max(pct, 2) + "%" : "0%" }} />
      </span>
      <span className="text-muted-foreground tnum">{s.measured ? s.earned + "/" + s.weight : "–"}</span>
      <span className="col-span-3 text-xs leading-snug text-muted-foreground sm:col-start-2 sm:col-span-2">{s.detail}</span>
    </li>
  );
}

function AdviceRow({ a, slug, canEdit, onEdit }: { a: GoalAdvice; slug: string; canEdit: boolean; onEdit: () => void }) {
  const tone: Tone = a.kind === "achieved" ? "ok" : a.kind === "lead-proposes" ? "info" : a.kind === "stalled" || a.kind === "unreliable" ? "bad" : a.kind === "review-due" || a.kind === "quiet" ? "neutral" : "warn";
  let action: React.ReactNode = null;
  if (a.action === "Edit goal" && canEdit) {
    action = (
      <Button size="xs" variant="outline" onClick={onEdit}>
        <PencilIcon /> Edit goal
      </Button>
    );
  } else if (a.action === "Add a task") {
    action = (
      <Button size="xs" variant="outline" asChild>
        <Link href={hrefs.project(slug, "work")}><PlusIcon /> Add a task</Link>
      </Button>
    );
  } else if (a.action === "Message the lead") {
    action = (
      <Button size="xs" variant="outline" asChild>
        <Link href={hrefs.project(slug, "work")}><MessageSquareQuoteIcon /> Message the lead</Link>
      </Button>
    );
  }
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-[13px] leading-relaxed">
      <ToneBadge tone={tone} dot className="shrink-0">{adviceWord(a.kind)}</ToneBadge>
      <span className="min-w-0 flex-1 basis-64 anywhere">{a.text}</span>
      {action}
    </li>
  );
}

function adviceWord(kind: string): string {
  switch (kind) {
    case "lead-proposes": return "Proposal";
    case "set-your-own": return "Your goal";
    case "quiet": return "Quiet";
    case "stalled": return "Stalled";
    case "slipping": return "Slipping";
    case "unreliable": return "Checks";
    case "drift": return "Drift";
    case "stale": return "Stale";
    case "vague": return "Vague";
    case "achieved": return "Reached";
    case "review-due": return "Lead";
    default: return kind;
  }
}

function ProposalCard({ slug, r, canDecide }: { slug: string; r: GoalReview; canDecide: boolean }) {
  const p = r.proposal;
  if (!p) return null;
  return (
    <div className="attn grid gap-2 rounded-xl border border-info/40 bg-info/5 px-3 py-3 text-[13px]">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-muted-foreground">
        <SparklesIcon className="size-3.5 text-info" aria-hidden />
        <span className="font-medium text-foreground">{p.agent} proposes a new goal</span>
        <span aria-hidden>·</span>
        <When d={p.date} />
        <Link href={hrefs.post(slug, p.post)} className="ml-auto underline underline-offset-2">Open the post</Link>
      </div>
      <blockquote className="whitespace-pre-wrap border-l-2 border-info/50 pl-3 text-[14px] leading-relaxed anywhere">{p.text}</blockquote>
      {p.why ? <p className="leading-relaxed text-muted-foreground anywhere"><span className="font-medium text-foreground">Why:</span> {p.why}</p> : null}
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground">Taking it replaces the goal above; dismissing it tells the lead to keep the current one.</span>
        <DecideButtons id={p.post} summary={"Proposed goal: " + p.text} canDecide={canDecide} size="sm" yesLabel="Use this goal" noLabel="Dismiss" />
      </div>
    </div>
  );
}
