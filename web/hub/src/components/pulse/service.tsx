"use client";

import { useEffect, useRef, useState } from "react";
import { CheckIcon, Loader2Icon, PowerIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { showApiError } from "./confirm";
import { TerminalAlternative } from "./evolve";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardTitle } from "./item-card";
import { Notice } from "./panel";
import { Mono, ToneBadge } from "./who";
import { api, writes, type Org, type ServiceInstallResult, type ServicePayload } from "@/lib/api";
import { invalidate } from "@/lib/use-api";

// Keep PULSE running (ADR-038): the one machine step a shared copy needs.
// The daemon runs the agents, measures and lands; the console is this page.
// Until they are user services (launchd, systemd --user) they stop with the
// terminal that started them. The card asks once, previews what `pulse
// service install` would write, installs on a second click, and - when this
// hub is a foreground `pulse hub` on the service's port - waits for the
// service's hub to answer on the same address before it reloads.

const refreshAfterInstall = () => invalidate((p) => p === "service" || p === "running" || p === "runner" || p === "org");

// How long the page waits for the service's hub after a handover before it says so.
const HANDOVER_WAIT_MS = 45000;
const HANDOVER_POLL_MS = 1000;

type Phase = "preview" | "installing" | "handover" | "done" | "late";

export function ServiceCard({ payload }: { payload: ServicePayload }) {
  const [open, setOpen] = useState(false);
  return (
    <ItemCard highlight ariaLabel="Keep PULSE running">
      <ItemCardHeader right={<ToneBadge tone="neutral">{payload.service.kind}</ToneBadge>}>
        <ToneBadge tone="accent">this machine</ToneBadge>
        <ToneBadge tone="warn">not kept alive</ToneBadge>
      </ItemCardHeader>
      <ItemCardTitle>Keep PULSE running</ItemCardTitle>
      <ItemCardBody clamp={5}>
        The daemon runs your agents, measures each cycle and lands what passes; this console is how you watch and decide. Right now both stop when the terminal that started them closes. Install them once as user services: they start
        when you log in and come back on their own after a rebuild.
      </ItemCardBody>
      <TerminalAlternative command={payload.command} />
      <ItemCardActions>
        <span className="text-xs text-muted-foreground">writes two small files under your home; nothing needs a password</span>
        <span className="ml-auto shrink-0">
          <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
            <PowerIcon /> Keep running
          </Button>
        </span>
      </ItemCardActions>
      <ServiceDialog open={open} onOpenChange={setOpen} payload={payload} />
    </ItemCard>
  );
}

