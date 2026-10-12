"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, ChevronRightIcon, CirclePauseIcon, MessageCircleQuestionIcon, MessageSquareTextIcon, UsersIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "./confirm";
import { DecideButtons } from "./decide-buttons";
import { Sparkline } from "./charts";
import { EmptyState } from "./panel";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle } from "./item-card";
import { Avatar, KindBadge, PostLink, ProjectLink, Provenance, ThreadLink, ToneBadge, VoteBadge, When, Who } from "./who";
import { cn } from "@/lib/utils";
import { firstSentence, hashHue, initials, plural, rel, sentence, statusTone, truncate } from "@/lib/format";
import { statusWord } from "@/lib/words";
import { hrefs } from "@/lib/nav";
import { COMMONS_HINT, evolveGate, taskProgress, writes, type Claim, type EvolveGate, type OwnerQuestion, type PostRef, type ProjectCardData, type Proposal, type Task, type TaskWorker } from "@/lib/api";
import { gateStep, type GateStep } from "./evolve";

// ----- project card ------------------------------------------------------

function ProjectAvatar({ slug }: { slug: string }) {
  return (
    <span
      className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-[oklch(0.93_0.045_var(--hue))] text-sm font-semibold text-[oklch(0.36_0.09_var(--hue))] dark:bg-[oklch(0.34_0.06_var(--hue))] dark:text-[oklch(0.92_0.05_var(--hue))]"
      style={{ ["--hue" as string]: hashHue(slug) }}
      aria-hidden
    >
      {initials(slug)}
    </span>
  );
}

// `gate` overrides what the org row says about the self-improvement gates
// (the hub page merges in the registry row); undefined reads the row itself.
// `forYou` is the project's share of the one inbox count (useInbox); without
// it the card falls back to what the org row alone can tell.
export function ProjectCard({ p, spark, gate, forYou: forYouCount }: { p: ProjectCardData; spark?: number[]; gate?: EvolveGate | null; forYou?: number }) {
  const g = gate === undefined ? evolveGate(p) : gate;
  const step = gateStep(p.slug, g);
  const forYou = forYouCount ?? p.proposals_awaiting + (p.owner_questions ?? 0) + (step ? 1 : 0);
  const last = p.last_post?.date || p.built_at;
  const status = projectStatus(p, step, forYou);
  const facts = [plural(p.agents, "agent"), plural(p.threads_open, "open thread"), last ? "Active " + rel(last) : "No activity yet"];
  return (
    <ItemCard href={hrefs.project(p.slug)} highlight={forYou > 0} ariaLabel={"Project " + p.slug + ": " + status.line} className="@container/project gap-3.5">
      <div className="flex min-w-0 items-start justify-between gap-3">
        <div className="flex min-w-0 flex-1 items-center gap-3">
          <ProjectAvatar slug={p.slug} />
          <div className="min-w-0 flex-1">
            <h3 title={p.slug} className="truncate text-[17px] font-semibold leading-tight tracking-tight">{p.slug}</h3>
            <p className="mt-0.5 truncate text-[13px] text-muted-foreground">{p.brain ? "PULSE's own source" : p.name && p.name !== p.slug ? p.name : "Software project"}</p>
          </div>
        </div>
        <ChevronRightIcon className="mt-2.5 size-4 shrink-0 text-muted-foreground opacity-60 transition-[transform,opacity] group-hover/item:opacity-100 motion-safe:group-hover/item:translate-x-0.5 motion-reduce:transition-none" aria-hidden />
      </div>
      <div className="flex min-w-0 items-center justify-between gap-3">
        <p className={cn("flex min-w-0 items-center gap-2 text-sm font-medium leading-snug", status.text)}>
          <span className={cn("size-2 shrink-0 rounded-full", status.dot)} aria-hidden />
          <span className="min-w-0 anywhere">{status.line}</span>
        </p>
        {spark?.length ? <Sparkline values={spark} w={80} h={22} className="shrink-0" /> : null}
      </div>
      <ItemCardMeta>
        {facts.map((f) => <span key={f} title={[p.branch, p.head, p.dirty ? "uncommitted changes" : "", g ? "self-improvement level " + g.evolve_level : ""].filter(Boolean).join(", ") || undefined}>{f}</span>)}
      </ItemCardMeta>
    </ItemCard>
  );
}

