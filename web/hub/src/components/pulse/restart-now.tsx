"use client";

import { useState } from "react";
import { RefreshCwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "./confirm";
import {
  restartActionLabel,
  restartButtonLabel,
  restartConsoleDetail,
  restartDialogLead,
  restartDialogTitle,
  restartDrainDetail,
  restartHoldCheckboxLabel,
  restartHoldDetail,
  restartHoldShown,
  restartOlderRunnerDetail,
  restartSuccessMessage,
} from "./restart-now-state";
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
  const holdShown = restartHoldShown(inflight);
  return (
    <span className={"inline-flex flex-wrap items-center gap-x-3 gap-y-1 " + (className ?? "")}>
      <Button size={size} variant={variant} onClick={() => setOpen(true)} data-testid="restart-now">
        <RefreshCwIcon /> {restartButtonLabel(pending, inflight)}
      </Button>
      {pending && ask ? (
        <span className="text-[13px] text-muted-foreground" title={fmtDate(ask.at)}>
          Asked {rel(ask.at)}; the runner restarts once its runs finish (or holds them, if asked to go now).
        </span>
      ) : null}
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={restartDialogTitle(holdNow)}
        description={
          holdShown ? (
            <>
              {restartDialogLead(inflight)}. {holdNow ? (
                <>
                  {restartHoldDetail()} {restartConsoleDetail()}
                </>
              ) : (
                <>
                  {restartDrainDetail()} {restartConsoleDetail()}
                </>
              )}
            </>
          ) : (
            <>
              {restartDialogLead(0)} {restartConsoleDetail()} {restartOlderRunnerDetail()}
            </>
          )
        }
        action={restartActionLabel(holdNow)}
        success={restartSuccessMessage(holdNow)}
        onConfirm={() => writes.restartNow(holdNow)}
        invalidates={(p) => ["running", "runner", "service", "harness", "jobs", "org", "events"].some((path) => p === path || p.startsWith(path + "?") || p.startsWith(path + "/"))}
      >
        {holdShown ? (
          <label className="flex cursor-pointer items-center gap-2 text-[13px] text-muted-foreground">
            <input type="checkbox" checked={holdNow} onChange={(e) => setHoldNow(e.target.checked)} />
            {restartHoldCheckboxLabel(inflight)}
          </label>
        ) : null}
      </ConfirmDialog>
    </span>
  );
}
