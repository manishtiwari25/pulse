"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, ChevronRightIcon, ExternalLinkIcon, ServerIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Progress } from "@/components/ui/progress";
import { ConfirmDialog } from "./confirm";
import { Sparkline } from "./charts";
import { EmptyState, Kv, Panel } from "./panel";
import { KindBadge, Mono, PostLink, ProjectLink, Provenance, StatusBadge, ThreadLink, ToneBadge, VoteBadge, When, Who } from "./who";
import { cn } from "@/lib/utils";
import { expiresAt, num, plural, statusTone, untilText } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { writes, type Amendment, type Claim, type Executor, type PostRef, type ProjectCardData, type Proposal, type Runner, type Suggestion } from "@/lib/api";
import { useNow } from "@/lib/use-api";

// ----- project card ------------------------------------------------------

export function ProjectCard({ p, spark }: { p: ProjectCardData; spark?: number[] }) {
  const attention = p.proposals_awaiting > 0;
  return (
    <Link href={hrefs.project(p.slug)} className={cn("group block rounded-lg border bg-card p-3.5 shadow-xs transition-colors hover:bg-muted/40 focus-visible:ring-2 focus-visible:ring-ring", attention && "border-primary/40")}>
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="truncate text-[15px] font-semibold tracking-tight">{p.slug}</span>
            {p.brain ? <Badge variant="secondary">framework</Badge> : null}
            {p.kind === "commons" ? <Badge variant="outline">commons</Badge> : null}
            {!p.present ? <Badge variant="destructive">missing on disk</Badge> : null}
            {p.stop_present ? <ToneBadge tone="bad">STOP</ToneBadge> : null}
          </div>
          <div className="mt-0.5 truncate text-xs text-muted-foreground">
            {p.branch ? <Mono>{p.branch}</Mono> : null}
            {p.head ? <Mono className="ml-1.5 text-muted-foreground">{p.head}</Mono> : null}
            {p.dirty ? <span className="ml-1.5 text-warn">dirty</span> : null}
          </div>
        </div>
        <div className="flex items-center gap-2 text-muted-foreground">
          {spark?.length ? <Sparkline values={spark} w={72} h={20} /> : null}
          <ChevronRightIcon className="size-4 opacity-40 transition-transform group-hover:translate-x-0.5" />
        </div>
      </div>
      {p.error ? <p className="mt-2 text-xs text-bad anywhere">{p.error}</p> : null}
      <dl className="mt-3 grid grid-cols-3 gap-2 text-center sm:grid-cols-6">
        <Stat n={p.proposals_awaiting} label="for you" tone={p.proposals_awaiting ? "ok" : undefined} />
        <Stat n={p.claims_active} label="working" tone={p.claims_active ? "ok" : undefined} />
        <Stat n={p.threads_open} label="threads" />
        <Stat n={p.suggestions_eligible} label="eligible" tone={p.suggestions_eligible ? "accent" : undefined} />
        <Stat n={p.suggestions_needing_votes + p.amendments_open} label="in vote" tone={p.suggestions_needing_votes + p.amendments_open ? "warn" : undefined} />
        <Stat n={p.posts_24h} label="posts/24h" />
      </dl>
      {p.last_post ? (
        <p className="mt-3 truncate text-xs text-muted-foreground">
          <KindBadge kind={p.last_post.kind} /> <span className="ml-1">{p.last_post.summary}</span>
        </p>
      ) : (
        <p className="mt-3 text-xs text-muted-foreground">{p.board_present ? "No posts yet." : "No board yet - pulse init registers this checkout."}</p>
      )}
    </Link>
  );
}

function Stat({ n, label, tone }: { n: number; label: string; tone?: "ok" | "warn" | "accent" }) {
  return (
    <div className="rounded-md bg-muted/40 px-1 py-1.5">
      <dd className={cn("tnum text-[15px] font-semibold leading-none", tone === "ok" && "text-ok", tone === "warn" && "text-warn", tone === "accent" && "text-accent-2")}>{num(n)}</dd>
      <dt className="mt-1 text-[10.5px] uppercase tracking-wide text-muted-foreground">{label}</dt>
    </div>
  );
}

// ----- proposal card + decide ---------------------------------------------

