"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, ExternalLinkIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Kv } from "@/components/pulse/panel";
import { Mono, ProjectLink, StatusBadge, ToneBadge, When, Who } from "@/components/pulse/who";
import { writes, type ComputeCaps, type ComputeRequest } from "@/lib/api";
import { num, plural } from "@/lib/format";
import { hrefs } from "@/lib/nav";

const CHANGES = (p: string) => p.startsWith("compute") || p.startsWith("executors") || p.startsWith("events") || p.startsWith("runner");

// Approve / deny buttons for one pending request; each confirms with words.
// Approve is the primary move, Deny stays quiet in the bad tone; `sm` by
// default so they are easy to hit on a card, `xs` for dense rows.
export function ComputeDecide({ r, size = "sm" }: { r: ComputeRequest; size?: "xs" | "sm" }) {
  const [which, setWhich] = useState<"approve" | "deny" | null>(null);
  if (r.status !== "pending") return null;
  return (
    <span className="inline-flex flex-wrap items-center gap-1.5">
      <Button size={size} className="glossy" onClick={() => setWhich("approve")} aria-label={"Approve " + r.id}>
        <CheckIcon /> Approve
      </Button>
      <Button size={size} variant="outline" onClick={() => setWhich("deny")} aria-label={"Deny " + r.id}>
        <XIcon /> Deny
      </Button>
      <ConfirmDialog
        open={which !== null}
        onOpenChange={(o) => !o && setWhich(null)}
        title={which === "approve" ? "Approve this machine?" : "Deny this machine?"}
        description={
          <span>
            <Mono>{r.requested_by}</Mono> asks for a <Mono>{r.size}</Mono> machine for {num(r.hours)} h. {which === "approve" ? "The daemon may provision it within the caps." : "The agent sees your denial on its next loop."}
          </span>
        }
        action={which === "approve" ? "Approve" : "Deny"}
        destructive={which === "deny"}
        words={{ label: "A note for the agents (optional)", placeholder: which === "approve" ? "Approved - needed for the load test." : "Not now - use the shared executor.", required: which === "deny" }}
        success={r.id + (which === "approve" ? " approved." : " denied.")}
        onConfirm={(w) => (which === "approve" ? writes.approveCompute(r.id, w) : writes.denyCompute(r.id, w))}
        invalidates={CHANGES}
      />
    </span>
  );
}