function projectStatus(p: ProjectCardData, step: GateStep | null, forYou: number): { line: string; dot: string; text: string } {
  if (p.error || p.missing || !p.present) return { line: "Missing on disk", dot: "bg-bad", text: "text-bad" };
  if (p.stop_present) return { line: "Self-improvement is stopped", dot: "bg-warn", text: "text-foreground" };
  if (forYou > 0) return { line: forYou === 1 ? "1 thing waits for you" : plural(forYou, "thing") + " wait for you", dot: "bg-you", text: "text-foreground" };
  if (p.claims_active > 0) return { line: "Agents are working", dot: "bg-primary", text: "text-foreground" };
  if (step) return { line: "Next step: " + sentence(step.label), dot: "bg-warn", text: "text-foreground" };
  return { line: "All quiet", dot: "bg-ok", text: "text-muted-foreground" };
}

// ----- shared board ------------------------------------------------------

// Whether the shared board has anything the owner should see right now: an
// open question or a post today. A quiet board stays off the home page; the
// project menu, the sidebar and search still reach it.
export function commonsLive(c: ProjectCardData): boolean {
  return c.present && (c.questions_open + (c.owner_questions ?? 0) > 0 || c.posts_24h > 0);
}

// One row, not a card: the shared board is not a checkout, so all it has to
// say is what waits on it and where to open it.
export function CommonsLine({ c, className }: { c: ProjectCardData; className?: string }) {
  const questions = c.questions_open + (c.owner_questions ?? 0);
  const facts = c.present
    ? [questions ? plural(questions, "open question") : "", c.posts_24h ? plural(c.posts_24h, "post") + " today" : "", c.last_post?.date ? "last post " + rel(c.last_post.date) : "no posts yet"].filter(Boolean)
    : ["not created yet"];
  return (
    <Link
      href={hrefs.project(c.slug)}
      title={COMMONS_HINT}
      aria-label="Shared board, for work that touches more than one project"
      className={cn("group/commons flex min-w-0 items-center gap-3 rounded-xl border bg-card px-4 py-3 text-sm leading-relaxed transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring", className)}
    >
      <span className={cn("flex size-8 shrink-0 items-center justify-center rounded-lg", questions ? "bg-you-soft text-you-ink" : "bg-muted text-muted-foreground")} aria-hidden>
        <UsersIcon className="size-4" />
      </span>
      <span className="min-w-0 flex-1 anywhere">
        <span className="font-medium text-foreground">Shared board</span>
        <span className="text-muted-foreground">{" for work that touches more than one project. " + sentence(facts.join(", ")) + "."}</span>
      </span>
      <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground opacity-60 transition-[transform,opacity] group-hover/commons:opacity-100 motion-safe:group-hover/commons:translate-x-0.5 motion-reduce:transition-none" aria-hidden />
    </Link>
  );
}

// A quiet link to the post on the board, carrying the ids in its tooltip.
function OnTheBoard({ slug, id, thread }: { slug: string; id: string; thread: string }) {
  return (
    <Link href={hrefs.post(slug, id)} title={"Thread " + thread + ", post " + id} className="text-[13px] font-medium text-muted-foreground underline-offset-2 hover:text-foreground hover:underline">
      See it on the board
    </Link>
  );
}

