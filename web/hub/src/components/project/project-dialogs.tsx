"use client";

import { useEffect, useState } from "react";
import { Loader2Icon, PlusIcon, RotateCcwIcon, Trash2Icon } from "lucide-react";
import { toast } from "sonner";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { showApiError } from "@/components/pulse/confirm";
import { Notice } from "@/components/pulse/panel";
import { Mono } from "@/components/pulse/who";
import { errorText, writes, type InitResult, type RemovePreview } from "@/lib/api";
import { invalidate } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// The project lifecycle from the console (ADR-027): register a checkout
// (`pulse init PATH`), remove a project or an executor after a preview
// (`pulse remove … --dry-run`, then for real), and restore an inactive
// project. Every write goes through the hub; nothing here touches disk.

type Size = "xs" | "sm" | "default";
type Variant = "default" | "outline" | "ghost";

// ----- add project -------------------------------------------------------

export function AddProjectButton({ size = "default", variant = "default", onRegistered }: { size?: Size; variant?: Variant; onRegistered?: (r: InitResult) => void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size={size} variant={variant} onClick={() => setOpen(true)}>
        <PlusIcon /> Add project
      </Button>
      <AddProjectDialog open={open} onOpenChange={setOpen} onRegistered={onRegistered} />
    </>
  );
}

const pathLooksAbsolute = (p: string) => p.startsWith("/") || p.startsWith("~");

