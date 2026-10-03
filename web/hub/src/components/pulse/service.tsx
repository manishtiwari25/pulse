"use client";

import { useEffect, useRef, useState } from "react";
import { CheckIcon, Loader2Icon, PowerIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { showApiError } from "./confirm";
import { canApplyServiceSetup, daemonServiceState, serviceSetupAction, type ServiceSetupAction, type ServiceSetupPhase } from "./daemon-control-state";
import { TerminalAlternative } from "./evolve";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardTitle } from "./item-card";
import { Notice } from "./panel";
import { waitForServiceHandover } from "./service-handover";
import { Mono, ToneBadge } from "./who";
import { api, writes, type ServiceInstallResult, type ServicePayload } from "@/lib/api";
import { invalidate } from "@/lib/use-api";

// Keep PULSE running (ADR-038): the one machine step a shared copy needs.
// The daemon runs the agents, measures and lands; the console is this page.
// Until they are user services (launchd, systemd --user) they stop with the
// terminal that started them. The card asks once, previews what `pulse
// service install` would write, installs on a second click, and - when this
// hub is a foreground `pulse hub` on the service's port - waits for the
// service's hub to answer on the same address before it reloads.

const refreshAfterInstall = () => invalidate((p) => p === "service" || p === "running" || p === "runner" || p === "settings" || p === "org");