function ReadMore({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  return (
    <button type="button" aria-expanded={open} className="mt-1 text-[13px] font-medium text-primary underline-offset-2 hover:underline" onClick={onToggle}>
      {open ? "Show less" : "Read more"}
    </button>
  );
}

// ----- proposal card + decide ---------------------------------------------

export function ProposalCard({ r, slug, canDecide = true }: { r: Proposal; slug: string; canDecide?: boolean }) {
  const [open, setOpen] = useState<"yes" | "no" | null>(null);
  const awaiting = r.status === "awaiting";
  const [expanded, setExpanded] = useState(false);
  const text = (r.proposal_text || "").trim();
  const lead = firstSentence(text);
  const more = text.length > lead.length + 1;
  const s = r.slug || slug;
  const who = r.agent || r.name || r.session || "an agent";
  const decidedBy = r.decided_agent === "human" || r.decided_agent === "owner" ? "you" : r.decided_agent;
  return (
    <ItemCard highlight={awaiting} ariaLabel={"Proposal: " + r.summary}>
      <ItemCardHeader right={<When d={r.date} />}>
        <ItemCardKind icon={MessageSquareTextIcon} tone={awaiting ? "you" : "neutral"}>{"Proposal from " + who}</ItemCardKind>
        {r.slug && r.slug !== slug ? <ProjectLink slug={r.slug} className="font-normal text-muted-foreground" /> : null}
      </ItemCardHeader>
      <ItemCardTitle>{sentence(r.summary)}</ItemCardTitle>
      {text ? (
        <ItemCardBody clamp={0}>
          {expanded ? <p className="whitespace-pre-line">{text}</p> : <p className="line-clamp-3">{lead}</p>}
          {more ? <ReadMore open={expanded} onToggle={() => setExpanded((e) => !e)} /> : null}
        </ItemCardBody>
      ) : null}
      <ItemCardActions>
        <OnTheBoard slug={s} id={r.id} thread={r.thread_key} />
        {awaiting && canDecide && r.can_decide !== false ? (
          <span className="flex w-full flex-wrap justify-start gap-2 sm:ml-auto sm:w-auto sm:justify-end">
            <Button variant="outline" onClick={() => setOpen("no")} aria-label={"No, decline: " + r.summary}><XIcon /> No, decline</Button>
            <Button onClick={() => setOpen("yes")} aria-label={"Yes, accept: " + r.summary}><CheckIcon /> Yes, accept</Button>
          </span>
        ) : awaiting ? (
          <span className="ml-auto text-[13px] text-muted-foreground">To answer, open PULSE on the machine that runs it.</span>
        ) : r.decided_id ? (
          <span className="ml-auto text-[13px] text-muted-foreground">{statusWord(r.status)} <When d={r.decided_date} />{decidedBy ? " by " + decidedBy : ""}</span>
        ) : null}
      </ItemCardActions>
      <ConfirmDialog
        open={open !== null}
        onOpenChange={(o) => !o && setOpen(null)}
        title={open === "yes" ? "Accept this proposal?" : "Decline this proposal?"}
        description={<><span className="block font-medium text-foreground">{sentence(r.summary)}</span><span className="mt-2 block">{open === "yes" ? "The agents go ahead as proposed. If it asks for work, a job is queued." : "The agents won't pursue it as written."}</span></>}
        action={open === "yes" ? "Yes, accept" : "No, decline"}
        destructive={open === "no"}
        words={{ label: "A note for the agents (optional)", placeholder: open === "yes" ? "Accepted: keep the scope as written." : "Not now, because…" }}
        success={open === "yes" ? "Accepted. The agents will go ahead." : "Declined. The agents won't pursue it."}
        onConfirm={(words) => writes.decide(r.id, open === "yes" ? "yes" : "no", words)}
      />
    </ItemCard>
  );
}

// ----- owner question --------------------------------------------------------

// A question that waits for the owner (ADR: for_owner posts and the evolution
// loop's stop questions). Who asked, where, the ask, and Yes / No that record
// the owner's decision on the board.
export function QuestionCard({ q, slug, canDecide = true }: { q: OwnerQuestion; slug: string; canDecide?: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const body = (q.body || "").trim();
  const s = q.slug || slug;
  const stop = isStopQuestion(q);
  const asker = q.agent || q.name || q.session || "an agent";
  const title = stop ? "Self-improvement on " + s.replace(/-/g, "\u2011") + " stopped. May it carry on?" : endSentence(sentence(q.summary));
  const lead = firstSentence(body);
  const hasMore = body.length > lead.length + 1;
  return (
    <ItemCard highlight={!q.stale} ariaLabel={"Question for you: " + q.summary}>
      <ItemCardHeader right={<When d={q.date} />}>
        {stop ? <ItemCardKind icon={CirclePauseIcon} tone="bad">Self-improvement paused</ItemCardKind> : <ItemCardKind icon={MessageCircleQuestionIcon} tone={q.stale ? "neutral" : "you"}>{"Question from " + asker}</ItemCardKind>}
        {q.slug && q.slug !== slug && !stop ? <ProjectLink slug={q.slug} className="font-normal text-muted-foreground" /> : null}
        {q.stale ? <ToneBadge tone="neutral">From an older thread</ToneBadge> : null}
      </ItemCardHeader>
      <ItemCardTitle>{title}</ItemCardTitle>
      {body ? (
        <ItemCardBody clamp={0}>
          {expanded ? <p className="whitespace-pre-line">{body}</p> : lead ? <p className="line-clamp-3">{lead}</p> : null}
          {hasMore ? <ReadMore open={expanded} onToggle={() => setExpanded((e) => !e)} /> : null}
        </ItemCardBody>
      ) : null}
      {q.on_post ? (
        <ItemCardMeta>
          <span>About <PostLink slug={s} id={q.on_post} label="an earlier post" className="text-foreground" /></span>
        </ItemCardMeta>
      ) : null}
      <ItemCardActions>
        <OnTheBoard slug={s} id={q.id} thread={q.thread_key} />
        <DecideButtons id={q.id} summary={title} stop={stop} canDecide={canDecide} yesLabel={stop ? "Yes, carry on" : "Yes"} noLabel={stop ? "No, keep it paused" : "No"} />
      </ItemCardActions>
    </ItemCard>
  );
}

// One closing mark: "hit?" stays "hit?", "do it" becomes "do it.", "now!!" becomes "now!".
function endSentence(t: string): string {
  const m = /([.!?])[.!?]*$/.exec(t);
  return m ? t.slice(0, m.index) + m[1] : t ? t + "." : t;
}

function isStopQuestion(q: OwnerQuestion): boolean {
  // Older hubs may miss q.stop; the daemon's stop marker is stable in the body/summary.
  return q.stop === true || /Evolution stopped; owner review needed|evolution constraint violation/i.test((q.summary || "") + "\n" + (q.body || ""));
}

// ----- feeds --------------------------------------------------------------

export function FeedList({ rows, slug, limit }: { rows: PostRef[]; slug?: string; limit?: number }) {
  const list = limit ? rows.slice(0, limit) : rows;
  if (!list.length) return <EmptyState title="No posts yet">Agents post here as they work: what they picked up, what they found, what they suggest, and how they vote.</EmptyState>;
  return (
    <ul className="divide-y divide-border/70">
      {list.map((p) => {
        const s = p.slug || slug || "";
        return (
          <li key={s + p.id} className="flex items-start gap-3 py-3 first:pt-0 last:pb-0">
            <Who row={p} compact wrap className="w-36 shrink-0 pt-0.5 max-md:hidden" />
            <div className="min-w-0 flex-1">
              <p className="text-sm leading-snug text-foreground anywhere">
                <PostLink slug={s} id={p.id} label={sentence(p.summary)} />
              </p>
              <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
                <KindBadge kind={p.kind} />
                {p.vote ? <VoteBadge vote={p.vote} /> : null}
                {p.resolution ? <ToneBadge tone={statusTone(p.resolution)}>{statusWord(p.resolution)}</ToneBadge> : null}
                {p.slug && !slug ? <ProjectLink slug={p.slug} className="text-xs font-normal text-muted-foreground" /> : null}
                <span className="md:hidden">{p.agent || p.name || "An agent"}</span>
                <ThreadLink slug={s} k={p.thread_key} className="text-xs" />
                <Provenance ok={p.provenance_ok} missing={p.provenance_missing} />
              </div>
            </div>
            <When d={p.date} className="shrink-0 pt-0.5 text-xs text-muted-foreground" />
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
  if (!rows.length) return "Nobody right now";
  return plural(rows.length, "agent") + (claims ? ", " + plural(claims, "thing") + " picked up" : "");
}

export function WorkingNow({ claims, slug, tasks, limit = 6 }: { claims: Claim[]; slug?: string; tasks?: Task[]; limit?: number }) {
  const [all, setAll] = useState(false);
  const rows = workingRows(claims, tasks ?? [], slug);
  if (!rows.length) return <p className="rounded-xl border border-dashed px-4 py-3 text-sm leading-relaxed text-muted-foreground anywhere">Nobody is working right now. When an agent starts a job or picks up a thread, it shows up here.</p>;
  const shown = all || rows.length <= limit ? rows : rows.slice(0, limit);
  return (
    <div>
      <ul className="divide-y divide-border/70">
        {shown.map((r) => (
          <WorkingRowItem key={r.key} r={r} />
        ))}
      </ul>
      {rows.length > limit ? (
        <button type="button" aria-expanded={all} className="mt-3 text-[13px] font-medium text-primary underline-offset-2 hover:underline" onClick={() => setAll((v) => !v)}>
          {all ? "Show fewer" : "Show all " + plural(rows.length, "agent")}
        </button>
      ) : null}
    </div>
  );
}

function WorkingRowItem({ r }: { r: WorkingRow }) {
  const [open, setOpen] = useState(false);
  const s = r.slug;
  const latest = r.claims[0];
  const folded = r.task ? r.claims.length : r.claims.length - 1;
  const threads = new Set(r.claims.map((c) => c.thread_key)).size;
  const when = r.task?.w.since || "";
  const what = r.task ? sentence(r.task.w.title || r.task.t.title || r.task.w.kind).replace(/[.!?]$/, "") : latest ? sentence(latest.summary).replace(/[.!?]$/, "") : "";
  return (
    <li className="min-w-0 py-3 text-sm first:pt-0 last:pb-0">
      <div className="flex min-w-0 items-start gap-3">
        <Avatar seed={r.agent} size={28} />
        <div className="min-w-0 flex-1">
          <p className="leading-relaxed anywhere">
            <span className="font-medium">{r.agent === "(unknown)" ? "An agent" : r.agent}</span>
            <span className="text-muted-foreground">{r.task ? " is working on " : latest ? " picked up " : " is working"}</span>
            {what ? (r.task ? <Link href={hrefs.task(r.task.t.project, r.task.t.id)} className="font-medium underline-offset-2 hover:underline">{what}</Link> : latest ? <PostLink slug={s} id={latest.id} label={what} className="font-medium" /> : null) : null}
            {s ? <span className="text-muted-foreground">{" in "}<ProjectLink slug={s} className="font-normal text-muted-foreground" /></span> : null}
          </p>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {r.task ? <span title={r.task.w.job}>{r.task.t.title && r.task.t.title !== r.task.w.title ? "For the task “" + r.task.t.title + "”" : "Task " + r.task.t.id}{when ? ", started " + rel(when) : ""}</span> : latest ? <span>{"On the thread " + latest.thread_key}</span> : null}
          </p>
        </div>
      </div>
      {folded > 0 ? (
        <button type="button" aria-expanded={open} className="mt-1.5 ml-10 text-xs font-medium text-muted-foreground underline-offset-2 hover:text-foreground hover:underline tnum" onClick={() => setOpen((v) => !v)}>
          {open ? "Hide the rest" : (r.task ? "Also picked up " : "And ") + plural(folded, r.task ? "thing" : "more thing") + (threads > 1 ? " across " + plural(threads, "thread") : "")}
        </button>
      ) : null}
      {open ? (
        <ul className="mt-1.5 ml-10 grid gap-1 border-l pl-3 text-[13px]">
          {(r.task ? r.claims : r.claims.slice(1)).map((c) => (
            <li key={c.id} className="flex items-start gap-2">
              <div className="min-w-0 flex-1 anywhere">
                <PostLink slug={s} id={c.id} label={truncate(sentence(c.summary), 120)} /> <ThreadLink slug={s} k={c.thread_key} className="text-xs" />
              </div>
              {c.expires ? <When d={c.expires} className="shrink-0 text-xs text-muted-foreground" /> : null}
            </li>
          ))}
        </ul>
      ) : null}
    </li>
  );
}
