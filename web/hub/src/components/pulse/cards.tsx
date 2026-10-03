"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, ChevronRightIcon, ServerIcon, UsersIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Progress } from "@/components/ui/progress";
import { ConfirmDialog } from "./confirm";
import { DecideButtons } from "./decide-buttons";
import { Sparkline } from "./charts";
import { EmptyState, Kv, Panel } from "./panel";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "./item-card";
import { Avatar, KindBadge, Mono, PostLink, ProjectLink, Provenance, StatusBadge, ThreadLink, ToneBadge, VoteBadge, When, Who } from "./who";
import { cn } from "@/lib/utils";
import { expiresAt, firstSentence, hashHue, initials, num, plural, rel, sentence, statusTone, truncate, untilText } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { COMMONS_HINT, evolveGate, taskProgress, writes, type Amendment, type Claim, type EvolveGate, type Executor, type LandingHold, type OwnerQuestion, type PostRef, type ProjectCardData, type Proposal, type Runner, type Suggestion, type Task, type TaskWorker } from "@/lib/api";
import { useNow } from "@/lib/use-api";
import { gateStep, type GateStep } from "./evolve";

// ----- project card ------------------------------------------------------

function ProjectAvatar({ slug }: { slug: string }) {
  return (
    <span className="flex size-10 shrink-0 items-center justify-center rounded-xl text-sm font-semibold text-white shadow-card" style={{ background: `oklch(0.62 0.12 ${hashHue(slug)})` }} aria-hidden>
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
  const attention = forYou;
  const last = p.last_post?.date || p.built_at;
  const status = projectStatus(p, step, forYou);
  const footerFacts = [plural(p.agents, "agent"), plural(p.threads_open, "open thread"), last ? "last post " + rel(last) : "no activity yet"];
  return (
    <ItemCard href={hrefs.project(p.slug)} highlight={attention > 0} ariaLabel={"Project " + p.slug} className="@container/project gap-4 surface-raised">
      <div className="flex min-w-0 items-start justify-between gap-3">
        <div className="flex min-w-0 flex-1 items-start gap-3">
          <ProjectAvatar slug={p.slug} />
          <div className="min-w-0 flex-1">
            <h3 title={p.slug} className="line-clamp-2 text-xl font-semibold leading-tight tracking-tight anywhere">{p.slug}</h3>
            <p className="mt-1 text-sm text-muted-foreground anywhere">{p.brain ? "Framework project" : p.name && p.name !== p.slug ? p.name : "Software project"}</p>
          </div>
        </div>
        <ChevronRightIcon className="mt-1 size-4 shrink-0 text-muted-foreground opacity-50 transition-[transform,opacity] group-hover/item:opacity-100 group-focus-visible/item:opacity-100 motion-safe:group-hover/item:translate-x-0.5 motion-reduce:transition-none" aria-hidden />
      </div>
      <div className="flex min-w-0 items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          <span className={cn("size-2.5 shrink-0 rounded-full", status.dot)} aria-label={status.aria} role="img" />
          <p className={cn("text-sm font-medium leading-snug anywhere", status.text)}>{status.line}</p>
        </div>
        {spark?.length ? <Sparkline values={spark} w={86} h={24} className="shrink-0" /> : null}
      </div>
      <ItemCardMeta>
        {footerFacts.map((f) => <span key={f} title={[p.branch, p.head, p.dirty ? "dirty" : "", g ? "level " + g.evolve_level : ""].filter(Boolean).join(" · ") || undefined}>{f}</span>)}
      </ItemCardMeta>
    </ItemCard>
  );
}

function projectStatus(p: ProjectCardData, step: GateStep | null, forYou: number): { line: string; dot: string; text: string; aria: string } {
  if (p.error || p.missing || !p.present) return { line: "Missing on disk", dot: "bg-bad", text: "text-bad", aria: "bad status" };
  if (p.stop_present) return { line: "Self-improvement stopped", dot: "bg-warn", text: "text-warn", aria: "warning status" };
  if (forYou > 0) return { line: plural(forYou, "thing") + (forYou === 1 ? " needs you" : " need you"), dot: "bg-primary", text: "text-brand", aria: "needs owner attention" };
  if (p.claims_active > 0) return { line: "An experiment is running", dot: "bg-info", text: "text-info", aria: "running status" };
  if (step) return { line: "Next step: " + sentence(step.label), dot: "bg-warn", text: "text-warn", aria: "next step status" };
  return { line: "Quiet — " + plural(p.agents, "agent") + (p.last_post?.date ? ", last post " + rel(p.last_post.date) : ", no posts yet"), dot: "bg-ok", text: "text-foreground", aria: "quiet status" };
}

// ----- shared board ------------------------------------------------------

// Whether the shared board has anything the owner should see right now: an
// open question or a post today. A quiet board stays off the home page; the
// project switcher, the command palette and the project picker still reach it.
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
      aria-label="Shared board - the board every project shares"
      className={cn("group/commons flex min-w-0 items-center gap-3 rounded-xl surface-glass px-4 py-3 text-sm leading-relaxed transition-colors hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring", className)}
    >
      <span className={cn("flex size-8 shrink-0 items-center justify-center rounded-lg", questions ? "bg-primary/10 text-primary" : "bg-muted text-muted-foreground")} aria-hidden>
        <UsersIcon className="size-4" />
      </span>
      <span className="min-w-0 flex-1 anywhere">
        <span className="font-medium text-foreground">Shared board</span>
        <span className="text-muted-foreground"> · for things that touch more than one project · {facts.join(" · ")}</span>
      </span>
      <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground opacity-50 transition-[transform,opacity] group-hover/commons:opacity-100 group-focus-visible/commons:opacity-100 motion-safe:group-hover/commons:translate-x-0.5 motion-reduce:transition-none" aria-hidden />
    </Link>
  );
}