export function ServiceCard({ payload }: { payload: ServicePayload }) {
  const state = daemonServiceState(payload);
  const starting = state === "unloaded";
  const finishing = serviceSetupAction(payload) === "finish";
  const title = finishing ? "Finish service setup" : starting ? "Start daemon" : "Keep PULSE running";
  return (
    <ItemCard highlight ariaLabel={title}>
      <ItemCardHeader right={<ToneBadge tone="neutral">{payload.service.kind}</ToneBadge>}>
        <ToneBadge tone="accent">this machine</ToneBadge>
        <ToneBadge tone={state === "loaded" && !finishing ? "neutral" : "warn"}>{finishing ? "console setup needed" : state === "unloaded" ? "not loaded" : state === "loaded" ? "service loaded" : state === "missing" ? "not installed" : state}</ToneBadge>
      </ItemCardHeader>
      <ItemCardTitle>{title}</ItemCardTitle>
      <ItemCardBody clamp={5}>
        {finishing ? "The daemon's service is present and loaded. Preview and apply the existing installer to finish the console's missing, unloaded or unconfirmed service setup." : starting ? "The daemon's service file exists, but it is not loaded. Preview and apply the existing installer to load it again." : "User services keep the daemon and this console available after you close a terminal. They start at login and come back after a rebuild."}
        {" "}Setup leaves unchanged, loaded units alone. It does not clear the owner pause or any STOP file, and a fresh heartbeat still needs to confirm progress.
      </ItemCardBody>
      <TerminalAlternative command={payload.command} />
      <ItemCardActions>
        <span className="text-xs text-muted-foreground">writes two small files under your home; nothing needs a password</span>
        <span className="ml-auto shrink-0">
          <ServiceControl payload={payload} />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}

export function ServiceControl({ payload, disabled = false, blockedReason = "" }: { payload: ServicePayload; disabled?: boolean; blockedReason?: string }) {
  const [intent, setIntent] = useState<ServiceSetupAction | null>(null);
  const action = serviceSetupAction(payload);
  return (
    <>
      {action ? <Button size="sm" variant="outline" disabled={disabled} onClick={() => setIntent(action)}><PowerIcon /> {action === "finish" ? "Finish service setup" : action === "start" ? "Start" : "Keep running at login"}</Button> : null}
      <ServiceDialog open={intent !== null} onOpenChange={(o) => !o && setIntent(null)} payload={payload} action={intent ?? "install"} disabled={disabled} blockedReason={blockedReason} />
    </>
  );
}

export function ServiceDialog({ open, onOpenChange, payload, action = "install", disabled = false, blockedReason = "" }: { open: boolean; onOpenChange: (o: boolean) => void; payload: ServicePayload; action?: ServiceSetupAction; disabled?: boolean; blockedReason?: string }) {
  const [phase, setPhase] = useState<ServiceSetupPhase>("preview");
  const [preview, setPreview] = useState<ServiceInstallResult | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [previewAttempt, setPreviewAttempt] = useState(0);
  const [result, setResult] = useState<ServiceInstallResult | null>(null);
  const [handoverNote, setHandoverNote] = useState("");
  const busy = phase === "installing" || phase === "handover";
  const canApply = canApplyServiceSetup({ phase, supported: payload.service.supported, preview, previewError, disabled, blockedReason });
  const timer = useRef<number | null>(null);
  const cancelHandover = useRef<(() => void) | null>(null);
  const active = useRef(false);
  const applying = useRef(false);
  const starting = action === "start";
  const finishing = action === "finish";

  // The preview is `pulse service install --dry-run`: the launchers and units as text, written nowhere.
  useEffect(() => {
    if (!open || !payload.service.supported) return;
    let cancelled = false;
    writes
      .serviceInstall(true)
      .then((r) => {
        if (!r.dry_run || r.installed) throw new Error("The service API did not return a dry-run preview. Refresh status before trying again.");
        if (!cancelled) setPreview(r);
      })
      .catch((e: unknown) => {
        if (!cancelled) setPreviewError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [open, payload.service.supported, previewAttempt]);

  useEffect(() => {
    active.current = open;
    return () => {
      active.current = false;
      if (timer.current) window.clearTimeout(timer.current);
      cancelHandover.current?.();
    };
  }, [open]);

  // Refresh after closing, including failed applies: the installer may have written only some files.
  const close = (o: boolean) => {
    if (busy) return;
    if (!o) {
      setPhase("preview");
      setResult(null);
      setPreview(null);
      setPreviewError("");
      setHandoverNote("");
      refreshAfterInstall();
    }
    onOpenChange(o);
  };

  const install = async () => {
    if (applying.current) return;
    if (!canApply) {
      showApiError(new Error(blockedReason || "A supported service manager, current status and successful preview are required before applying setup."));
      return;
    }
    applying.current = true;
    setPhase("installing");
    const id = toast.loading(finishing ? "Completing service setup..." : starting ? "Loading the services..." : "Installing the services...");
    try {
      const r = await writes.serviceInstall(false);
      if (r.dry_run || !r.installed) throw new Error("The service setup was not applied. Refresh status and preview it again.");
      if (!active.current) {
        toast.dismiss(id);
        refreshAfterInstall();
        return;
      }
      setResult(r);
      if (r.handover) {
        toast.success("Service setup applied; waiting for the service's console", { id });
        setPhase("handover");
        cancelHandover.current = waitForServiceHandover({
          load: (signal) => api<ServicePayload>("service", { signal }),
          onReady: () => {
            setPhase("done");
            timer.current = window.setTimeout(() => window.location.reload(), 400);
          },
          onLate: (note) => {
            setHandoverNote(note);
            setPhase("late");
          },
        });
        return;
      }
      toast.success("Service setup applied. Close to refresh the daemon's status.", { id });
      setPhase("done");
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not install the services");
      if (active.current) setPhase("preview");
    } finally {
      applying.current = false;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{finishing ? "Finish service setup" : starting ? "Start daemon" : "Keep PULSE running"}</DialogTitle>
          <DialogDescription>
            Two user services under {payload.service.kind}: the daemon and this console. They start at login, restart if they stop, and pick up each rebuild. The same as <Mono>{payload.command}</Mono>.
            {" "}Unchanged, loaded units are left alone. Setup does not clear the owner pause or any STOP file, and does not prove a fresh daemon heartbeat.
          </DialogDescription>
        </DialogHeader>

        {phase === "preview" || phase === "installing" ? (
          <div className="grid gap-3">
            {!payload.service.supported ? (
              <Notice tone="warn">No supported service manager is available. Run <Mono>pulse daemon</Mono> in a terminal only after confirming no daemon is already running.</Notice>
            ) : previewError ? (
              <Notice tone="warn">
                Could not preview: {previewError}
                <Button size="xs" variant="outline" className="ml-2" onClick={() => { setPreview(null); setPreviewError(""); setPreviewAttempt((n) => n + 1); }}>Retry preview</Button>
              </Notice>
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
            {blockedReason ? <Notice tone="warn">{blockedReason}</Notice> : disabled ? <Notice tone="warn">Service or runner status changed or is being refreshed. Refresh status before applying setup.</Notice> : null}
          </div>
        ) : phase === "handover" ? (
          <div className="flex items-center gap-2 text-sm" aria-busy="true">
            <Loader2Icon className="size-4 animate-spin" /> Setup applied. Waiting for the service&apos;s console at <Mono>{result?.hub_url ?? payload.hub_url}</Mono>…
          </div>
        ) : phase === "late" ? (
          <Notice tone="warn">
            Setup was applied, but the service&apos;s console has not confirmed the handover at <Mono>{result?.hub_url ?? payload.hub_url}</Mono>. {handoverNote} Give it a moment and reload; <Mono>pulse service status</Mono> in a terminal shows both units.
          </Notice>
        ) : (
          <div className="flex items-center gap-2 text-sm">
            <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-ok/10 text-ok" aria-hidden>
              <CheckIcon className="size-3.5" />
            </span>
            <span>
              {result?.handover ? "The service's console answered. Reconnecting to read its current daemon status." : "Service setup applied. Close to refresh the heartbeat and owner pause; neither a saved setting nor service setup proves the daemon is making progress."}
              {" "}<Mono>{payload.service.logs}</Mono> holds the logs.
            </span>
          </div>
        )}

        <DialogFooter>
          {phase === "done" && !result?.handover ? (
            <Button onClick={() => close(false)}>Done</Button>
          ) : phase === "late" || phase === "done" ? (
            <Button onClick={() => window.location.reload()}>Reload</Button>
          ) : (
            <>
              <Button variant="ghost" onClick={() => close(false)} disabled={busy}>
                Not now
              </Button>
              <Button onClick={() => void install()} disabled={!canApply}>
                {busy ? <Loader2Icon className="animate-spin" /> : <PowerIcon />} {finishing ? "Apply setup for console" : starting ? "Apply setup and start daemon" : "Install and keep running"}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
