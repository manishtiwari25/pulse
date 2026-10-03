"use client";

import { useState } from "react";
import { CheckIcon, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "./confirm";
import { writes } from "@/lib/api";

// The Yes/No pair with its confirmation, shared by the question card and the
// self-improvement panel. `stop` picks the copy for one of the evolution
// loop's stop questions: a yes records the review and the loop plans past
// the stop; a no is recorded too, and an instruction-shaped post stays
// stopped until its thread is closed.
export function DecideButtons({ id, summary, stop, canDecide = true, size = "sm" }: { id: string; summary: string; stop?: boolean; canDecide?: boolean; size?: "xs" | "sm" }) {
  const [open, setOpen] = useState<"yes" | "no" | null>(null);
  if (!canDecide) return <span className="ml-auto text-xs text-muted-foreground">read-only: open the hub on the machine that runs it to decide</span>;
  const yes = open === "yes";
  return (
    <>
      <span className="ml-auto flex shrink-0 gap-2">
        <Button size={size} variant="outline" className="text-bad" onClick={() => setOpen("no")} aria-label={"No to: " + summary}>
          <XIcon /> No
        </Button>
        <Button size={size} onClick={() => setOpen("yes")} aria-label={"Yes to: " + summary}>
          <CheckIcon /> Yes
        </Button>
      </span>
      <ConfirmDialog
        open={open !== null}
        onOpenChange={(o) => !o && setOpen(null)}
        title={stop ? (yes ? "Reviewed - let the loop plan past this stop?" : "Say no to this stop question?") : yes ? "Answer yes to this question?" : "Answer no to this question?"}
        description={
          <>
            <span className="block">{summary}</span>
            {stop ? <span className="mt-2 block text-xs">{yes ? "Your yes records the review; the loop plans past this stop on its next tick." : "Your no is recorded. An instruction-shaped post stays stopped until its thread is closed; other review gates count your answer as the review."}</span> : null}
          </>
        }
        action={yes ? "Yes" : "No"}
        destructive={!yes}
        words={{ label: "Your words", placeholder: stop ? (yes ? "e.g. reviewed - that post is not a command to agents" : "e.g. the target really is out of scope - do not retry as written") : yes ? "e.g. yes - go with the first option" : "e.g. no - leave it as it is", required: true }}
        success={stop ? "Recorded - the board closes the stop question." : yes ? "Decided yes - the board records your answer; a job is queued to act on it." : "Decided no - the board records your answer."}
        onConfirm={(words) => writes.decide(id, yes ? "yes" : "no", words)}
      />
    </>
  );
}