export function AddProjectDialog({ open, onOpenChange, onRegistered }: { open: boolean; onOpenChange: (o: boolean) => void; onRegistered?: (r: InitResult) => void }) {
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [verify, setVerify] = useState("");
  const [busy, setBusy] = useState(false);
  const trimmed = path.trim();
  const valid = trimmed.length > 0 && pathLooksAbsolute(trimmed);

  const submit = async () => {
    setBusy(true);
    const id = toast.loading("Registering " + trimmed + "…");
    try {
      const r = await writes.registerProject({ path: trimmed, name: name.trim() || undefined, verify: verify.trim() || undefined });
      const job = r.job ? " · onboarding job " + r.job.id + (r.job_queued ? " queued" : " already queued") : "";
      toast.success(r.project.slug + (r.created ? " registered" : " refreshed") + job, { id });
      invalidate((p) => p === "org" || p === "projects" || p.startsWith("events") || p.startsWith("projects/") || p.startsWith("tasks"));
      onOpenChange(false);
      setPath("");
      setName("");
      setVerify("");
      onRegistered?.(r);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not add the project");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Add project</DialogTitle>
          <DialogDescription>
            Register a git checkout on this machine so the hub lists it, its lead can work in it and you can give it tasks. The same as <Mono>pulse init</Mono> in that folder.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-3">
          <div className="grid gap-1.5">
            <Label htmlFor="ap-path">Checkout path</Label>
            <Input
              id="ap-path"
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder="/Users/you/src/my-app"
              className="font-mono text-[12.5px]"
              autoFocus
              spellCheck={false}
              onKeyDown={(e) => {
                if (e.key === "Enter" && valid && !busy) void submit();
              }}
            />
            <p className="text-xs text-muted-foreground">An absolute path to a git checkout on this machine. ~ is allowed.</p>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="grid gap-1.5">
              <Label htmlFor="ap-name">Name (optional)</Label>
              <Input id="ap-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="my-app" maxLength={100} />
              <p className="text-xs text-muted-foreground">Defaults to the folder name; the slug is derived from it.</p>
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="ap-verify">Verify command (optional)</Label>
              <Input id="ap-verify" value={verify} onChange={(e) => setVerify(e.target.value)} placeholder="go test ./... or npm test" className="font-mono text-[12.5px]" spellCheck={false} />
              <p className="text-xs text-muted-foreground">The command the runner uses to check a job before it lands.</p>
            </div>
          </div>
          <div className="rounded-md border bg-muted/30 px-3 py-2">
            <div className="mb-1 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">What happens</div>
            <ul className="grid gap-1 text-[12.5px] leading-snug text-muted-foreground">
              <li className="flex gap-2">
                <span aria-hidden>•</span>
                <span>Registers the checkout as a project (or refreshes it when the path is already known)</span>
              </li>
              <li className="flex gap-2">
                <span aria-hidden>•</span>
                <span>
                  Imports <Mono>docs/context/environment.md</Mono> as the project&apos;s environment recipe when the checkout has one
                </span>
              </li>
              <li className="flex gap-2">
                <span aria-hidden>•</span>
                <span>Queues the onboarding job - the runner picks it up when it is on</span>
              </li>
            </ul>
          </div>
        </div>
        <DialogFooter className="items-center gap-2">
          {trimmed && !valid ? <span className="mr-auto text-xs text-warn">The path must start with / or ~.</span> : null}
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void submit()} disabled={busy || !valid}>
            {busy ? <Loader2Icon className="animate-spin" /> : <PlusIcon />}
            Add project
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ----- remove (project or executor) ---------------------------------------

type RemoveKind = "project" | "executor";

const REMOVE_TEXT: Record<RemoveKind, { title: string; description: string; action: string; done: string }> = {
  project: {
    title: "Remove project",
    description: "Nothing is deleted. The project turns inactive: its queued jobs are cancelled, executors that named it stop serving it, and its posts, runs and checkout stay. You can restore it later.",
    action: "Remove project",
    done: "restore it from the project picker any time.",
  },
  executor: {
    title: "Remove executor",
    description: "The hub forgets this machine: its row turns removed and no job is sent there again. Runs it made keep its name. The machine itself is untouched.",
    action: "Remove executor",
    done: "the machine is untouched.",
  },
};

const removeWrite = (kind: RemoveKind, name: string, dryRun: boolean) => (kind === "project" ? writes.removeProject(name, dryRun) : writes.removeExecutor(name, dryRun));

export function RemoveDialog({ open, onOpenChange, kind, name, onApplied }: { open: boolean; onOpenChange: (o: boolean) => void; kind: RemoveKind; name: string; onApplied?: (p: RemovePreview) => void }) {
  // The body holds the preview and the busy flag; it mounts with the content,
  // so each opening starts from a fresh preview and the fetch runs once.
  const [busy, setBusy] = useState(false);
  return (
    <AlertDialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <AlertDialogContent size="default" className="max-h-[90vh] overflow-y-auto sm:max-w-lg">
        <RemoveBody kind={kind} name={name} busy={busy} setBusy={setBusy} onOpenChange={onOpenChange} onApplied={onApplied} />
      </AlertDialogContent>
    </AlertDialog>
  );
}

function RemoveBody({ kind, name, busy, setBusy, onOpenChange, onApplied }: { kind: RemoveKind; name: string; busy: boolean; setBusy: (b: boolean) => void; onOpenChange: (o: boolean) => void; onApplied?: (p: RemovePreview) => void }) {
  const text = REMOVE_TEXT[kind];
  const [preview, setPreview] = useState<RemovePreview | null>(null);
  const [refusal, setRefusal] = useState<{ error: string; why?: string; fix?: string } | null>(null);

  // Preview first: the same write with dry_run so the owner reads what changes
  // and what stays before the destructive button does anything.
  useEffect(() => {
    let alive = true;
    removeWrite(kind, name, true)
      .then((p) => {
        if (alive) setPreview(p);
      })
      .catch((e: unknown) => {
        if (alive) setRefusal(errorText(e));
      });
    return () => {
      alive = false;
    };
  }, [kind, name]);

  const apply = async () => {
    setBusy(true);
    const id = toast.loading("Removing " + name + "…");
    try {
      const p = await removeWrite(kind, name, false);
      toast.success(name + " removed - " + text.done, { id });
      invalidate();
      onOpenChange(false);
      onApplied?.(p);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not remove " + name);
    } finally {
      setBusy(false);
    }
  };

  const leased = preview?.leased_jobs?.length ?? 0;
  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle>
          {text.title + " "}
          <Mono className="text-[15px]">{name}</Mono>
        </AlertDialogTitle>
        <AlertDialogDescription className="anywhere">{text.description}</AlertDialogDescription>
      </AlertDialogHeader>
      {refusal ? (
        <Notice tone="bad" className="text-[13px]">
          <div className="font-medium">{refusal.error || "The hub refused the preview."}</div>
          {refusal.why ? <div className="mt-0.5 text-muted-foreground">why: {refusal.why}</div> : null}
          {refusal.fix ? <div className="mt-0.5 text-muted-foreground">fix: {refusal.fix}</div> : null}
        </Notice>
      ) : preview ? (
        <div className="grid gap-3" aria-live="polite">
          <PreviewList title="Changes" items={preview.changes} empty="Nothing changes." />
          <PreviewList title="Stays" items={preview.stays} empty="Nothing else is touched." />
          {preview.yours?.length ? (
            <section>
              <PreviewTitle>Yours to run</PreviewTitle>
              <ul className="grid gap-1">
                {preview.yours.map((cmd, i) => (
                  <li key={i}>
                    <code className="block rounded bg-muted px-2 py-1 font-mono text-[11.5px] anywhere">{cmd}</code>
                  </li>
                ))}
              </ul>
              <p className="mt-1 text-[11.5px] text-muted-foreground">Commands the hub does not run for you; run them yourself if they apply.</p>
            </section>
          ) : null}
          {leased ? <p className="text-xs text-warn">{leased + " leased job" + (leased === 1 ? " is" : "s are") + " left to finish; nothing new is leased."}</p> : null}
        </div>
      ) : (
        <div className="grid gap-2" aria-busy="true" aria-label="Loading the preview">
          <Skeleton className="h-4 w-24" />
          <Skeleton className="h-8" />
          <Skeleton className="h-4 w-20" />
          <Skeleton className="h-8" />
        </div>
      )}
      <AlertDialogFooter>
        <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
        <AlertDialogAction
          disabled={busy || !preview}
          onClick={(e) => {
            e.preventDefault();
            void apply();
          }}
          className={cn("bg-destructive text-white hover:bg-destructive/90")}
        >
          {busy ? <Loader2Icon className="animate-spin" /> : <Trash2Icon />}
          {text.action}
        </AlertDialogAction>
      </AlertDialogFooter>
    </>
  );
}

function PreviewTitle({ children }: { children: React.ReactNode }) {
  return <div className="mb-1 text-[10.5px] font-medium uppercase tracking-wide text-muted-foreground">{children}</div>;
}

function PreviewList({ title, items, empty }: { title: string; items: RemovePreview["changes"]; empty: string }) {
  return (
    <section>
      <PreviewTitle>{title}</PreviewTitle>
      {items?.length ? (
        <ul className="grid gap-1 text-[12.5px] leading-snug">
          {items.map((it, i) => (
            <li key={i} className="flex flex-wrap gap-x-1.5 anywhere">
              <Mono>{it.label}</Mono>
              {it.detail ? <span className="text-muted-foreground">{"- " + it.detail}</span> : null}
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-[12.5px] text-muted-foreground">{empty}</p>
      )}
    </section>
  );
}

// "Remove…" in a project header or at the end of an executor row. Executor
// rows are one line, so there it is an icon button with a tooltip.
export function RemoveMenuButton({ kind, name, onApplied, compact }: { kind: RemoveKind; name: string; onApplied?: (p: RemovePreview) => void; compact?: boolean }) {
  const [open, setOpen] = useState(false);
  const label = "Remove " + kind + " " + name;
  return (
    <>
      {compact ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="ghost" size="icon-xs" aria-label={label} className="text-muted-foreground hover:text-destructive" onClick={() => setOpen(true)}>
              <Trash2Icon />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{"Remove " + kind + "…"}</TooltipContent>
        </Tooltip>
      ) : (
        <Button variant="outline" size="sm" aria-label={label} onClick={() => setOpen(true)}>
          <Trash2Icon /> Remove…
        </Button>
      )}
      <RemoveDialog open={open} onOpenChange={setOpen} kind={kind} name={name} onApplied={onApplied} />
    </>
  );
}

// ----- restore ------------------------------------------------------------

export function RestoreButton({ slug, onRestored }: { slug: string; onRestored?: (r: InitResult) => void }) {
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    const id = toast.loading("Restoring " + slug + "…");
    try {
      const r = await writes.restoreProject(slug);
      const job = r.job ? " · onboarding job " + r.job.id + (r.job_queued ? " queued" : " already queued") : "";
      toast.success(slug + " restored" + job, { id });
      invalidate();
      onRestored?.(r);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not restore " + slug);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Button variant="outline" size="sm" onClick={() => void run()} disabled={busy}>
      {busy ? <Loader2Icon className="animate-spin" /> : <RotateCcwIcon />}
      Restore
    </Button>
  );
}