export function ProposalCard({ r, slug, canDecide = true }: { r: Proposal; slug: string; canDecide?: boolean }) {
  const [open, setOpen] = useState<"yes" | "no" | null>(null);
  const awaiting = r.status === "awaiting";
  const [expanded, setExpanded] = useState(false);
  const long = (r.proposal_text || "").length > 420;
  return (
    <article className={cn("rounded-lg border bg-card p-3.5 shadow-xs", awaiting && "border-primary/40")} aria-labelledby={"prop-" + r.id}>
      <div className="flex flex-wrap items-center gap-1.5 text-xs">
        <StatusBadge status={r.status} />
        {r.slug && r.slug !== slug ? <ProjectLink slug={r.slug} /> : null}
        <ThreadLink slug={r.slug || slug} k={r.thread_key} />
        <span className="ml-auto flex items-center gap-2 text-muted-foreground">
          <Who row={r} compact />
          <When d={r.date} />
        </span>
      </div>
      <h3 id={"prop-" + r.id} className="mt-2 text-[14px] font-semibold leading-snug anywhere">
        {r.summary}
      </h3>
      {r.proposal_text ? (
        <div className="mt-2 text-[13px] leading-relaxed text-muted-foreground anywhere">
          <p className={cn(!expanded && long && "line-clamp-5")}>{r.proposal_text}</p>
          {long ? (
            <button type="button" className="mt-1 text-xs underline underline-offset-2 hover:text-foreground" onClick={() => setExpanded((e) => !e)}>
              {expanded ? "less" : "read the whole proposal"}
            </button>
          ) : null}
        </div>
      ) : null}
      {!awaiting && r.decided_id ? (
        <p className="mt-2 text-xs text-muted-foreground">
          decided <When d={r.decided_date} /> {r.decided_agent ? "by " + r.decided_agent : ""} {"·"} <PostLink slug={r.slug || slug} id={r.decided_id} label={r.decided_summary || "the decision post"} />
        </p>
      ) : null}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button asChild variant="ghost" size="sm">
          <Link href={hrefs.post(r.slug || slug, r.id)}>
            open the post <ExternalLinkIcon />
          </Link>
        </Button>
        {awaiting && canDecide && r.can_decide !== false ? (
          <span className="ml-auto flex gap-2">
            <Button size="sm" variant="outline" className="text-bad" onClick={() => setOpen("no")}>
              <XIcon /> No
            </Button>
            <Button size="sm" onClick={() => setOpen("yes")}>
              <CheckIcon /> Yes
            </Button>
          </span>
        ) : awaiting ? (
          <span className="ml-auto text-xs text-muted-foreground">read-only: open the hub on the machine that runs it to decide</span>
        ) : null}
      </div>
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
    </article>
  );
}

// ----- suggestion / amendment ---------------------------------------------

export function SuggestionCard({ s, quorum, slug, detail }: { s: Suggestion; quorum: number; slug: string; detail?: React.ReactNode }) {
  const now = useNow();
  const need = Math.max(0, quorum - s.voters);
  const exp = expiresAt(s);
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

export function WorkingNow({ claims, slug }: { claims: Claim[]; slug?: string }) {
  const now = useNow();
  if (!claims.length) return <EmptyState>Nobody holds a claim right now. A claim is an agent saying &quot;I am on this thread&quot; for a few hours.</EmptyState>;
  return (
    <ul className="divide-y">
      {claims.map((c) => {
        const s = c.slug || slug || "";
        const exp = c.expires ? new Date(c.expires) : null;
        return (
          <li key={s + c.id} className="flex items-start gap-2.5 py-2 first:pt-0 last:pb-0">
            <Who row={c} compact className="w-32 shrink-0" />
            <div className="min-w-0 flex-1 text-[13px]">
              <div className="flex flex-wrap items-center gap-1.5 text-xs">
                {c.slug && !slug ? <ProjectLink slug={c.slug} /> : null}
                <ThreadLink slug={s} k={c.thread_key} />
              </div>
              <p className="mt-0.5 leading-snug anywhere">
                <PostLink slug={s} id={c.id} label={c.summary} />
              </p>
            </div>
            <span className="shrink-0 text-xs text-muted-foreground tnum">{exp ? untilText(exp, now) : "no expiry"}</span>
          </li>
        );
      })}
    </ul>
  );
}

// ----- the machine ---------------------------------------------------------

export function MachinePanel({ r }: { r: Runner | undefined | null }) {
  if (!r) return <Panel title="The machine">{<EmptyState>The runner state is not available.</EmptyState>}</Panel>;
  if (!r.available) return <Panel title="The machine">{<EmptyState hint="pulse daemon">No store yet, or no daemon has run. Start one with the CLI.</EmptyState>}</Panel>;
  const d = r.daemon;
  const stopped = r.runner && !r.runner.allowed;
  return (
    <Panel
      title="The machine"
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
          ["hands", r.hands.free + " free of " + r.hands.slots + " slots on " + plural(r.hands.executors_active, "active executor")],
          ["workers", r.workers.busy + " busy · " + r.workers.idle + " idle" + (r.workers.idle_names.length ? " (" + r.workers.idle_names.join(", ") + ")" : "") + " · target " + r.workers.target],
          ["jobs", r.jobs.queued + " queued · " + r.jobs.leased + " leased · " + r.jobs.running + " running" + (r.jobs.blocked ? " · " + r.jobs.blocked + " blocked" : "")],
          ["runs today", r.jobs.runs_today + (r.jobs.cap ? " of " + r.jobs.cap : " (no daily cap)")],
        ]}
      />
      {stopped ? <p className="mt-2 text-xs text-bad">{"The runner is stopped: " + (r.runner?.reason || "the stop switch is on") + "."}</p> : null}
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

export function ExecutorRow({ e }: { e: Executor }) {
  return (
    <li className="flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-xs">
      <ServerIcon className="size-3.5 text-muted-foreground" />
      <span className="font-medium">{e.name}</span>
      <ToneBadge tone="neutral">{e.kind}</ToneBadge>
      <StatusBadge status={e.status} />
      <span className="ml-auto tnum text-muted-foreground">{e.running + " running · " + e.free + " free of " + e.workers}</span>
    </li>
  );
}
