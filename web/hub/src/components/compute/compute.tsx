"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardMeta, ItemCardTitle } from "@/components/pulse/item-card";
import { Kv } from "@/components/pulse/panel";
import { Mono, ProjectLink, StatusBadge, ToneBadge, When, Who } from "@/components/pulse/who";
import { Bar2 } from "@/components/pulse/charts";
import { writes, type ComputeCaps, type ComputeRequest } from "@/lib/api";
import { num } from "@/lib/format";
import { hrefs } from "@/lib/nav";

const CHANGES = (p: string) => p.startsWith("compute") || p.startsWith("executors") || p.startsWith("events") || p.startsWith("runner");

// Approve / deny buttons for one pending request; each confirms with words.
// Approve is the primary move, Deny an outline in the bad tone; `sm` by
// default so they are easy to hit on a card, `xs` for dense rows.
export function ComputeDecide({ r, size = "sm" }: { r: ComputeRequest; size?: "xs" | "sm" }) {
  const [which, setWhich] = useState<"approve" | "deny" | null>(null);
  if (r.status !== "pending") return null;
  return (
    <span className="inline-flex flex-wrap items-center gap-1.5">
      <Button size={size} onClick={() => setWhich("approve")} aria-label={"Approve " + r.id}>
        <CheckIcon /> Approve
      </Button>
      <Button size={size} variant="outline" className="text-bad hover:text-bad" onClick={() => setWhich("deny")} aria-label={"Deny " + r.id}>
        <XIcon /> Deny
      </Button>
      <ConfirmDialog
        open={which !== null}
        onOpenChange={(o) => !o && setWhich(null)}
        title={(which === "approve" ? "Approve " : "Deny ") + r.id + "?"}
        description={
          <span>
            <Mono>{r.requested_by}</Mono> in <Mono>{r.project}</Mono> asks for <Mono>{r.size}</Mono> for {num(r.hours)} h: {r.reason}
            {which === "approve" ? " Approving lets the daemon provision the machine within the caps and charges the budget." : " Denying records your reason on the request; the agent sees it on its next loop."}
          </span>
        }
        action={which === "approve" ? "Approve" : "Deny"}
        destructive={which === "deny"}
        words={{ label: "Your words (recorded on the request)", placeholder: which === "approve" ? "approved - needed for the load test" : "not now - use the shared executor", required: which === "deny" }}
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
export function ComputeRequestCard({ r, compact }: { r: ComputeRequest; compact?: boolean }) {
  const pending = r.status === "pending";
  const decider = r.decided_by === "human" || r.decided_by === "owner" ? "you" : r.decided_by;
  return (
    <ItemCard highlight={pending} ariaLabel={r.id + " " + r.status.replace(/_/g, " ")}>
      <ItemCardHeader
        right={
          <>
            <Who row={{ agent: r.requested_by }} compact />
            <When d={r.at} />
          </>
        }
      >
        <StatusBadge status={r.status} />
        <ProjectLink slug={r.project} />
      </ItemCardHeader>
      <ItemCardTitle>
        <Mono className="text-[13px]">{r.size}</Mono> for {num(r.hours)} h
      </ItemCardTitle>
      {r.reason ? <ItemCardBody clamp={compact ? 3 : 0}>{r.reason}</ItemCardBody> : null}
      <ItemCardMeta>
        <Mono>{r.id}</Mono>
        {decider ? <span>decided by {decider}</span> : null}
        {r.executor ? (
          <span>
            executor <Mono>{r.executor}</Mono>
          </span>
        ) : null}
      </ItemCardMeta>
      {pending ? (
        <ItemCardActions>
          <ComputeDecide r={r} />
        </ItemCardActions>
      ) : null}
    </ItemCard>
  );
}

export function CapsPanel({ caps }: { caps: ComputeCaps }) {
  const auto = caps.auto_approve === true || caps.auto_approve === 1;
  return (
    <div className="grid gap-3">
      <Kv
        rows={[
          ["provider", <Mono key="p">{caps.provider || "(none)"}</Mono>],
          [
            "machines",
            <span key="m" className="inline-flex items-center gap-2">
              <span className="tnum">{num(caps.vms_now) + " of " + num(caps.max_vms)}</span>
              <Bar2 value={caps.vms_now} max={caps.max_vms} className="w-24" />
            </span>,
          ],
          [
            "hours today",
            <span key="h" className="inline-flex items-center gap-2">
              <span className="tnum">{num(caps.hours_today) + " of " + num(caps.max_hours_per_day)}</span>
              <Bar2 value={caps.hours_today} max={caps.max_hours_per_day} className="w-24" />
            </span>,
          ],
          ["budget", <span key="b" className="tnum">{num(caps.budget_credits) + " credits"}</span>],
          ["auto-approve", <ToneBadge key="a" tone={auto ? "warn" : "neutral"}>{auto ? "on - requests within caps start without you" : "off - every request waits for you"}</ToneBadge>],
        ]}
      />
      <p className="text-xs text-muted-foreground">
        Caps and the Azure target live in{" "}
        <Link className="underline" href={hrefs.settings("compute")}>
          Settings → Compute
        </Link>
        .
      </p>
    </div>
  );
}
