"use client";

import { CircleCheckIcon, CircleDashedIcon, CircleSlashIcon, ClockIcon, CopyIcon, FilePenLineIcon, LightbulbIcon, LoaderIcon, PauseCircleIcon } from "lucide-react";
import { ItemCard, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Progress } from "@/components/ui/progress";
import { PostLink, ProjectLink, StatusBadge, ThreadLink, When } from "@/components/pulse/who";
import { cn } from "@/lib/utils";
import { expiresAt, num, plural, sentence, span } from "@/lib/format";
import { useNow } from "@/lib/use-api";
import type { Amendment, Suggestion } from "@/lib/api";

// The cards for what agents vote on: a suggestion (an idea for work) and a
// rule change (an amendment to a project's constitution). Both read as the
// idea itself, how the vote stands in plain numbers, and (for suggestions)
// what happens next, in one sentence. Ids, gates and hashes stay in tooltips.
// Shared by the Votes page and a project's Board tab.

type Tone = "ok" | "warn" | "bad" | "neutral" | "info";

// "1st", "2nd", "3rd", "11th".
function ordinal(n: number): string {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) return n + "th";
  return n + (n % 10 === 1 ? "st" : n % 10 === 2 ? "nd" : n % 10 === 3 ? "rd" : "th");
}

const whoName = (r: { agent?: string | null; name?: string | null; session?: string | null }) => {
  const n = r.agent || r.name || r.session || "";
  return n === "human" || n === "owner" ? "you" : n || "an agent";
};

// A planner gate, as a person would explain it. Unknown gates read as written.
function gateWords(gate: string, score: number): string {
  const g = gate.trim();
  if (/net votes\s*<=\s*0|score\s*>\s*0/i.test(g)) return score < 0 ? "more agents voted against than for" : "as many agents voted against it as for it";
  if (/signal/i.test(g)) return "it doesn't point at evidence yet (a file, post, job or check)";
  if (/owner.only|immutable/i.test(g)) return "it touches a file only you can change";
  if (/operating.layer/i.test(g)) return "today's limit for changes to how agents work is reached";
  if (/\bfile:|\bvariable:/i.test(g)) return "it doesn't name the file and the one thing to change";
  if (/addressed to the owner|for_owner/i.test(g)) return "it is addressed to you, so agents don't plan it";
  if (/tried|in.flight/i.test(g)) return "the same idea was already tried or is in progress";
  return g.replace(/^not planned:\s*/i, "").replace(/[.;]\s*$/, "");
}

const DUP = /\bdup(?:licate)?\s+of\s+(\S+)/i;

// What happens to a suggestion next, in one sentence.
function nextStep(s: Suggestion, quorum: number, now: number): { text: string; tone: Tone; icon: React.ComponentType<{ className?: string }>; title?: string } | null {
  const gate = (s.gate || "").trim();
  if (s.status === "rejected") return { text: "Turned down", tone: "neutral", icon: CircleSlashIcon };
  if (s.job) {
    const title = "Job " + s.job + (s.job_status ? " (" + s.job_status + ")" : "");
    if (s.job_status === "leased" || s.job_status === "running") return { text: "An agent is working on it", tone: "info", icon: LoaderIcon, title };
    if (s.job_status === "blocked") return { text: "Its job is stuck. Check Runs to see why.", tone: "bad", icon: PauseCircleIcon, title };
    return { text: "A job is waiting to start", tone: "info", icon: ClockIcon, title };
  }
  if (s.status === "eligible") {
    if (gate && !DUP.test(gate)) return { text: "Not planned yet: " + gateWords(gate, s.score), tone: "warn", icon: CircleDashedIcon, title: gate };
    if (s.score <= 0) return { text: "Not planned: " + gateWords("net votes <= 0", s.score), tone: "warn", icon: CircleDashedIcon };
    // The rank is per project: only the first one is up next.
    const behind = s.rank && s.rank > 1 ? ", " + ordinal(s.rank) + " in line" : "";
    if (s.plannable && !behind) return { text: "Next in line. The runner plans it on its next pass.", tone: "ok", icon: CircleCheckIcon };
    if (s.plannable) return { text: "Ready to plan" + behind + ". The runner takes it when its turn comes.", tone: "ok", icon: CircleCheckIcon };
    return { text: "Agreed" + behind + ". The runner plans it when a place frees up.", tone: "ok", icon: CircleCheckIcon };
  }
  const need = Math.max(0, quorum - s.voters);
  if (need || s.status === "needs votes") {
    const exp = expiresAt(s);
    const left = exp ? (exp.getTime() - now) / 1000 : null;
    if (left !== null && left <= 0) {
      const closed = "Voting closed " + span(-left) + " ago";
      return { text: closed + (need ? ", " + plural(need, "vote") + " short." : " with the votes tied."), tone: "neutral", icon: ClockIcon };
    }
    const ask = need ? "Needs " + plural(need, "more vote") + "." : "Votes are tied; one more decides it.";
    return { text: ask + (left === null ? "" : " Closes in " + span(left) + "."), tone: "neutral", icon: ClockIcon };
  }
  return null;
}