export function ServiceDialog({ open, onOpenChange, payload }: { open: boolean; onOpenChange: (o: boolean) => void; payload: ServicePayload }) {
  const [phase, setPhase] = useState<Phase>("preview");
  const [preview, setPreview] = useState<ServiceInstallResult | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [result, setResult] = useState<ServiceInstallResult | null>(null);
  const busy = phase === "installing" || phase === "handover";
  const timer = useRef<number | null>(null);

  // The preview is `pulse service install --dry-run`: the launchers and units as text, written nowhere.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    writes
      .serviceInstall(true)
      .then((r) => {
        if (!cancelled) setPreview(r);
      })
      .catch((e: unknown) => {
        if (!cancelled) setPreviewError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  useEffect(
    () => () => {
      if (timer.current) window.clearTimeout(timer.current);
    },
    [],
  );

  // The card leaves the inbox once `service` says installed, and the dialog with it: refresh on close, not before.
  const close = (o: boolean) => {
    if (busy) return;
    if (!o) {
      const installed = phase === "done" || phase === "late";
      setPhase("preview");
      setResult(null);
      setPreview(null);
      setPreviewError("");
      if (installed) refreshAfterInstall();
    }
    onOpenChange(o);
  };

  // After a handover the old hub has left the port: poll until a hub answers again, then reload so the page reads the service's.
  const awaitHandover = (started: number) => {
    api<Org>("org")
      .then(() => {
        setPhase("done");
        window.setTimeout(() => window.location.reload(), 400);
      })
      .catch(() => {
        if (Date.now() - started > HANDOVER_WAIT_MS) {
          setPhase("late");
          return;
        }
        timer.current = window.setTimeout(() => awaitHandover(started), HANDOVER_POLL_MS);
      });
  };

  const install = async () => {
    setPhase("installing");
    const id = toast.loading("Installing the services…");
    try {
      const r = await writes.serviceInstall(false);
      setResult(r);
      if (r.handover) {
        toast.success("Installed · this console hands over to the service's", { id });
        setPhase("handover");
        // Give the old hub its moment to stop listening before the first poll.
        timer.current = window.setTimeout(() => awaitHandover(Date.now()), 1500);
        return;
      }
      toast.success("Installed · the daemon and the console are kept alive", { id });
      setPhase("done");
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not install the services");
      setPhase("preview");
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="max-h-[90vh] overflow-y-auto rounded-xl sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Keep PULSE running</DialogTitle>
          <DialogDescription>
            Two user services under {payload.service.kind}: the daemon and this console. They start at login, restart if they stop, and pick up each rebuild. The same as <Mono>{payload.command}</Mono>.
          </DialogDescription>
        </DialogHeader>

        {phase === "preview" || phase === "installing" ? (
          <div className="grid gap-3">
            {previewError ? (
              <Notice tone="warn">Could not preview: {previewError}</Notice>
            ) : preview ? (
              <>
                <p className="text-xs text-muted-foreground">
                  What it writes - <Mono>{preview.daemon_path}</Mono>
                  {preview.hub_path ? (
                    <>
                      {" and "}
                      <Mono>{preview.hub_path}</Mono>
                    </>
                  ) : null}
                  , each running a small launcher under your PULSE home:
                </p>
                <pre className="max-h-[40vh] overflow-auto rounded-md border bg-muted/40 p-3 text-[11.5px] leading-snug anywhere">{preview.output}</pre>
                {!payload.hub_is_service && payload.hub_port === 8765 ? (
                  <Notice tone="info">
                    This console is a <Mono>pulse hub</Mono> you started by hand on the service&apos;s port. When the install finishes it steps aside, the service&apos;s console takes over at the same address, and this page reconnects to it.
                  </Notice>
                ) : null}
              </>
            ) : (
              <div className="flex items-center gap-2 text-sm text-muted-foreground" aria-busy="true">
                <Loader2Icon className="size-4 animate-spin" /> previewing…
              </div>
            )}
          </div>
        ) : phase === "handover" ? (
          <div className="flex items-center gap-2 text-sm" aria-busy="true">
            <Loader2Icon className="size-4 animate-spin" /> Installed. Waiting for the service&apos;s console at <Mono>{result?.hub_url ?? payload.hub_url}</Mono>…
          </div>
        ) : phase === "late" ? (
          <Notice tone="warn">
            The services are installed, but no console answered at <Mono>{result?.hub_url ?? payload.hub_url}</Mono> yet. Give it a moment and reload; <Mono>pulse service status</Mono> in a terminal shows both units.
          </Notice>
        ) : (
          <div className="flex items-center gap-2 text-sm">
            <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-ok/10 text-ok" aria-hidden>
              <CheckIcon className="size-3.5" />
            </span>
            Installed. The daemon and the console are user services now; <Mono>{payload.service.logs}</Mono> holds their logs.
          </div>
        )}

        <DialogFooter>
          {phase === "done" && !result?.handover ? (
            <Button onClick={() => close(false)}>Done</Button>
          ) : phase === "late" ? (
            <Button onClick={() => window.location.reload()}>Reload</Button>
          ) : (
            <>
              <Button variant="ghost" onClick={() => close(false)} disabled={busy}>
                Not now
              </Button>
              <Button onClick={() => void install()} disabled={busy || !preview}>
                {busy ? <Loader2Icon className="animate-spin" /> : <PowerIcon />} Install and keep running
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
