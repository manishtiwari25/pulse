"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, ChevronRightIcon, ExternalLinkIcon, ServerIcon, UsersIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Progress } from "@/components/ui/progress";
import { ConfirmDialog } from "./confirm";
import { Sparkline } from "./charts";
import { EmptyState, Kv, Panel } from "./panel";
import { Fact, ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "./item-card";
import { KindBadge, Mono, PostLink, ProjectLink, Provenance, StatusBadge, ThreadLink, ToneBadge, VoteBadge, When, Who } from "./who";
import { cn } from "@/lib/utils";
import { expiresAt, hashHue, initials, num, plural, statusTone, truncate, untilText } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { COMMONS_HINT, evolveGate, taskProgress, writes, type Amendment, type Claim, type EvolveGate, type Executor, type LandingHold, type PostRef, type ProjectCardData, type Proposal, type Runner, type Suggestion, type Task, type TaskWorker } from "@/lib/api";
import { useNow } from "@/lib/use-api";
import { EvolveBadges, gateStep } from "./evolve";

// ----- project card ------------------------------------------------------

type FactTone = "ok" | "warn" | "accent";
const FACT_TONE: Record<FactTone, string> = { ok: "text-ok", warn: "text-warn", accent: "text-accent-2" };
const FOOTER_TONE = {
  neutral: "border-border bg-muted/40 text-muted-foreground",
  warn: "border-warn/30 bg-warn/10 text-warn",
  bad: "border-bad/30 bg-bad/10 text-bad",
} as const;

function ProjectAvatar({ slug }: { slug: string }) {
  return (
    <span className="flex size-9 shrink-0 items-center justify-center rounded-lg font-semibold text-white shadow-card" style={{ background: `oklch(0.62 0.12 ${hashHue(slug)})` }} aria-hidden>
      {initials(slug)}
    </span>
  );
}

function AttentionLine({ tone = "neutral", children }: { tone?: keyof typeof FOOTER_TONE; children: React.ReactNode }) {
  return <p className={cn("rounded-md border px-2.5 py-1.5 text-xs leading-snug anywhere", FOOTER_TONE[tone])}>{children}</p>;
}

// `gate` overrides what the org row says about the self-improvement gates
// (the hub page merges in the registry row); undefined reads the row itself.
export function ProjectCard({ p, spark, gate }: { p: ProjectCardData; spark?: number[]; gate?: EvolveGate | null }) {
  const inVote = p.suggestions_needing_votes + p.amendments_open;
  const g = gate === undefined ? evolveGate(p) : gate;
  // A self-improvement step only the owner can take counts as "for you" like an awaiting proposal (ADR-029).
  const step = gateStep(p.slug, g);
  const forYou = p.proposals_awaiting + (step ? 1 : 0);
  const attention = forYou > 0;
  const facts: { n: number; label: string; tone?: FactTone }[] = [
    { n: forYou, label: "for you", tone: forYou ? "ok" : undefined },
    { n: p.claims_active, label: "working", tone: p.claims_active ? "ok" : undefined },
    { n: p.threads_open, label: "threads" },
    { n: p.suggestions_eligible, label: "eligible", tone: p.suggestions_eligible ? "accent" : undefined },
    { n: inVote, label: "in vote", tone: inVote ? "warn" : undefined },
    { n: p.posts_24h, label: "posts/24h" },
  ];
  const footer = p.error
    ? { tone: "bad" as const, text: p.error }
    : p.stop_present
      ? { tone: "bad" as const, text: "STOP is set - the evolution loop halts until it is removed." }
      : p.missing || !p.present
        ? { tone: "warn" as const, text: "Registered but missing on disk." }
        : !p.board_present
          ? { tone: "warn" as const, text: "No board yet - pulse init registers this checkout." }
          : null;
  return (
    <ItemCard href={hrefs.project(p.slug)} highlight={attention} ariaLabel={"Project " + p.slug}>
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 items-start gap-2.5">
          <ProjectAvatar slug={p.slug} />
          <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="truncate text-[15px] font-semibold tracking-tight">{p.slug}</span>
            {p.brain ? <Badge variant="secondary">framework</Badge> : null}
            {!p.present ? <Badge variant="destructive">missing on disk</Badge> : null}
            {p.stop_present ? <ToneBadge tone="bad">STOP</ToneBadge> : null}
          </div>
          <div className="truncate text-xs text-muted-foreground">
            {p.branch ? <Mono>{p.branch}</Mono> : null}
            {p.head ? <Mono className="ml-1.5 text-muted-foreground">{p.head}</Mono> : null}
            {p.dirty ? <span className="ml-1.5 text-warn">dirty</span> : null}
          </div>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2 text-muted-foreground">
          {spark?.length ? <Sparkline values={spark} w={72} h={20} /> : null}
          <ChevronRightIcon className="size-4 opacity-40 transition-transform group-hover/item:translate-x-0.5" />
        </div>
      </div>
      {/* Tiles from sm up; one quiet line on phones so four cards fit a screen. */}
      <div className="hidden grid-cols-6 gap-1.5 sm:grid">
        {facts.map((f) => (
          <Fact key={f.label} n={num(f.n)} label={f.label} tone={f.tone} />
        ))}
      </div>
      <ItemCardMeta className="sm:hidden">
        {facts.map((f) => (
          <span key={f.label}>
            <b className={cn("tnum font-semibold text-foreground", f.tone && FACT_TONE[f.tone])}>{num(f.n)}</b> {f.label}
          </span>
        ))}
      </ItemCardMeta>
      {/* The self-improvement gates (ADR-029): why the loop runs or waits, in a few chips - the baseline one reports the daemon's progress (ADR-031). Hidden when the hub does not report them. */}
      {g ? <EvolveBadges g={g} /> : null}
      {p.last_post ? (
        <p className="truncate text-xs text-muted-foreground">
          <KindBadge kind={p.last_post.kind} /> <span className="ml-1">{p.last_post.summary}</span>
        </p>
      ) : footer ? (
        <AttentionLine tone={footer.tone}>{footer.text}</AttentionLine>
      ) : (
        <AttentionLine>No posts yet.</AttentionLine>
      )}
      {p.last_post && footer ? <AttentionLine tone={footer.tone}>{footer.text}</AttentionLine> : null}
    </ItemCard>
  );
}

// ----- commons card -------------------------------------------------------

// The shared board, kept out of the project grid: it is not a checkout, so it
// gets its own card that says what it is and what waits on it.
export function CommonsCard({ c, spark }: { c: ProjectCardData; spark?: number[] }) {
  const facts: { n: number; label: string; tone?: FactTone }[] = [
    { n: c.questions_open, label: "open questions", tone: c.questions_open ? "warn" : undefined },
    { n: c.threads_open, label: "threads" },
    { n: c.posts_24h, label: "posts/24h" },
    { n: c.agents, label: "agents" },
  ];
  return (
    <ItemCard href={hrefs.project(c.slug)} ariaLabel="Commons - the board every project shares">
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 items-start gap-2.5">
          <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary shadow-card" aria-hidden>
            <UsersIcon className="size-4" />
          </span>
          <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-[15px] font-semibold tracking-tight">Commons</span>
            <Badge variant="outline">shared board</Badge>
            {!c.present ? <Badge variant="secondary">not created yet</Badge> : null}
          </div>
          <p className="text-xs text-muted-foreground">{COMMONS_HINT}</p>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2 text-muted-foreground">
          {spark?.length ? <Sparkline values={spark} w={72} h={20} /> : null}
          <ChevronRightIcon className="size-4 opacity-40 transition-transform group-hover/item:translate-x-0.5" />
        </div>
      </div>
      {c.present && c.error ? <p className="text-xs text-bad anywhere">{c.error}</p> : null}
      <div className="hidden grid-cols-4 gap-1.5 sm:grid">
        {facts.map((f) => (
          <Fact key={f.label} n={num(f.n)} label={f.label} tone={f.tone} />
        ))}
      </div>
      <ItemCardMeta className="sm:hidden">
        {facts.map((f) => (
          <span key={f.label}>
            <b className={cn("tnum font-semibold text-foreground", f.tone && FACT_TONE[f.tone])}>{num(f.n)}</b> {f.label}
          </span>
        ))}
      </ItemCardMeta>
      {c.last_post ? (
        // line-clamp, not truncate: this card sits outside a CardGrid, and a nowrap line would widen the column to the summary's length.
        <ItemCardBody clamp={1} className="text-xs">
          <KindBadge kind={c.last_post.kind} /> <span className="ml-1">{c.last_post.summary}</span>
        </ItemCardBody>
      ) : (
        <p className="text-xs text-muted-foreground">{c.present ? "No posts yet." : "The first post that concerns more than one project creates it" + (c.path ? " at " + c.path : "") + "."}</p>
      )}
    </ItemCard>
  );
}

// ----- proposal card + decide ---------------------------------------------

export function ProposalCard({ r, slug, canDecide = true }: { r: Proposal; slug: string; canDecide?: boolean }) {
  const [open, setOpen] = useState<"yes" | "no" | null>(null);
  const awaiting = r.status === "awaiting";
  const [expanded, setExpanded] = useState(false);
  const long = (r.proposal_text || "").length > 420;
  const s = r.slug || slug;
  return (
    <ItemCard highlight={awaiting} ariaLabel={"Proposal: " + r.summary}>
      <ItemCardHeader
        right={
          <>
            <Who row={r} compact />
            <When d={r.date} />
          </>
        }
      >
        <StatusBadge status={r.status} />
        {r.slug && r.slug !== slug ? <ProjectLink slug={r.slug} /> : null}
        <ThreadLink slug={s} k={r.thread_key} />
      </ItemCardHeader>
      <ItemCardTitle>{r.summary}</ItemCardTitle>
      {r.proposal_text ? (
        <ItemCardBody clamp={0}>
          <p className={cn(!expanded && long && "line-clamp-5")}>{r.proposal_text}</p>
          {long ? (
            <button type="button" className="mt-1 text-xs underline underline-offset-2 hover:text-foreground" onClick={() => setExpanded((e) => !e)}>
              {expanded ? "less" : "read the whole proposal"}
            </button>
          ) : null}
        </ItemCardBody>
      ) : null}
      {!awaiting && r.decided_id ? (
        <ItemCardMeta>
          <span>
            decided <When d={r.decided_date} /> {r.decided_agent ? "by " + r.decided_agent : ""} {"·"} <PostLink slug={s} id={r.decided_id} label={r.decided_summary || "the decision post"} />
          </span>
        </ItemCardMeta>
      ) : null}
      <ItemCardActions>
        <Button asChild variant="ghost" size="xs" className="text-muted-foreground">
          <Link href={hrefs.post(s, r.id)}>
            open the post <ExternalLinkIcon />
          </Link>
        </Button>
        {awaiting && canDecide && r.can_decide !== false ? (
          <span className="ml-auto flex shrink-0 gap-2">
            <Button size="sm" variant="outline" className="text-bad" onClick={() => setOpen("no")} aria-label={"No to: " + r.summary}>
              <XIcon /> No
            </Button>
            <Button size="sm" onClick={() => setOpen("yes")} aria-label={"Yes to: " + r.summary}>
              <CheckIcon /> Yes
            </Button>
          </span>
        ) : awaiting ? (
          <span className="ml-auto text-xs text-muted-foreground">read-only: open the hub on the machine that runs it to decide</span>
        ) : null}
      </ItemCardActions>
      <ConfirmDialog
        open={open !== null}
        onOpenChange={(o) => !o && setOpen(null)}
        title={open === "yes" ? "Say yes to this proposal?" : "Say no to this proposal?"}
        description={r.summary}
        action={open === "yes" ? "Yes, proceed" : "No, decline"}
        destructive={open === "no"}
        words={{ label: "Your words", placeholder: open === "yes" ? "e.g. approved - keep the scope as written" : "e.g. not now - revisit after the Go port lands", required: true }}
        success={open === "yes" ? "Decided yes - the board records your answer; a job is queued when the proposal asks for work." : "Decided no - the board records your answer."}
        onConfirm={(words) => writes.decide(r.id, open === "yes" ? "yes" : "no", words)}
      />
    </ItemCard>
  );
}

// ----- suggestion / amendment ---------------------------------------------

export function SuggestionCard({ s, quorum, slug, detail }: { s: Suggestion; quorum: number; slug: string; detail?: React.ReactNode }) {
  const now = useNow();
  const need = Math.max(0, quorum - s.voters);
  const exp = expiresAt(s);
  const planned = s.status === "eligible" ? (s.job ? s.gate || "job " + s.job + (s.job_status ? " is " + s.job_status : "") : s.gate ? "not planned: " + s.gate : s.plannable ? "plannable — the daemon queues it at its next tick" : "") : "";
  const plannedTone = s.job ? "text-muted-foreground" : s.gate ? "text-warn" : "text-ok";
  return (
    <article className="rounded-lg border bg-card p-3.5 shadow-xs">
      <div className="flex flex-wrap items-center gap-1.5 text-xs">
        <StatusBadge status={s.status} />
        {s.rank ? <ToneBadge tone="neutral">#{s.rank}</ToneBadge> : null}
        {s.slug && s.slug !== slug ? <ProjectLink slug={s.slug} /> : null}
        <ThreadLink slug={s.slug || slug} k={s.thread_key} />
        <span className="ml-auto flex items-center gap-2 text-muted-foreground">
          <Who row={s} compact />
          <When d={s.date} />
        </span>
      </div>
      <p className="mt-2 text-[13.5px] leading-snug anywhere">
        <PostLink slug={s.slug || slug} id={s.id} label={s.summary} className="font-medium" />
      </p>
      <div className="mt-2.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
        <span>
          score <b className="tnum text-foreground">{s.score > 0 ? "+" + s.score : s.score}</b>
        </span>
        <span>
          <span className="text-ok">{"▲"} {s.up}</span> <span className="text-bad">{"▼"} {s.down}</span>
        </span>
        <span className="inline-flex items-center gap-2">
          voters <b className="tnum text-foreground">{s.voters}</b>/{quorum}
          <Progress value={Math.min(100, (100 * s.voters) / Math.max(1, quorum))} className="h-1.5 w-16" />
          {need ? <span>{need + " more to be eligible"}</span> : <span className="text-ok">quorum met</span>}
        </span>
        {exp && s.status !== "eligible" ? <span>{"expires " + untilText(exp, now)}</span> : null}
      </div>
      {planned ? <p className={cn("mt-1.5 text-xs leading-snug", plannedTone)}>{s.job ? "▸ " : ""}{planned}</p> : null}
      {detail}
    </article>
  );
}

export function AmendmentCard({ a, slug, detail }: { a: Amendment; slug: string; detail?: React.ReactNode }) {
  const uncounted = a.uncounted_self + a.uncounted_hash_mismatch + a.uncounted_no_read_line;
  return (
    <article className="rounded-lg border bg-card p-3.5 shadow-xs">
      <div className="flex flex-wrap items-center gap-1.5 text-xs">
        <StatusBadge status={a.status} />
        {a.section ? <ToneBadge tone="warn">{"section: " + a.section}</ToneBadge> : null}
        {a.slug && a.slug !== slug ? <ProjectLink slug={a.slug} /> : null}
        <ThreadLink slug={a.slug || slug} k={a.thread_key} />
        <span className="ml-auto flex items-center gap-2 text-muted-foreground">
          <Who row={a} compact />
          <When d={a.date} />
        </span>
      </div>
      <p className="mt-2 text-[13.5px] leading-snug anywhere">
        <PostLink slug={a.slug || slug} id={a.id} label={a.summary} className="font-medium" />
      </p>
      <div className="mt-2.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
        <span>
          <span className="text-ok">{"▲"} {a.up}</span> <span className="text-bad">{"▼"} {a.down}</span> {"·"} <b className="tnum text-foreground">{a.pct_up}%</b> up
        </span>
        <span className="inline-flex items-center gap-2">
          sessions <b className="tnum text-foreground">{a.voters}</b>/{a.quorum}
          <Progress value={Math.min(100, (100 * a.voters) / Math.max(1, a.quorum))} className="h-1.5 w-16" />
        </span>
        <span>{plural(a.identities, "identity") + " eligible"}</span>
        {uncounted ? <span className="text-warn">{uncounted + " not counted (" + [a.uncounted_self && a.uncounted_self + " self", a.uncounted_hash_mismatch && a.uncounted_hash_mismatch + " hash", a.uncounted_no_read_line && a.uncounted_no_read_line + " no Read:"].filter(Boolean).join(", ") + ")"}</span> : null}
        <Mono className="text-muted-foreground">{"hash " + a.text_hash.slice(0, 12)}</Mono>
      </div>
      {detail}
    </article>
  );
}

// ----- feeds --------------------------------------------------------------

export function FeedList({ rows, slug, limit }: { rows: PostRef[]; slug?: string; limit?: number }) {
  const list = limit ? rows.slice(0, limit) : rows;
  if (!list.length) return <EmptyState>No posts yet. Agents post claims, findings, suggestions and votes with the CLI.</EmptyState>;
  return (
    <ul className="divide-y">
      {list.map((p) => {
        const s = p.slug || slug || "";
        return (
          <li key={s + p.id} className="flex items-start gap-2.5 py-2 first:pt-0 last:pb-0">
            <Who row={p} compact className="w-32 shrink-0 max-md:hidden" />
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-1.5 text-xs">
                <KindBadge kind={p.kind} />
                {p.vote ? <VoteBadge vote={p.vote} /> : null}
                {p.resolution ? <ToneBadge tone={statusTone(p.resolution)}>{p.resolution}</ToneBadge> : null}
                {p.slug && !slug ? <ProjectLink slug={p.slug} className="text-xs" /> : null}
                <ThreadLink slug={s} k={p.thread_key} className="text-muted-foreground" />
                <Provenance ok={p.provenance_ok} missing={p.provenance_missing} />
              </div>
              <p className="mt-0.5 text-[13px] leading-snug anywhere">
                <PostLink slug={s} id={p.id} label={p.summary} />
              </p>
            </div>
            <When d={p.date} className="shrink-0 text-xs text-muted-foreground" />
          </li>
        );
      })}
    </ul>
  );
}

// ----- working now ---------------------------------------------------------

export interface WorkingRow {
  key: string;
  agent: string;
  slug: string;
  task: { t: Task; w: TaskWorker } | null;
  /** The agent's live claims in this project, newest first. */
  claims: Claim[];
  /** When the newest unit of work started (ms), for ordering. */
  at: number;
}

const ms = (s?: string) => (s ? Date.parse(s) || 0 : 0);

// One row per agent and project. A claim is a unit of work, not a person, and a busy fleet
// holds a dozen per agent - so the panel answers "who is on what" and keeps the claims behind a
// toggle instead of listing every one.
export function workingRows(claims: Claim[], tasks: Task[] = [], slug?: string): WorkingRow[] {
  const rows = new Map<string, WorkingRow>();
  const rowFor = (agent: string, project: string) => {
    const key = agent + "@" + project;
    let r = rows.get(key);
    if (!r) {
      r = { key, agent, slug: project, task: null, claims: [], at: 0 };
      rows.set(key, r);
    }
    return r;
  };
  for (const t of tasks) {
    for (const w of taskProgress(t).working) {
      const r = rowFor(w.agent, t.project);
      if (!r.task || ms(w.since) > ms(r.task.w.since)) r.task = { t, w };
      r.at = Math.max(r.at, ms(w.since));
    }
  }
  for (const c of claims) {
    const r = rowFor(c.agent || c.name || c.session || "(unknown)", c.slug || slug || "");
    r.claims.push(c);
    r.at = Math.max(r.at, ms(c.expires));
  }
  for (const r of rows.values()) r.claims.sort((a, b) => ms(b.expires) - ms(a.expires) || (a.id < b.id ? 1 : -1));
  return [...rows.values()].sort((a, b) => Number(!!b.task) - Number(!!a.task) || b.at - a.at || a.agent.localeCompare(b.agent));
}

export function workingSummary(rows: WorkingRow[], claims: number): string {
  return claims ? plural(rows.length, "agent") + " · " + plural(claims, "claim") : plural(rows.length, "agent");
}

export function WorkingNow({ claims, slug, tasks, limit = 6 }: { claims: Claim[]; slug?: string; tasks?: Task[]; limit?: number }) {
  const [all, setAll] = useState(false);
  const rows = workingRows(claims, tasks ?? [], slug);
  if (!rows.length) return <p className="rounded-md border border-dashed px-3 py-2 text-xs text-muted-foreground anywhere">Nobody is on a task or holds a claim right now - work appears here when a job is leased or an agent claims a thread.</p>;
  const shown = all || rows.length <= limit ? rows : rows.slice(0, limit);
  return (
    <div>
      <ul className="divide-y">
        {shown.map((r) => (
          <WorkingRowItem key={r.key} r={r} slug={slug} />
        ))}
      </ul>
      {rows.length > limit ? (
        <button type="button" aria-expanded={all} className="mt-2 text-xs underline underline-offset-2 hover:text-foreground" onClick={() => setAll((v) => !v)}>
          {all ? "show fewer" : "show all " + plural(rows.length, "agent")}
        </button>
      ) : null}
    </div>
  );
}

function WorkingRowItem({ r, slug }: { r: WorkingRow; slug?: string }) {
  const [open, setOpen] = useState(false);
  const s = r.slug;
  const latest = r.claims[0];
  // With a task the whole claim list folds; without one the newest claim is the headline and the rest fold.
  const folded = r.task ? r.claims.length : r.claims.length - 1;
  const threads = new Set(r.claims.map((c) => c.thread_key)).size;
  const when = r.task ? (
    r.task.w.since ? <span className="ml-auto shrink-0 text-xs text-muted-foreground">since <When d={r.task.w.since} /></span> : null
  ) : latest?.expires ? (
    <When d={latest.expires} className="ml-auto shrink-0 text-xs text-muted-foreground" />
  ) : (
    <span className="ml-auto shrink-0 text-xs text-muted-foreground tnum">no expiry</span>
  );
  // The panel lives in a narrow column: the agent leads the header line instead of taking a side column, so
  // the summary - the answer to "on what?" - gets the full width.
  return (
    <li className="py-2 first:pt-0 last:pb-0 text-[13px]">
      <div className="flex flex-wrap items-center gap-x-1.5 gap-y-0.5 text-xs">
        <Who row={{ agent: r.agent }} compact className="max-w-[45%] font-medium" />
        {r.task ? (
          <>
            <span className="text-muted-foreground">on</span>
            <Link href={hrefs.task(r.task.t.project, r.task.t.id)} className="hover:underline">
              <Mono>{r.task.t.id}</Mono>
            </Link>
            <ToneBadge tone="info">{r.task.w.kind}</ToneBadge>
          </>
        ) : latest ? (
          <>
            {s && !slug ? <ProjectLink slug={s} /> : null}
            <ThreadLink slug={s} k={latest.thread_key} className="text-muted-foreground" />
          </>
        ) : null}
        {when}
      </div>
      {r.task ? (
        <p className="mt-0.5 line-clamp-2 leading-snug anywhere">
          <Link href={hrefs.task(r.task.t.project, r.task.t.id)} className="hover:underline">{truncate(r.task.t.title, 90)}</Link>
          <span className="text-muted-foreground"> - {r.task.w.title ? truncate(r.task.w.title, 96) : r.task.w.job}</span>
        </p>
      ) : latest ? (
        <p className="mt-0.5 line-clamp-2 leading-snug anywhere">
          <PostLink slug={s} id={latest.id} label={latest.summary} />
        </p>
      ) : null}
      {folded > 0 ? (
        <button type="button" aria-expanded={open} className="mt-0.5 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline tnum" onClick={() => setOpen((v) => !v)}>
          {open ? "hide claims" : (r.task ? "also holds " : "+ ") + plural(folded, r.task ? "claim" : "more claim") + (threads > 1 ? " across " + plural(threads, "thread") : "")}
        </button>
      ) : null}
      {open ? (
        <ul className="mt-1.5 ml-1 grid gap-1 border-l pl-2.5 text-[12.5px]">
          {(r.task ? r.claims : r.claims.slice(1)).map((c) => (
            <li key={c.id} className="flex items-start gap-2">
              <div className="min-w-0 flex-1 anywhere">
                <ThreadLink slug={s} k={c.thread_key} className="text-muted-foreground" /> <PostLink slug={s} id={c.id} label={truncate(c.summary, 120)} />
              </div>
              {c.expires ? <When d={c.expires} className="shrink-0 text-xs text-muted-foreground" /> : null}
            </li>
          ))}
        </ul>
      ) : null}
    </li>
  );
}

// ----- the machine ---------------------------------------------------------

export function MachinePanel({ r }: { r: Runner | undefined | null }) {
  if (!r) return <Panel title="The machine" icon={ServerIcon}>{<EmptyState>The runner state is not available.</EmptyState>}</Panel>;
  if (!r.available) return <Panel title="The machine" icon={ServerIcon}>{<EmptyState hint="pulse daemon">No store yet, or no daemon has run. Start one with the CLI.</EmptyState>}</Panel>;
  const d = r.daemon;
  const stopped = r.runner && !r.runner.allowed;
  return (
    <Panel
      title="The machine"
      icon={ServerIcon}
      actions={
        <span className="flex items-center gap-1.5">
          <ToneBadge tone={stopped ? "bad" : d.state === "running" || d.state === "alive" ? "ok" : statusTone(d.state)} dot>
            {stopped ? "STOP" : d.state}
          </ToneBadge>
          <Link href={hrefs.settings("runner")} className="hover:underline">
            caps
          </Link>
        </span>
      }
    >
      <Kv
        rows={[
          ["daemon", (d.text || d.state) + (d.pid ? "" : "")],
          ["last loop", <When key="at" d={d.at} />],
          ["slots", r.hands.free + " free of " + r.hands.slots + " on " + plural(r.hands.executors_active, "active executor")],
          ["workers", r.workers.busy + " busy · " + r.workers.idle + " idle" + (r.workers.idle_names.length ? " (" + r.workers.idle_names.join(", ") + ")" : "") + " · target " + r.workers.target],
          ["jobs", r.jobs.queued + " queued · " + r.jobs.leased + " leased · " + r.jobs.running + " running" + (r.jobs.blocked ? " · " + r.jobs.blocked + " blocked" : "")],
          ...(r.capacity ? [["capacity", r.capacity] as [React.ReactNode, React.ReactNode]] : []),
          ["runs today", r.jobs.runs_today + (r.jobs.cap ? " of " + r.jobs.cap : " (no daily cap)")],
        ]}
      />
      {stopped ? <p className="mt-2 text-xs text-bad">{"The runner is stopped: " + (r.runner?.reason || "the stop switch is on") + "."}</p> : null}
      <WorkerHold hold={r.hold} />
      <LandingHolds holds={r.landing} />
      {r.executors.length ? (
        <ul className="mt-3 grid gap-1.5">
          {r.executors.map((e) => (
            <ExecutorRow key={e.name} e={e} />
          ))}
        </ul>
      ) : null}
    </Panel>
  );
}

function WorkerHold({ hold }: { hold: string | undefined }) {
  if (!hold) return null;
  return (
    <p className="mt-2 text-xs text-warn" aria-label="workers held">
      <span className="font-medium">workers held: </span>
      {hold}{" "}
      <Link href={hrefs.settings("harness")} className="font-medium underline-offset-2 hover:underline">
        Sign in
      </Link>
    </p>
  );
}

// Verified branches that cannot land and why (ADR-024): the daemon writes one
// note per held project - a dirty or switched checkout, a lock, an unreadable
// folder - and clears it when the branches land again. Nothing pushes; landing
// moves the local default branch, so the fix is the owner's: commit or stash,
// switch back, and the next tick lands the queue.
export function LandingHolds({ holds }: { holds: LandingHold[] | undefined }) {
  if (!holds?.length) return null;
  return (
    <ul className="mt-2 grid gap-1 text-xs text-warn" aria-label="landing held">
      {holds.map((h) => (
        <li key={h.project}>
          <span className="font-medium">{"landing held for " + h.project + ": "}</span>
          {h.note}
          {h.stale ? <span className="text-muted-foreground">{" (stale)"}</span> : null}
        </li>
      ))}
    </ul>
  );
}

export function ExecutorRow({ e, action }: { e: Executor; action?: React.ReactNode }) {
  return (
    <li className="flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-xs">
      <span className="size-1.5 shrink-0 rounded-full bg-muted-foreground" aria-hidden />
      <ServerIcon className="size-3.5 text-muted-foreground" />
      <span className="font-medium">{e.name}</span>
      <ToneBadge tone="neutral">{e.kind}</ToneBadge>
      <StatusBadge status={e.status} />
      <span className="ml-auto tnum text-muted-foreground">{(e.running ?? 0) + " running · " + (e.free ?? 0) + " free of " + e.workers}</span>
      {action ? <span className="-my-1 -mr-1 shrink-0">{action}</span> : null}
    </li>
  );
}