// ----- proposal card + decide ---------------------------------------------

export function ProposalCard({ r, slug, canDecide = true }: { r: Proposal; slug: string; canDecide?: boolean }) {
  const [open, setOpen] = useState<"yes" | "no" | null>(null);
  const awaiting = r.status === "awaiting";
  const [expanded, setExpanded] = useState(false);
  const long = (r.proposal_text || "").length > 420;
  const s = r.slug || slug;
  const who = r.agent || r.name || r.session || "An agent";
  return (
    <ItemCard highlight={awaiting} ariaLabel={"Proposal: " + r.summary}>
      <ItemCardHeader right={<><Who row={r} compact /><When d={r.date} /></>}>
        {r.slug && r.slug !== slug ? <ProjectLink slug={r.slug} /> : null}
      </ItemCardHeader>
      <ItemCardTitle>{sentence(who) + " proposes: " + sentence(r.summary)}</ItemCardTitle>
      {r.proposal_text ? (
        <ItemCardBody clamp={0}>
          <p className={cn(!expanded && long && "line-clamp-3")}>{firstSentence(r.proposal_text)}</p>
          {long ? (
            <button type="button" aria-expanded={expanded} className="mt-1 text-xs underline underline-offset-2 hover:text-foreground" onClick={() => setExpanded((e) => !e)}>
              {expanded ? "Show less" : "Read more"}
            </button>
          ) : null}
          {expanded ? <p className="mt-2 whitespace-pre-line">{r.proposal_text}</p> : null}
        </ItemCardBody>
      ) : null}
      <ItemCardMeta>
        <StatusBadge status={r.status} />
        <Mono>{r.id}</Mono>
        <ThreadLink slug={s} k={r.thread_key} />
        <Link href={hrefs.post(s, r.id)} className="hover:underline">open the post</Link>
        {!awaiting && r.decided_id ? <span>decided <When d={r.decided_date} /> {r.decided_agent ? "by " + r.decided_agent : ""}</span> : null}
      </ItemCardMeta>
      <ItemCardActions>
        {awaiting && canDecide && r.can_decide !== false ? (
          <span className="flex w-full flex-wrap justify-start gap-2 sm:ml-auto sm:w-auto sm:justify-end">
            <Button variant="outline" onClick={() => setOpen("no")} aria-label={"No, decline: " + r.summary}><XIcon /> No, decline</Button>
            <Button className="glossy" onClick={() => setOpen("yes")} aria-label={"Yes, accept: " + r.summary}><CheckIcon /> Yes, accept</Button>
          </span>
        ) : awaiting ? (
          <span className="ml-auto text-xs text-muted-foreground">read-only: open the hub on the machine that runs it to decide</span>
        ) : null}
      </ItemCardActions>
      <ConfirmDialog
        open={open !== null}
        onOpenChange={(o) => !o && setOpen(null)}
        title={open === "yes" ? "Say yes to this?" : "Say no to this?"}
        description={open === "yes" ? "The agents may proceed with this proposal." : "The proposal is declined and the agents will not pursue it as written."}
        action={open === "yes" ? "Yes, accept" : "No, decline"}
        destructive={open === "no"}
        words={{ label: "A note for the agents (optional)", placeholder: open === "yes" ? "Approved - keep the scope as written." : "Not now because …" }}
        success={open === "yes" ? "Decided yes - the board records your answer; a job is queued when the proposal asks for work." : "Decided no - the board records your answer."}
        onConfirm={(words) => writes.decide(r.id, open === "yes" ? "yes" : "no", words)}
      />
    </ItemCard>
  );
}

