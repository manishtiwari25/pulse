"use client";

import { useState } from "react";
import { RefreshCwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "./confirm";
import { writes, type RestartAsk } from "@/lib/api";
import { fmtDate, rel } from "@/lib/format";

// "Restart now" while a newer PULSE waits for runs or conversations to
// finish: the owner takes the update at once instead of waiting. Runs in
// flight are asked to hold - each saves where it is and the new runner picks
// it back up with the same agent, workspace and conversation; whatever
// cannot hold within the runner's window is stopped and its job returns to
// the queue. Conversations in this console carry on by themselves under the
// new console. Shown wherever the console says an update waits.
export function RestartNow({ ask, size = "sm", variant = "outline", className }: { ask?: RestartAsk | null; size?: "xs" | "sm" | "default"; variant?: "outline" | "default" | "ghost"; className?: string }) {
  const [open, setOpen] = useState(false);
  const pending = !!ask?.pending;
  return (
    <span className={"inline-flex flex-wrap items-center gap-x-3 gap-y-1 " + (className ?? "")}>
      <Button size={size} variant={variant} onClick={() => setOpen(true)} data-testid="restart-now">
        <RefreshCwIcon /> {pending ? "Ask again" : "Restart now"}
      </Button>
      {pending && ask ? (
        <span className="text-[13px] text-muted-foreground" title={fmtDate(ask.at)}>
          Asked {rel(ask.at)}; the runner is holding its runs and restarts.
        </span>
      ) : null}
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title="Restart PULSE now?"
        description={
          <>
            Runs in progress are asked to hold: each saves where it is, and the new runner picks it back up with the same agent, workspace and conversation. Anything that can&apos;t hold within two minutes is stopped and its job goes back to the queue. Conversations in this console carry on by themselves once the new console is up. A runner older than this console may not hear the ask; then it restarts once its run finishes, as before.
          </>
        }
        action="Restart now"
        success="Asked. The runner holds its runs and restarts; this console follows if it has a newer build waiting."
        onConfirm={() => writes.restartNow()}
        invalidates={(p) => ["running", "runner", "service", "harness", "jobs", "org", "events"].some((path) => p === path || p.startsWith(path + "?") || p.startsWith(path + "/"))}
      />
    </span>
  );
}