const TONE_ICON: Record<Tone, string> = {
  ok: "text-ok",
  warn: "text-warn",
  bad: "text-bad",
  info: "text-primary",
  neutral: "text-muted-foreground",
};

// "2 of 2 votes" with a thin bar; past the bar it says how many it needed.
function VoteProgress({ voters, quorum, title }: { voters: number; quorum: number; title?: string }) {
  const q = Math.max(1, quorum);
  const label = voters > quorum ? plural(voters, "vote") + " (" + num(quorum) + " needed)" : num(voters) + " of " + plural(quorum, "vote");
  return (
    <span className="inline-flex min-w-0 items-center gap-2" title={title}>
      <Progress value={Math.min(100, (100 * voters) / q)} className="h-1.5 w-20 shrink-0" aria-label={label} />
      <span className="tnum">{label}</span>
    </span>
  );
}

function ForAgainst({ up, down }: { up: number; down: number }) {
  return (
    <span className="tnum">
      {up} for, {down} against
    </span>
  );
}

// ----- suggestion ---------------------------------------------------------

export function SuggestionCard({ s, quorum, slug, detail }: { s: Suggestion; quorum: number; slug: string; detail?: React.ReactNode }) {
  const now = useNow();
  const project = s.slug || slug;
  const q = s.quorum ?? quorum;
  const step = nextStep(s, q, now);
  const dup = DUP.exec(s.gate || "") || DUP.exec(s.summary || "");
  return (
    <ItemCard ariaLabel={"Suggestion: " + s.summary}>
      <ItemCardHeader right={<When d={s.date} />}>
        <ItemCardKind icon={LightbulbIcon}>{"Suggestion from " + whoName(s)}</ItemCardKind>
        {s.slug && s.slug !== slug ? <ProjectLink slug={s.slug} className="font-normal text-muted-foreground" /> : null}
      </ItemCardHeader>
      <ItemCardTitle>
        <PostLink slug={project} id={s.id} label={sentence(s.summary)} />
      </ItemCardTitle>
      <ItemCardMeta>
        <StatusBadge status={s.status} />
        <ForAgainst up={s.up} down={s.down} />
        <VoteProgress voters={s.voters} quorum={q} title={"Votes needed: " + q + " different agents (quorum)"} />
      </ItemCardMeta>
      {step ? (
        <p className="flex min-w-0 items-start gap-2 text-sm leading-relaxed text-foreground anywhere" title={step.title}>
          <step.icon className={cn("mt-0.5 size-4 shrink-0", TONE_ICON[step.tone])} aria-hidden />
          <span className="min-w-0">{step.text}</span>
        </p>
      ) : null}
      <ItemCardMeta className="mt-auto">
        <span className="inline-flex min-w-0 items-center gap-1">
          In <ThreadLink slug={project} k={s.thread_key} />
        </span>
        {dup ? (
          <span className="inline-flex items-center gap-1" title={"Duplicate of " + dup[1]}>
            <CopyIcon className="size-3.5" aria-hidden /> Same idea as an earlier suggestion
          </span>
        ) : null}
      </ItemCardMeta>
      {detail}
    </ItemCard>
  );
}

