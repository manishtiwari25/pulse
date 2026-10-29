"use client";

import { useState } from "react";
import { RefreshCwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "./confirm";
import { writes, type RestartAsk } from "@/lib/api";
import { fmtDate, rel } from "@/lib/format";

// Restart the daemon. The default drains like a self-update: nothing new
// starts and it swaps in the new build once the runs in flight have ended
// (up to daemon.restart_drain_minutes), so nothing is held or lost. With
// runs in flight the button says how many, and the dialog offers that drain
// as the default; the operator who wants the swap now picks "Hold them now"
// - each run saves where it is and carries on under the new build, and what
// cannot hold in the runner's window is stopped and returns to the queue.
export function RestartNow({ ask, inflight = 0, size = "sm", variant = "outline", className }: { ask?: RestartAsk | null; inflight?: number; size?: "xs" | "sm" | "default"; variant?: "outline" | "default" | "ghost"; className?: string }) {
  const [open, setOpen] = useState(false);
  const [holdNow, setHoldNow] = useState(false);
  const pending = !!ask?.pending;
  const runs = inflight > 0 ? `${inflight} run${inflight === 1 ? "" : "s"} in flight` : null;
  return (
    <span className={"inline-flex flex-wrap items-center gap-x-3 gap-y-1 " + (className ?? "")}>
      <Button size={size} variant={variant} onClick={() => setOpen(true)} data-testid="restart-now">
        <RefreshCwIcon /> {pending ? "Ask again" : runs ? `Restart · ${runs}` : "Restart now"}
      </Button>
      {pending && ask ? (
        <span className="text-[13px] text-muted-foreground" title={fmtDate(ask.at)}>
          Asked {rel(ask.at)}; the runner restarts once its runs finish (or holds them, if asked to go now).
        </span>
      ) : null}
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={holdNow ? "Restart PULSE now, holding its runs?" : "Restart PULSE?"}
        description={
          runs ? (
            <>
              {runs.charAt(0).toUpperCase() + runs.slice(1)}. {holdNow ? (
                <>
                  Each is asked to hold: it saves where it is, and the new runner picks it back up with the same agent, workspace and conversation. Anything that can&apos;t hold within two minutes is stopped and its job goes back to the queue. Conversations in this console carry on by themselves once the new console is up.
                </>
              ) : (
                <>
                  The runner stops starting runs and restarts once they end (up to the drain budget), so nothing is held or lost. Conversations in this console carry on by themselves once the new console is up.
                </>
              )}
            </>
          ) : (
            <>
              Nothing is in flight, so the runner swaps in the new build at once. Conversations in this console carry on by themselves once the new console is up. A runner older than this console may not hear the ask; then it restarts once its run finishes, as before.
            </>
          )
        }
        action={holdNow ? "Hold them now" : "Restart when they finish"}
        success={holdNow ? "Asked. The runner holds its runs and restarts now." : "Asked. The runner starts nothing new and restarts once its runs finish."}
        onConfirm={() => writes.restartNow(holdNow)}
        invalidates={(p) => ["running", "runner", "service", "harness", "jobs", "org", "events"].some((path) => p === path || p.startsWith(path + "?") || p.startsWith(path + "/"))}
      >
        {runs ? (
          <label className="flex cursor-pointer items-center gap-2 text-[13px] text-muted-foreground">
            <input type="checkbox" checked={holdNow} onChange={(e) => setHoldNow(e.target.checked)} />
            Hold the {inflight} run{inflight === 1 ? "" : "s"} and restart now instead
          </label>
        ) : null}
      </ConfirmDialog>
    </span>
  );
}