// ----- owner question --------------------------------------------------------

// A question that waits for the owner (ADR: for_owner posts and the evolution
// loop's stop questions). Modelled on the proposal card: who asked, where,
// the ask, and Yes / No that record the owner's decision on the board.
export function QuestionCard({ q, slug, canDecide = true }: { q: OwnerQuestion; slug: string; canDecide?: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const body = (q.body || "").trim();
  const long = body.length > 420 || body.split("\n").length > 5;
  const s = q.slug || slug;
  const stop = isStopQuestion(q);
  const title = stop ? <>The self-improvement loop on <b>{s}</b> stopped — may it continue?</> : sentence(q.summary).replace(/[.!]*$/, q.summary.trim().endsWith("?") ? "?" : ".");
  const askLine = stop ? firstSentence(body) : "asked by " + (q.agent || q.name || q.session || "an agent") + " · ";
  return (
    <ItemCard highlight={!q.stale} ariaLabel={"Question for you: " + q.summary}>
      <ItemCardHeader right={stop ? <When d={q.date} /> : undefined}>
        {q.slug && q.slug !== slug ? <ProjectLink slug={q.slug} /> : null}
      </ItemCardHeader>
      <ItemCardTitle>{title}</ItemCardTitle>
      <ItemCardBody clamp={2}>
        {stop ? askLine : <>{askLine}<When d={q.date} /></>}
      </ItemCardBody>
      {body ? (
        <ItemCardBody clamp={0}>
          {expanded ? <p className="whitespace-pre-line">{body}</p> : null}
          {long || !stop ? (
            <button type="button" aria-expanded={expanded} className="mt-1 text-xs underline underline-offset-2 hover:text-foreground" onClick={() => setExpanded((e) => !e)}>
              {expanded ? "Show less" : "Read more"}
            </button>
          ) : null}
        </ItemCardBody>
      ) : null}
      <ItemCardMeta>
        {stop ? <ToneBadge tone="bad">evolution stop</ToneBadge> : <ToneBadge tone="warn">question for you</ToneBadge>}
        {q.stale ? <Badge variant="outline">older thread</Badge> : null}
        <Mono>{q.id}</Mono>
        <ThreadLink slug={s} k={q.thread_key} />
        {q.on_post ? <span>about <PostLink slug={s} id={q.on_post} label={q.on_post} /></span> : null}
        <Link href={hrefs.post(s, q.id)} className="hover:underline">open the post</Link>
      </ItemCardMeta>
      <ItemCardActions>
        <DecideButtons id={q.id} summary={typeof title === "string" ? title : q.summary} stop={stop} canDecide={canDecide} yesLabel={stop ? "Yes, continue" : "Yes"} noLabel={stop ? "No, keep it stopped" : "No"} />
      </ItemCardActions>
    </ItemCard>
  );
}

function isStopQuestion(q: OwnerQuestion): boolean {
  // Older hubs may miss q.stop; the daemon's stop marker is stable in the body/summary.
  return q.stop === true || /Evolution stopped; owner review needed|evolution constraint violation/i.test((q.summary || "") + "\n" + (q.body || ""));
}

// ----- suggestion / amendment ---------------------------------------------

export function SuggestionCard({ s, quorum, slug, detail }: { s: Suggestion; quorum: number; slug: string; detail?: React.ReactNode }) {
  const now = useNow();
  const need = Math.max(0, quorum - s.voters);
  const exp = expiresAt(s);
  const planned = s.status === "eligible" ? (s.job ? s.gate || "job " + s.job + (s.job_status ? " is " + s.job_status : "") : s.gate ? "not planned: " + s.gate : s.plannable ? "plannable — the daemon queues it at its next tick" : "") : "";
  const plannedTone = s.job ? "text-muted-foreground" : s.gate ? "text-warn" : "text-ok";
  return (
    <ItemCard className="gap-0">
      <div className="flex min-w-0 flex-wrap items-center gap-1.5 text-xs anywhere">
        <StatusBadge status={s.status} />
        {s.rank ? <ToneBadge tone="neutral">#{s.rank}</ToneBadge> : null}
        {s.slug && s.slug !== slug ? <ProjectLink slug={s.slug} /> : null}
        <ThreadLink slug={s.slug || slug} k={s.thread_key} />
        <span className="ml-auto flex min-w-0 max-w-full flex-wrap items-center gap-2 text-muted-foreground">
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
    </ItemCard>
  );
}

export function AmendmentCard({ a, slug, detail }: { a: Amendment; slug: string; detail?: React.ReactNode }) {
  const uncounted = a.uncounted_self + a.uncounted_hash_mismatch + a.uncounted_no_read_line;
  return (
    <ItemCard className="gap-0">
      <div className="flex min-w-0 flex-wrap items-center gap-1.5 text-xs anywhere">
        <StatusBadge status={a.status} />
        {a.section ? <ToneBadge tone="warn">{"section: " + a.section}</ToneBadge> : null}
        {a.slug && a.slug !== slug ? <ProjectLink slug={a.slug} /> : null}
        <ThreadLink slug={a.slug || slug} k={a.thread_key} />
        <span className="ml-auto flex min-w-0 max-w-full flex-wrap items-center gap-2 text-muted-foreground">
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
    </ItemCard>
  );
}

// ----- feeds --------------------------------------------------------------

export function FeedList({ rows, slug, limit }: { rows: PostRef[]; slug?: string; limit?: number }) {
  const list = limit ? rows.slice(0, limit) : rows;
  if (!list.length) return <EmptyState>No posts yet. Agents post claims, findings, suggestions and votes with the CLI.</EmptyState>;
  return (
    <ul className="divide-y divide-border/70">
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
          <WorkingRowItem key={r.key} r={r} />
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

function WorkingRowItem({ r }: { r: WorkingRow }) {
  const [open, setOpen] = useState(false);
  const s = r.slug;
  const latest = r.claims[0];
  const folded = r.task ? r.claims.length : r.claims.length - 1;
  const threads = new Set(r.claims.map((c) => c.thread_key)).size;
  const when = r.task?.w.since || latest?.expires || "";
  const work = r.task
    ? plainWork(r.task.w.title || r.task.t.title || r.task.w.kind)
    : latest
      ? plainWork(latest.summary)
      : "is working";
  return (
    <li className="min-w-0 py-3 text-[13px] first:pt-0 last:pb-0">
      <div className="flex min-w-0 items-start gap-2.5">
        <Avatar seed={r.agent} size={28} />
        <div className="min-w-0 flex-1">
          <p className="line-clamp-2 leading-relaxed anywhere">
            <span className="font-medium">{r.agent}</span>{" "}{work}{s ? <> on <ProjectLink slug={s} className="font-normal" /></> : null}
            {when ? <span className="text-muted-foreground">{" · " + rel(when)}</span> : null}
          </p>
          <ItemCardMeta className="mt-1">
            {r.task ? <Link href={hrefs.task(r.task.t.project, r.task.t.id)} className="hover:underline"><Mono>{r.task.t.id}</Mono></Link> : latest ? <><ThreadLink slug={s} k={latest.thread_key} /><PostLink slug={s} id={latest.id} label="post" /></> : null}
          </ItemCardMeta>
        </div>
      </div>
      {folded > 0 ? (
        <button type="button" aria-expanded={open} className="mt-1 ml-10 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline tnum" onClick={() => setOpen((v) => !v)}>
          {open ? "hide claims" : (r.task ? "also holds " : "+ ") + plural(folded, r.task ? "claim" : "more claim") + (threads > 1 ? " across " + plural(threads, "thread") : "")}
        </button>
      ) : null}
      {open ? (
        <ul className="mt-1.5 ml-10 grid gap-1 border-l pl-2.5 text-[12.5px]">
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

function plainWork(s: string): string {
  const t = sentence(s).replace(/[.!?]$/, "");
  if (!t) return "is working";
  return /^(is|are|has|have)\b/i.test(t) ? t.charAt(0).toLowerCase() + t.slice(1) : "is " + t.charAt(0).toLowerCase() + t.slice(1);
}

// ----- the machine ---------------------------------------------------------

export function MachinePanel({ r }: { r: Runner | undefined | null }) {
  const [details, setDetails] = useState(false);
  if (!r) return <Panel title="The machine" icon={ServerIcon}>{<EmptyState>The runner state is not available.</EmptyState>}</Panel>;
  if (!r.available) return <Panel title="The machine" icon={ServerIcon}>{<EmptyState hint="pulse daemon">No store yet, or no daemon has run. Start one with the CLI.</EmptyState>}</Panel>;
  const d = r.daemon;
  const stopped = r.runner && !r.runner.allowed;
  const daemonOk = !stopped && (d.state === "running" || d.state === "alive");
  const leftovers = hygieneLeftovers(r.hygiene);
  return (
    <Panel
      title="The machine"
      icon={ServerIcon}
      actions={
        <Link href={hrefs.settings("runner")} className="text-xs text-muted-foreground hover:underline">
          caps
        </Link>
      }
    >
      <div className="grid gap-2 text-sm">
        <HumanMachineLine tone={stopped ? "bad" : daemonOk ? "ok" : "warn"} label="Daemon" text={(stopped ? "stopped" : d.state || d.text || "unknown") + (d.at ? ", checked " + rel(d.at) : "")} />
        <HumanMachineLine tone={r.workers.busy ? "info" : "neutral"} label="Agents" text={plural(r.workers.busy, "working") + ", " + plural(r.workers.idle, "resting")} />
        <HumanMachineLine tone={r.jobs.blocked ? "bad" : r.jobs.running || r.jobs.leased ? "info" : r.jobs.queued ? "warn" : "ok"} label="Work" text={r.jobs.blocked ? plural(r.jobs.blocked, "blocked") + " — see failed jobs" : plural(r.jobs.queued, "waiting") + ", " + plural(r.jobs.running + r.jobs.leased, "running")} />
        <HumanMachineLine tone={leftovers.count ? "warn" : "ok"} label="Cleanup" text={leftovers.text} />
      </div>
      {stopped ? <p className="mt-2 text-xs text-bad">{"The runner is stopped: " + (r.runner?.reason || "the stop switch is on") + "."}</p> : null}
      <details className="mt-3" open={details} onToggle={(e) => setDetails(e.currentTarget.open)}>
        <summary className="cursor-pointer select-none text-xs font-medium text-muted-foreground hover:text-foreground" aria-expanded={details}>Show details</summary>
        <div className="mt-2 grid gap-2">
          <Kv
            rows={[
              ["daemon", (d.text || d.state) + (d.loops !== undefined ? " · loop " + num(d.loops) : "")],
              ["interval", d.interval ? spanSeconds(d.interval) : "—"],
              ["last loop", <When key="at" d={d.at} />],
              ["slots", r.hands.free + " free of " + r.hands.slots + " on " + plural(r.hands.executors_active, "active executor")],
              ["workers", r.workers.busy + " busy · " + r.workers.idle + " idle" + (r.workers.idle_names.length ? " (" + r.workers.idle_names.join(", ") + ")" : "") + " · target " + r.workers.target],
              ["jobs", r.jobs.queued + " queued · " + r.jobs.leased + " leased · " + r.jobs.running + " running" + (r.jobs.blocked ? " · " + r.jobs.blocked + " blocked" : "")],
              ...(r.capacity ? [["capacity", r.capacity] as [React.ReactNode, React.ReactNode]] : []),
              ["runs today", r.jobs.runs_today + (r.jobs.cap ? " of " + r.jobs.cap : " (no daily cap)")],
              ...(r.hygiene ? [["hygiene", r.hygiene] as [React.ReactNode, React.ReactNode]] : []),
            ]}
          />
          <WorkerHold hold={r.hold} />
          <LandingHolds holds={r.landing} />
          {r.executors.length ? (
            <ul className="grid gap-1.5">
              {r.executors.map((e) => <ExecutorRow key={e.name} e={e} />)}
            </ul>
          ) : null}
        </div>
      </details>
    </Panel>
  );
}

function HumanMachineLine({ tone, label, text }: { tone: "ok" | "warn" | "bad" | "info" | "neutral"; label: string; text: string }) {
  const dot = tone === "ok" ? "bg-ok" : tone === "warn" ? "bg-warn" : tone === "bad" ? "bg-bad" : tone === "info" ? "bg-info" : "bg-muted-foreground";
  return (
    <div className="flex min-w-0 items-start gap-2 rounded-lg border bg-muted/20 px-3 py-2">
      <span className={cn("mt-1.5 size-2 shrink-0 rounded-full", dot)} aria-label={tone + " status"} role="img" />
      <p className="min-w-0 leading-snug anywhere"><b>{label}:</b> {text}</p>
    </div>
  );
}

function hygieneLeftovers(h?: string): { count: number; text: string } {
  if (!h) return { count: 0, text: "nothing left behind" };
  const held = [...h.matchAll(/held\s+([a-z]+)\s+(\d+)/gi)].reduce((n, m) => n + Number(m[2] || 0), 0);
  const swept = /last sweep\s+([^:]+):/i.exec(h)?.[1];
  return held ? { count: held, text: plural(held, "leftover") + (swept ? ", swept " + rel(swept) : "") } : { count: 0, text: "nothing left behind" };
}

const spanSeconds = (sec: number) => sec < 60 ? plural(sec, "second") : sec < 3600 ? plural(Math.round(sec / 60), "minute") : plural(Math.round(sec / 3600), "hour");

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