// ----- rule change (amendment) --------------------------------------------

// Why some votes on a rule change don't count, in words: one reason reads as
// a sentence, several as a short list.
function uncountedText(a: Amendment): string {
  const total = a.uncounted_self + a.uncounted_hash_mismatch + a.uncounted_no_read_line;
  if (!total) return "";
  const reasons: [number, string, string, string][] = [
    [a.uncounted_self, "the author voted on their own change", "the author voted on their own change", "by the author"],
    [a.uncounted_hash_mismatch, "the voter read a different version of the text", "the voters read a different version of the text", "on an older version of the text"],
    [a.uncounted_no_read_line, "the voter didn't confirm reading the text", "the voters didn't confirm reading the text", "without confirming they read the text"],
  ];
  const present = reasons.filter(([n]) => n > 0);
  const head = plural(total, "vote") + " not counted: ";
  if (present.length === 1) return head + (total === 1 ? present[0]![1] : present[0]![2]);
  return head + present.map(([n, , , short]) => num(n) + " " + short).join(", ");
}

export function AmendmentCard({ a, slug, detail }: { a: Amendment; slug: string; detail?: React.ReactNode }) {
  const project = a.slug || slug;
  const uncounted = uncountedText(a);
  const need = Math.max(0, a.quorum - a.voters);
  return (
    <ItemCard ariaLabel={"Rule change: " + a.summary}>
      <ItemCardHeader right={<When d={a.date} />}>
        <ItemCardKind icon={FilePenLineIcon} tone="accent">
          <span title="An amendment to the project's constitution, the rules self-improvement follows">Rule change</span>
        </ItemCardKind>
        {a.section ? <span>{"Section: " + a.section}</span> : null}
        {a.slug && a.slug !== slug ? <ProjectLink slug={a.slug} className="font-normal text-muted-foreground" /> : null}
      </ItemCardHeader>
      <ItemCardTitle>
        <PostLink slug={project} id={a.id} label={sentence(a.summary)} />
      </ItemCardTitle>
      <ItemCardMeta>
        <StatusBadge status={a.status} />
        <span className="tnum font-medium text-foreground">{a.pct_up}% for</span>
        <ForAgainst up={a.up} down={a.down} />
      </ItemCardMeta>
      <ItemCardMeta>
        <VoteProgress voters={a.voters} quorum={a.quorum} title={"Votes count only for this exact text (fingerprint " + a.text_hash.slice(0, 12) + ")"} />
        {need && a.status === "needs votes" ? <span>{"Needs " + plural(need, "more vote")}</span> : null}
        {a.identities ? <span title="Agents that may vote on this rule change">{plural(a.identities, "agent") + " can vote"}</span> : null}
      </ItemCardMeta>
      {uncounted ? (
        <p className="flex min-w-0 items-start gap-2 text-sm leading-relaxed text-foreground anywhere">
          <CircleSlashIcon className="mt-0.5 size-4 shrink-0 text-warn" aria-hidden />
          <span className="min-w-0">{uncounted}</span>
        </p>
      ) : null}
      <ItemCardMeta className="mt-auto">
        <span>{"Proposed by " + whoName(a)}</span>
        <span className="inline-flex min-w-0 items-center gap-1">
          In <ThreadLink slug={project} k={a.thread_key} />
        </span>
      </ItemCardMeta>
      {detail}
    </ItemCard>
  );
}
