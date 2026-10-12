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
  if (!canDecide) return <span className="ml-auto text-[13px] text-muted-foreground">To answer, open PULSE on the machine that runs it.</span>;
  const yes = open === "yes";
  return (
    <>
      <span className="flex w-full flex-wrap justify-start gap-2 sm:ml-auto sm:w-auto sm:justify-end">
        <Button size={size} variant="outline" onClick={() => setOpen("no")} aria-label={noLabel + " to: " + summary}>
          <XIcon /> {noLabel}
        </Button>
        <Button size={size} onClick={() => setOpen("yes")} aria-label={yesLabel + " to: " + summary}>
          <CheckIcon /> {yesLabel}
        </Button>
      </span>
      <ConfirmDialog
        open={open !== null}
        onOpenChange={(o) => !o && setOpen(null)}
        title={yes ? yesLabel + "?" : noLabel + "?"}
        description={
          <>
            <span className="block">{summary}</span>
            {stop ? <span className="mt-2 block">{yes ? "Self-improvement carries on past this stop. The change that stopped it is not tried again as written." : "Self-improvement stays paused until you say yes."}</span> : <span className="mt-2 block">Your answer goes on the board, so the agents know what to do next.</span>}
          </>
        }
        action={yes ? yesLabel : noLabel}
        destructive={!yes}
        words={{ label: "A note for the agents (optional)", placeholder: stop ? (yes ? "Reviewed, carry on." : "Keep it paused because…") : yes ? "Yes, because…" : "No, because…" }}
        success={stop ? (yes ? "Answered. Self-improvement carries on." : "Answered. Self-improvement stays paused.") : yes ? "You said yes. The agents will act on it." : "You said no. The agents won't go ahead."}
        onConfirm={(words) => writes.decide(id, yes ? "yes" : "no", words)}
      />
    </>
  );
}