// One request as a card: who asks, for what and why, with Approve / Deny in
// reach while it is pending and the decision once made. There is no detail
// page, so the card is a plain article. `compact` clamps the reason.
export function ComputeRequestCard({ r, compact, caps }: { r: ComputeRequest; compact?: boolean; caps?: ComputeCaps }) {
  const pending = r.status === "pending";
  const decider = r.decided_by === "human" || r.decided_by === "owner" ? "you" : r.decided_by;
  return (
    <ItemCard highlight={pending} ariaLabel={r.id + " " + r.status.replace(/_/g, " ")}>
      <ItemCardHeader right={<When d={r.at} />}>
        <ProjectLink slug={r.project} className="text-xs" />
      </ItemCardHeader>
      <ItemCardTitle>
        {r.requested_by + " asks for a "}
        <Mono>{r.size}</Mono>
        {" machine for " + plural(r.hours, "hour")}
      </ItemCardTitle>
      {r.reason ? <ItemCardBody clamp={compact ? 3 : 0}>{r.reason}</ItemCardBody> : null}
      <ItemCardMeta>
        <ToneBadge tone="info">compute</ToneBadge>
        <StatusBadge status={r.status} />
        <Mono>{r.id}</Mono>
        <Who row={{ agent: r.requested_by }} compact />
        {caps ? (
          <>
            <span className="tnum">{num(caps.vms_now) + " of " + num(caps.max_vms) + " VMs up"}</span>
            <span className="tnum">{num(caps.hours_today) + " of " + num(caps.max_hours_per_day) + " h today"}</span>
            <span className="tnum">{"budget " + num(caps.budget_credits) + " credits"}</span>
            {caps.provider ? <Mono>{caps.provider}</Mono> : null}
          </>
        ) : null}
        {decider ? <span>decided by {decider}</span> : null}
        {r.executor ? (
          <span>
            executor <Mono>{r.executor}</Mono>
          </span>
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

export function CapsPanel({ caps }: { caps: ComputeCaps }) {
  const auto = caps.auto_approve === true || caps.auto_approve === 1;
  const pct = (value: number, max: number) => (max > 0 ? Math.max(0, Math.min(100, (value / max) * 100)) : 0);
  return (
    <div className="grid gap-3">
      <div className="grid gap-3">
        <CapMeter label="VMs up" value={caps.vms_now} max={caps.max_vms} percent={pct(caps.vms_now, caps.max_vms)} />
        <CapMeter label="hours today" value={caps.hours_today} max={caps.max_hours_per_day} percent={pct(caps.hours_today, caps.max_hours_per_day)} />
        <div className="flex items-center justify-between gap-3 rounded-lg border bg-muted/30 px-3 py-2">
          <span className="text-xs text-muted-foreground">budget</span>
          <span className="tnum font-medium">{num(caps.budget_credits) + " credits"}</span>
        </div>
        <Kv
          rows={[
            ["provider", <Mono key="p" className="rounded-full border bg-muted px-2 py-0.5">{caps.provider || "(none)"}</Mono>],
            ["auto-approve", <ToneBadge key="a" tone={auto ? "warn" : "neutral"}>{auto ? "on - requests within caps start without you" : "off - every request waits for you"}</ToneBadge>],
          ]}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        Caps, the provider and its target live in{" "}
        <Link className="underline" href={hrefs.settings("compute")}>
          Settings → Compute
        </Link>
        .
      </p>
    </div>
  );
}

function CapMeter({ label, value, max, percent }: { label: string; value: number; max: number; percent: number }) {
  return (
    <div className="grid gap-1.5 rounded-lg border bg-muted/30 px-3 py-2">
      <div className="flex items-center justify-between gap-3">
        <span className="text-xs text-muted-foreground">{label}</span>
        <span className="tnum font-medium">{num(value) + " / " + num(max)}</span>
      </div>
      <Progress value={percent} aria-label={label + " cap"} />
    </div>
  );
}

// One pending compute request as an inbox card: who asks for what, for how
// long and why, with Approve / Deny in reach (ComputeDecide confirms each).
export function ComputeInboxCard({ r, caps }: { r: ComputeRequest; caps?: ComputeCaps }) {
  return (
    <ItemCard highlight ariaLabel={"Compute request " + r.id}>
      <ItemCardHeader
        right={
          <>
            <Who row={{ agent: r.requested_by }} compact />
            <When d={r.at} />
          </>
        }
      >
        <Mono className="font-semibold">{r.id}</Mono>
        <StatusBadge status={r.status} />
        <ToneBadge tone="info">compute</ToneBadge>
        <ProjectLink slug={r.project} className="text-xs" />
      </ItemCardHeader>
      <ItemCardTitle>
        {r.requested_by + " asks for "}
        <Mono>{r.size}</Mono>
        {" for " + plural(r.hours, "hour")}
      </ItemCardTitle>
      <ItemCardBody clamp={3}>{r.reason}</ItemCardBody>
      {caps ? (
        <ItemCardMeta>
          <span className="tnum">{num(caps.vms_now) + " of " + num(caps.max_vms) + " VMs up"}</span>
          <span className="tnum">{num(caps.hours_today) + " of " + num(caps.max_hours_per_day) + " h today"}</span>
          <span className="tnum">{"budget " + num(caps.budget_credits) + " credits"}</span>
          {caps.provider ? <Mono>{caps.provider}</Mono> : null}
        </ItemCardMeta>
      ) : null}
      <ItemCardActions>
        <Button asChild variant="ghost" size="xs" className="text-muted-foreground">
          <Link href={hrefs.compute()}>
            all requests <ExternalLinkIcon />
          </Link>
        </Button>
        <span className="flex w-full justify-start sm:ml-auto sm:w-auto sm:justify-end">
          <ComputeDecide r={r} />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}
