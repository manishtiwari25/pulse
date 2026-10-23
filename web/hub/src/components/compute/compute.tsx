"use client";

import Link from "next/link";
import { useState } from "react";
import { ArrowRightIcon, CheckIcon, ServerIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardKind, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Mono, ProjectLink, StatusBadge, When } from "@/components/pulse/who";
import { statusWord } from "@/lib/words";
import { writes, type ComputeCaps, type ComputeRequest } from "@/lib/api";
import { num, plural } from "@/lib/format";
import { hrefs } from "@/lib/nav";

const CHANGES = (p: string) => p.startsWith("compute") || p.startsWith("executors") || p.startsWith("events") || p.startsWith("runner");

// Approve / deny buttons for one pending request; each confirms with words.
// Approve is the primary move, Deny stays quiet in the bad tone; `sm` by
// default so they are easy to hit on a card, `xs` for dense rows.
export function ComputeDecide({ r, size = "sm" }: { r: ComputeRequest; size?: "xs" | "sm" | "default" }) {
  const [which, setWhich] = useState<"approve" | "deny" | null>(null);
  if (r.status !== "pending") return null;
  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <Button size={size} variant="outline" onClick={() => setWhich("deny")} aria-label={"Deny " + r.id}>
        <XIcon /> Deny
      </Button>
      <Button size={size} onClick={() => setWhich("approve")} aria-label={"Approve " + r.id}>
        <CheckIcon /> Approve
      </Button>
      <ConfirmDialog
        open={which !== null}
        onOpenChange={(o) => !o && setWhich(null)}
        title={which === "approve" ? "Approve the extra machine?" : "Deny the extra machine?"}
        description={
          <span>
            {r.requested_by} asks for one <Mono>{r.size}</Mono> machine for {plural(r.hours, "hour")}. {which === "approve" ? "PULSE starts it within the limits you set, and removes it when the time is up." : "The agent sees your answer the next time it checks."}
          </span>
        }
        action={which === "approve" ? "Approve" : "Deny"}
        destructive={which === "deny"}
        words={{ label: which === "deny" ? "Tell the agent why" : "A note for the agent (optional)", placeholder: which === "approve" ? "Approved: needed for the load test." : "Not now. Use the machines you have.", required: which === "deny" }}
        success={which === "approve" ? "Approved. PULSE will start the machine." : "Denied. The agent will see your note."}
        onConfirm={(w) => (which === "approve" ? writes.approveCompute(r.id, w) : writes.denyCompute(r.id, w))}
        invalidates={CHANGES}
      />
    </span>
  );
}

// Approved requests that are starting or running: their machine is live.
const LIVE = new Set(["approved", "provisioning", "ready"]);

// One request as a card: who asked for what and why, with Approve / Deny in
// reach while it is pending and your answer once made. There is no detail
// page, so the card is a plain article; the request id sits in tooltips.
// `compact` clamps the reason; `caps` adds the limits to a pending card.
export function ComputeRequestCard({ r, compact, caps }: { r: ComputeRequest; compact?: boolean; caps?: ComputeCaps }) {
  const pending = r.status === "pending";
  const decider = r.decided_by === "human" || r.decided_by === "owner" ? "you" : r.decided_by;
  return (
    <ItemCard highlight={pending} ariaLabel={"Request " + r.id + ": " + statusWord(r.status)}>
      <ItemCardHeader right={<When d={r.at} />}>
        <ItemCardKind icon={ServerIcon} tone={pending ? "you" : LIVE.has(r.status) ? "ok" : "neutral"}>{"Extra machine for " + r.requested_by}</ItemCardKind>
        <ProjectLink slug={r.project} className="font-normal text-muted-foreground" />
      </ItemCardHeader>
      <ItemCardTitle>
        <span title={"Request " + r.id}>
          {"One "}
          <Mono>{r.size}</Mono>
          {" machine for " + plural(r.hours, "hour")}
        </span>
      </ItemCardTitle>
      {r.reason ? <ItemCardBody clamp={compact ? 3 : 0}>{"\u201c" + r.reason.trim() + "\u201d"}</ItemCardBody> : null}
      <ItemCardMeta>
        <StatusBadge status={r.status} />
        {decider && !pending ? <span>{(r.status === "denied" ? "Denied" : "Approved") + " by " + decider}</span> : null}
        {r.executor ? <span>{"Runs on " + r.executor}</span> : null}
        {pending && caps ? (
          <>
            <span>{num(caps.vms_now) + " of " + num(caps.max_vms) + " extra machines in use"}</span>
            <span>{num(caps.hours_today) + " of " + plural(caps.max_hours_per_day, "hour") + " used today"}</span>
          </>
        ) : null}
      </ItemCardMeta>
      {pending ? (
        <ItemCardActions className="justify-end">
          <ComputeDecide r={r} />
        </ItemCardActions>
      ) : null}
    </ItemCard>
  );
}

