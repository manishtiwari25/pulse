"use client";

import { useState } from "react";
import { CheckIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "./confirm";
import { writes } from "@/lib/api";

export function DecideButtons({
  id,
  summary,
  stop,
  canDecide = true,
  size = "default",
  yesLabel = "Yes",
  noLabel = "No",
}: {
  id: string;
  summary: string;
  stop?: boolean;
  canDecide?: boolean;
  size?: "xs" | "sm" | "default";
  yesLabel?: string;
  noLabel?: string;
}) {
  const [open, setOpen] = useState<"yes" | "no" | null>(null);
  if (!canDecide) return <span className="ml-auto text-xs text-muted-foreground">read-only: open the hub on the machine that runs it to decide</span>;
  const yes = open === "yes";
  return (
    <>
      <span className="flex w-full flex-wrap justify-start gap-2 sm:ml-auto sm:w-auto sm:justify-end">
        <Button size={size} variant="outline" onClick={() => setOpen("no")} aria-label={noLabel + " to: " + summary}>
          <XIcon /> {noLabel}
        </Button>
        <Button size={size} className="glossy" onClick={() => setOpen("yes")} aria-label={yesLabel + " to: " + summary}>
          <CheckIcon /> {yesLabel}
        </Button>
      </span>
      <ConfirmDialog
        open={open !== null}
        onOpenChange={(o) => !o && setOpen(null)}
        title={yes ? "Say yes to this?" : "Say no to this?"}
        description={
          <>
            <span className="block">{summary}</span>
            {stop ? <span className="mt-2 block text-xs">{yes ? "The loop plans past this stop. The card that stopped it is not retried as written." : "The loop stays stopped until you say yes."}</span> : <span className="mt-2 block text-xs">Your answer is recorded on the board so the agents know what to do next.</span>}
          </>
        }
        action={yes ? yesLabel : noLabel}
        destructive={!yes}
        words={{ label: "A note for the agents (optional)", placeholder: stop ? (yes ? "Reviewed - continue past this stop." : "Keep it stopped because …") : yes ? "Yes because …" : "No because …" }}
        success={stop ? "Recorded - the board closes the stop question." : yes ? "Decided yes - the board records your answer; a job is queued to act on it." : "Decided no - the board records your answer."}
        onConfirm={(words) => writes.decide(id, yes ? "yes" : "no", words)}
      />
    </>
  );
}
