"use client";

import { useEffect, useRef, useState } from "react";
import { CheckIcon, CirclePowerIcon, Loader2Icon, PowerIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { showApiError } from "./confirm";
import { canApplyServiceSetup, daemonServiceState, serviceSetupAction, type ServiceSetupAction, type ServiceSetupPhase } from "./daemon-control-state";
import { TerminalAlternative } from "./evolve";
import { ItemCard, ItemCardActions, ItemCardBody, ItemCardHeader, ItemCardKind, ItemCardTitle } from "./item-card";
import { Disclosure, Notice } from "./panel";
import { waitForServiceHandover } from "./service-handover";
import { Mono } from "./who";
import { api, writes, type ServiceInstallResult, type ServicePayload } from "@/lib/api";
import { invalidate } from "@/lib/use-api";

// Keep PULSE running (ADR-038): the one machine step a shared copy needs.
// The runner (daemon) runs the agents, measures and merges; the console is
// this page. Until they are user services (launchd, systemd --user) they stop
// with the terminal that started them. The card asks once, previews what
// `pulse service install` would write, installs on a second click, and - when
// this hub is a foreground `pulse hub` on the service's port - waits for the
// service's hub to answer on the same address before it reloads.

const refreshAfterInstall = () => invalidate((p) => p === "service" || p === "running" || p === "runner" || p === "settings" || p === "org");

const ACTION_WORD: Record<ServiceSetupAction, string> = { finish: "Finish service setup", start: "Start the runner", install: "Keep PULSE running" };

export function ServiceCard({ payload }: { payload: ServicePayload }) {
  const state = daemonServiceState(payload);
  const action = serviceSetupAction(payload);
  const title = ACTION_WORD[action ?? "install"];
  return (
    <ItemCard highlight ariaLabel={title}>
      <ItemCardHeader right={<span title="This machine's service manager">{payload.service.kind}</span>}>
        <ItemCardKind icon={CirclePowerIcon} tone="you">This machine</ItemCardKind>
      </ItemCardHeader>
      <ItemCardTitle>{title}</ItemCardTitle>
      <ItemCardBody clamp={0}>
        {action === "finish" ? "The runner already runs in the background, but this console doesn't yet. Finish the setup so it keeps going after you close the terminal too." : state === "unloaded" ? "The runner's background service is installed but not loaded, so agents can't start work. Start it again." : "Set PULSE up as a background service so agents keep working after you close the terminal. It starts again when you log in."}
        {" "}You&apos;ll see exactly what changes before anything is installed.
      </ItemCardBody>
      <TerminalAlternative command={payload.command} />
      <ItemCardActions>
        <span className="text-[13px] text-muted-foreground">Adds two small files in your home folder. No password needed.</span>
        <span className="flex w-full justify-start sm:ml-auto sm:w-auto sm:justify-end">
          <ServiceControl payload={payload} size="default" />
        </span>
      </ItemCardActions>
    </ItemCard>
  );
}

export function ServiceControl({ payload, disabled = false, blockedReason = "", variant = "default", size = "sm" }: { payload: ServicePayload; disabled?: boolean; blockedReason?: string; variant?: "default" | "outline"; size?: "sm" | "default" }) {
  const [intent, setIntent] = useState<ServiceSetupAction | null>(null);
  const action = serviceSetupAction(payload);
  return (
    <>
      {action ? <Button size={size} variant={variant} disabled={disabled} onClick={() => setIntent(action)}><PowerIcon /> {ACTION_WORD[action]}</Button> : null}
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
        if (!r.dry_run || r.installed) throw new Error("PULSE didn't return a preview. Refresh the status and try again.");
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
      showApiError(new Error(blockedReason || "PULSE needs a supported service manager, a fresh status and a preview before it can set up the services."));
      return;
    }
    applying.current = true;
    setPhase("installing");
    const id = toast.loading(finishing ? "Finishing the setup…" : starting ? "Starting the runner…" : "Setting up the background services…");
    try {
      const r = await writes.serviceInstall(false);
      if (r.dry_run || !r.installed) throw new Error("The setup wasn't applied. Refresh the status and try again.");
      if (!active.current) {
        toast.dismiss(id);
        refreshAfterInstall();
        return;
      }
      setResult(r);
      if (r.handover) {
        toast.success("Set up. Waiting for the background console to answer…", { id });
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
      toast.success("Set up. PULSE now runs in the background.", { id });
      setPhase("done");
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Couldn't set up the services");
      if (active.current) setPhase("preview");
    } finally {
      applying.current = false;
    }
  };

  const url = result?.hub_url ?? payload.hub_url;
  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{ACTION_WORD[action]}</DialogTitle>
          <DialogDescription>
            {finishing ? "PULSE finishes setting up its two background services, the runner and this console, so both keep going after you close the terminal." : starting ? "PULSE loads its background service for the runner again, so agents can work and it keeps going after you close the terminal." : "PULSE installs two background services: the runner and this console, so they keep going after you close the terminal."}
            {" "}They start when you log in and restart on their own after an update.
          </DialogDescription>
        </DialogHeader>

        {phase === "preview" || phase === "installing" ? (
          <div className="grid gap-3">
            {!payload.service.supported ? (
              <Notice tone="warn">This machine has no supported background service manager (launchd on macOS, systemd on Linux). Check <Mono>pulse status</Mono> first; only if no runner is running, run <Mono>pulse daemon</Mono> in a terminal and keep it open.</Notice>
            ) : previewError ? (
              <Notice tone="warn">
                <span>Couldn&apos;t prepare the preview: {previewError}</span>
                <Button size="xs" variant="outline" className="ml-2" onClick={() => { setPreview(null); setPreviewError(""); setPreviewAttempt((n) => n + 1); }}>Try again</Button>
              </Notice>
            ) : preview ? (
              <>
                <ul className="grid gap-2 text-sm leading-relaxed">
                  {["Adds two small service files in your home folder. No password needed.", "Leaves anything that is already set up and running alone.", "Doesn't resume a pause or change any setting."].map((t) => (
                    <li key={t} className="flex gap-2.5">
                      <CheckIcon className="mt-1 size-4 shrink-0 text-ok" aria-hidden />
                      <span>{t}</span>
                    </li>
                  ))}
                </ul>
                {!payload.hub_is_service && payload.hub_port === 8765 ? (
                  <Notice tone="info">You started this console by hand. When setup finishes, the background console takes over at the same address and this page reconnects to it.</Notice>
                ) : null}
                <Disclosure summary="Show the files it writes" contentClassName="grid gap-2">
                    <p className="text-xs leading-relaxed text-muted-foreground">
                      <Mono>{preview.daemon_path}</Mono>
                      {preview.hub_path ? (
                        <>
                          {" and "}
                          <Mono>{preview.hub_path}</Mono>
                        </>
                      ) : null}
                      , each running a small launcher in your PULSE home:
                    </p>
                    <pre className="max-h-[36vh] overflow-auto rounded-lg border bg-muted/40 p-3 font-mono text-xs leading-snug anywhere">{preview.output}</pre>
                </Disclosure>
                <TerminalAlternative command={payload.command} />
              </>
            ) : (
              <div className="flex items-center gap-2 text-sm text-muted-foreground" aria-busy="true">
                <Loader2Icon className="size-4 animate-spin" /> Preparing a preview…
              </div>
            )}
            {blockedReason ? <Notice tone="warn">{blockedReason}</Notice> : disabled ? <Notice tone="warn">The runner&apos;s status changed or is refreshing. Refresh the status, then try again.</Notice> : null}
          </div>
        ) : phase === "handover" ? (
          <div className="flex items-center gap-2 text-sm" aria-busy="true">
            <Loader2Icon className="size-4 shrink-0 animate-spin" /> <span className="anywhere">Set up. Waiting for the background console at <Mono>{url}</Mono> to answer…</span>
          </div>
        ) : phase === "late" ? (
          <Notice tone="warn">
            Setup is done, but the background console hasn&apos;t answered at <Mono>{url}</Mono> yet. {handoverNote} Give it a moment, then reload. In a terminal, <Mono>pulse service status</Mono> shows both services.
          </Notice>
        ) : (
          <div className="flex items-start gap-2.5 text-sm">
            <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-ok/10 text-ok" aria-hidden>
              <CheckIcon className="size-3.5" />
            </span>
            <span className="grid gap-1">
              <span>{result?.handover ? "The background console answered. Reconnecting…" : "Done. PULSE now runs in the background. Close this to refresh the runner's status. A fresh check-in confirms it's working."}</span>
              {payload.service.logs ? <span className="text-xs text-muted-foreground anywhere">Logs: <Mono>{payload.service.logs}</Mono></span> : null}
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
                {busy ? <Loader2Icon className="animate-spin" /> : <PowerIcon />} {finishing ? "Finish setup" : starting ? "Start the runner" : "Install the services"}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