const PROVIDER: Record<string, string> = { azure: "Microsoft Azure", aws: "Amazon Web Services", gcp: "Google Cloud", google: "Google Cloud", hetzner: "Hetzner Cloud", digitalocean: "DigitalOcean", ssh: "Your own machines over SSH" };
export const providerWord = (p?: string | null) => (p ? (PROVIDER[p.toLowerCase()] ?? p.charAt(0).toUpperCase() + p.slice(1)) : "");

// The limits every request must fit, in words, with the two that fill up
// shown as meters. They are changed in Settings, linked at the end.
export function CapsPanel({ caps }: { caps: ComputeCaps }) {
  const auto = caps.auto_approve === true || caps.auto_approve === 1;
  const pct = (value: number, max: number) => (max > 0 ? Math.max(0, Math.min(100, (value / max) * 100)) : 0);
  const full = caps.max_hours_per_day > 0 && caps.hours_today >= caps.max_hours_per_day;
  return (
    <div className="grid gap-4">
      <dl className="grid gap-x-8 gap-y-4 text-sm sm:grid-cols-2">
        <CapMeter label="Extra machines" text={num(caps.vms_now) + " of " + num(caps.max_vms) + " in use"} percent={pct(caps.vms_now, caps.max_vms)} />
        <CapMeter label="Hours used today" text={num(caps.hours_today) + " of " + num(caps.max_hours_per_day) + (full ? ", none left today" : "")} percent={pct(caps.hours_today, caps.max_hours_per_day)} />
        <CapFact label="Budget" value={num(caps.budget_credits) + " credits"} />
        <CapFact label="Cloud provider" value={providerWord(caps.provider) || "None set up yet"} title={caps.provider || undefined} />
        <CapFact className="sm:col-span-2" label="Auto-approve" value={auto ? "On: requests within the limits start by themselves" : "Off: every request waits for you"} />
      </dl>
      <p className="text-[13px] text-muted-foreground">
        <Link className="font-medium text-foreground underline underline-offset-2" href={hrefs.settings("compute")}>
          Change these in Settings
        </Link>
      </p>
    </div>
  );
}

function CapFact({ label, value, title, className }: { label: string; value: string; title?: string; className?: string }) {
  return (
    <div className={className}>
      <dt className="text-[13px] text-muted-foreground">{label}</dt>
      <dd className="m-0 mt-0.5 font-medium text-foreground anywhere" title={title}>{value}</dd>
    </div>
  );
}

function CapMeter({ label, text, percent }: { label: string; text: string; percent: number }) {
  return (
    <div>
      <dt className="text-[13px] text-muted-foreground">{label}</dt>
      <dd className="m-0 mt-0.5 grid gap-2">
        <span className="font-medium text-foreground">{text}</span>
        <Progress value={percent} aria-label={label} />
      </dd>
    </div>
  );
}

// One pending compute request as an inbox card: who asks for what, for how
// long and why, with Approve / Deny in reach (ComputeDecide confirms each).
// `here` drops the "All requests" link when the card already sits on the Machines page.
export function ComputeInboxCard({ r, caps, here }: { r: ComputeRequest; caps?: ComputeCaps; here?: boolean }) {
  return (
    <ItemCard highlight ariaLabel={"Compute request " + r.id + " from " + r.requested_by}>
      <ItemCardHeader right={<When d={r.at} />}>
        <ItemCardKind icon={ServerIcon} tone="you">{"Extra machine for " + r.requested_by}</ItemCardKind>
        <ProjectLink slug={r.project} className="font-normal text-muted-foreground" />
      </ItemCardHeader>
      <ItemCardTitle>
        {"One "}
        <Mono>{r.size}</Mono>
        {" machine for " + plural(r.hours, "hour")}
      </ItemCardTitle>
      {r.reason ? <ItemCardBody clamp={3}>{"“" + r.reason.trim() + "”"}</ItemCardBody> : null}
      {caps ? (
        <ItemCardMeta>
          <span>{num(caps.vms_now) + " of " + num(caps.max_vms) + " extra machines in use"}</span>
          <span>{num(caps.hours_today) + " of " + plural(caps.max_hours_per_day, "hour") + " used today"}</span>
          {caps.provider ? <span title={caps.provider}>{"Runs on " + providerWord(caps.provider)}</span> : null}
        </ItemCardMeta>
      ) : null}
      <ItemCardActions>
        {here ? null : (
        <Link href={hrefs.compute()} title={"Request " + r.id} className="inline-flex items-center gap-1 text-[13px] font-medium text-muted-foreground underline-offset-2 hover:text-foreground hover:underline">
          All requests <ArrowRightIcon className="size-3.5" aria-hidden />
        </Link>
        )}
        <span className="flex w-full justify-start sm:ml-auto sm:w-auto sm:justify-end">
          <ComputeDecide r={r} size="default" />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}
