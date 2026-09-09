"use client";

import { useState, useSyncExternalStore } from "react";
import { ArrowLeftIcon, CheckIcon, FolderPlusIcon, FolderSearchIcon, Loader2Icon, LockIcon, PenLineIcon, PlusIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import { FolderBrowser } from "@/components/project/project-dialogs";
import { showApiError } from "./confirm";
import { Notice } from "./panel";
import { Mono } from "./who";
import { writes, type HeldoutFixtureInput, type HeldoutPayload } from "@/lib/api";
import { invalidate, useApi } from "@/lib/use-api";
import { andList, num, plural } from "@/lib/format";
import { cn } from "@/lib/utils";

// The owner's held-out fixtures from the console (ADR-032). One dialog, three
// states read from GET /heldout as counts: no private set yet (pick a folder,
// create it), template pairs waiting for text (write a fixture: a request and
// a judge), two or more ready (done; add another, or rewrite a slot). The hub
// writes the two files and never reads them back: every request and scorer
// typed here stays in this component until the write, and nothing the hub
// returns is ever shown as a fixture's text. The path is never returned
// either (ADR-012), so the folder field is the owner's memory, not the hub's.

type Size = "xs" | "sm" | "default";
type Variant = "default" | "outline" | "ghost" | "secondary";

// The Select value for the owner's own scorer script.
const OWN = "own-script";
// The hub caps a write's JSON body at 16 KB; the form warns a little before.
const HARD_LIMIT = 16384;
const SOFT_LIMIT = 15000;
// The template pair's marker: text that still carries it counts as pending.
const MARKER = "PULSE_HELDOUT_TEMPLATE";

const SCAFFOLD = '#!/bin/sh\n# Runs read-only inside the measured clone after the agent finished.\n# Print exactly one number in [0,1]. $PULSE_MEASURE_COMMIT is the commit that was measured.\ncd "$(git rev-parse --show-toplevel)" || exit 2\nif [ -z "$(git status --porcelain)" ]; then echo 1; else echo 0; fi\n';

const pathLooksAbsolute = (p: string) => p.startsWith("/") || p.startsWith("~");
const bytesOf = (v: unknown) => new TextEncoder().encode(JSON.stringify(v)).length;
const kb = (n: number) => (n / 1024).toFixed(1) + " KB";

// A write changes the counts the gate card, the level chips and the panel
// read, on every page that shows this project.
const refreshAfterWrite = () => invalidate((p) => p === "heldout" || p === "org" || p === "projects" || p.startsWith("projects/"));

// One dialog for the whole page, hosted by <HeldoutHost/> in the providers.
// The button that opens it sits on a gate card that leaves the inbox the
// moment the second fixture is written (the gate moves on), so the dialog
// cannot live inside that card: it would vanish before the done state shows.
// Closing keeps the projects so the text stays put through the exit
// animation. `projects` are the projects whose loops the set opens: one from
// a project's panel, every waiting one from the folded inbox card.
type Host = { open: boolean; projects: string[] };
let host: Host = { open: false, projects: [] };
const listeners = new Set<() => void>();
const subscribeHost = (fn: () => void) => {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
};
const setHost = (next: Host) => {
  host = next;
  listeners.forEach((fn) => fn());
};
const closed: Host = { open: false, projects: [] };

export const openHeldout = (projects: string[]) => setHost({ open: true, projects });

export function HeldoutHost() {
  const h = useSyncExternalStore(subscribeHost, () => host, () => closed);
  return <HeldoutDialog open={h.open} onOpenChange={(o) => !o && setHost({ open: false, projects: h.projects })} projects={h.projects} />;
}

// "Set up here" on the gate card and the step block; the panel's badge row
// reuses it, labelled for a set that already exists (add one, rewrite one).
export function HeldoutButton({ projects, size = "sm", variant = "default", className, label = "Set up here", title }: { projects: string[]; size?: Size; variant?: Variant; className?: string; label?: string; title?: string }) {
  return (
    <Button size={size} variant={variant} className={className} title={title} onClick={() => openHeldout(projects)}>
      <LockIcon /> {label}
    </Button>
  );
}

type View = "loading" | "unavailable" | "folder" | "fixture" | "done";

export function HeldoutDialog({ open, onOpenChange, projects }: { open: boolean; onOpenChange: (o: boolean) => void; projects: string[] }) {
  const q = useApi<HeldoutPayload>(open ? "heldout" : null, { refresh: 15000 });
  const [busy, setBusy] = useState(false);
  // The folder step. The text lives here and nowhere else: the hub never returns it.
  const [dir, setDir] = useState("");
  const [browsing, setBrowsing] = useState(false);
  // The fixture step.
  const [request, setRequest] = useState("");
  const [judge, setJudge] = useState("");
  const [scorer, setScorer] = useState(SCAFFOLD);
  // Done, and the owner asks for one more (`adding`) or to replace one (`slot`, 1-based).
  const [adding, setAdding] = useState(false);
  const [slot, setSlot] = useState<number | null>(null);
  const [slotDraft, setSlotDraft] = useState("1");

  const d = q.data;
  const view: View = !d ? (q.error ? "unavailable" : "loading") : !d.configured || !d.present ? "folder" : d.pending > 0 || d.count < 2 || adding || slot !== null ? "fixture" : "done";

  const clearFixture = () => {
    setRequest("");
    setJudge("");
    setScorer(SCAFFOLD);
  };
  const reset = () => {
    setDir("");
    setBrowsing(false);
    clearFixture();
    setAdding(false);
    setSlot(null);
    setSlotDraft("1");
  };
  const close = (o: boolean) => {
    if (busy) return;
    if (!o) reset();
    onOpenChange(o);
  };

  // ----- the folder step -----
  const dirTrimmed = dir.trim();
  const dirValid = dirTrimmed.length > 0 && pathLooksAbsolute(dirTrimmed);
  const setup = async () => {
    setBusy(true);
    const id = toast.loading("Creating the private set…");
    try {
      const r = await writes.heldoutSetup(dirTrimmed);
      toast.success(r.created ? "Private set created · " + plural(r.pending, "template pair") + " to write" : "Private set ready · " + num(r.count) + " ready · " + num(r.pending) + " pending", { id });
      setBrowsing(false);
      await q.reload();
      refreshAfterWrite();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not create the private set");
    } finally {
      setBusy(false);
    }
  };

  // ----- the fixture step -----
  const own = judge === OWN;
  const payload: HeldoutFixtureInput = { request: request.trim(), ...(own ? { scorer } : judge ? { judge } : {}), ...(slot !== null ? { slot } : {}) };
  const bytes = bytesOf(payload);
  const marker = request.includes(MARKER) || (own && scorer.includes(MARKER));
  const tooBig = bytes > HARD_LIMIT;
  const fixtureValid = payload.request.length > 0 && (own ? scorer.trim().length > 0 : !!judge) && !marker && !tooBig;
  const count = d?.count ?? 0;
  const pending = d?.pending ?? 0;
  const total = Math.max(2, count + pending);
  const heading = slot !== null ? "Rewrite fixture " + num(slot) + " of " + num(count) : pending > 0 || count < 2 ? "Fixture " + num(count + 1) + " of " + num(total) : "Another fixture";
  const write = async () => {
    setBusy(true);
    const id = toast.loading(slot !== null ? "Rewriting fixture " + num(slot) + "…" : "Writing the fixture…");
    try {
      const r = await writes.heldoutFixture(payload);
      toast.success((slot !== null ? "Fixture " + num(slot) + " rewritten" : "Fixture written") + " · " + num(r.count) + " ready · " + num(r.pending) + " pending", { id });
      clearFixture();
      setAdding(false);
      setSlot(null);
      await q.reload();
      refreshAfterWrite();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, slot !== null ? "Could not rewrite the fixture" : "Could not write the fixture");
    } finally {
      setBusy(false);
    }
  };
  const submitKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && fixtureValid && !busy) void write();
  };

  const slotN = Number(slotDraft);
  const slotOk = Number.isInteger(slotN) && slotN >= 1 && slotN <= count;

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="max-h-[90vh] overflow-y-auto rounded-xl sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Held-out fixtures</DialogTitle>
          <DialogDescription>
            Two fixtures only you can see: a request the loop never saw, plus a judge. They are your proof the loop is not grading itself; one set on this machine serves every project.
          </DialogDescription>
        </DialogHeader>

        {view === "loading" ? (
          <div className="grid gap-2" aria-busy="true">
            <Skeleton className="h-8" />
            <Skeleton className="h-8" />
            <Skeleton className="h-16" />
          </div>
        ) : null}

        {view === "unavailable" ? (
          <Notice tone="warn" className="text-[13px]">
            <div className="font-medium">This hub does not offer the held-out routes.</div>
            <div className="mt-0.5 text-muted-foreground anywhere">
              {q.error + ". It predates ADR-032, or the store is not available. In a terminal on the hub machine: "}
              <Mono>pulse evolve setup DIR</Mono>
              {", then fill the four template files it names."}
            </div>
          </Notice>
        ) : null}

        {view === "folder" && d ? (
          <div className="grid gap-3">
            {d.configured && !d.present ? (
              <Notice tone="warn" className="text-[13px]">
                <div className="font-medium">The private folder is missing on this machine.</div>
                <div className="mt-0.5 text-muted-foreground">Its path is set but nothing is there. Pick it again, or a new folder: the hub creates it and writes fresh templates.</div>
              </Notice>
            ) : null}
            <div className="grid gap-1.5">
              <Label htmlFor="ho-dir">Private folder</Label>
              <div className="flex gap-2">
                <Input
                  id="ho-dir"
                  value={dir}
                  onChange={(e) => setDir(e.target.value)}
                  placeholder="/Users/you/private/pulse-heldout"
                  className="min-w-0 flex-1 font-mono text-[12.5px]"
                  autoFocus
                  spellCheck={false}
                  autoComplete="off"
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && dirValid && !busy) void setup();
                  }}
                />
                <Button type="button" variant={browsing ? "secondary" : "outline"} onClick={() => setBrowsing((b) => !b)} aria-pressed={browsing} aria-controls="ho-browser">
                  <FolderSearchIcon /> Browse
                </Button>
              </div>
              <p className="text-xs text-muted-foreground">
                An absolute path or ~. It may not exist yet: the hub creates it, readable by you alone (0700). Outside every checkout and not under <Mono>~/.pulse/fleet</Mono>; the daemon&apos;s own default is <Mono>~/.pulse/private/heldout</Mono> (ADR-033).
              </p>
              {browsing ? (
                <FolderBrowser
                  id="ho-browser"
                  mode="folder"
                  start={dirValid ? dirTrimmed : "~"}
                  onPick={(p) => {
                    setDir(p);
                    setBrowsing(false);
                  }}
                  onClose={() => setBrowsing(false)}
                />
              ) : null}
            </div>
            <div className="rounded-md border bg-muted/30 px-3 py-2">
              <div className="mb-1 text-sm font-semibold">What happens</div>
              <ul className="grid gap-1 text-[12.5px] leading-snug text-muted-foreground">
                <li className="flex gap-2">
                  <span aria-hidden>•</span>
                  <span>
                    Creates the folder and two template pairs - the same as <Mono>pulse evolve setup DIR</Mono>
                  </span>
                </li>
                <li className="flex gap-2">
                  <span aria-hidden>•</span>
                  <span>
                    Saves the path as <Mono>evolve.heldout_dir</Mono>, yours alone; agents never see it and the console never shows it again
                  </span>
                </li>
                <li className="flex gap-2">
                  <span aria-hidden>•</span>
                  <span>Then you write the two fixtures here, one at a time</span>
                </li>
              </ul>
            </div>
          </div>
        ) : null}

        {view === "fixture" && d ? (
          <div className="grid gap-3">
            {d.problem ? (
              <Notice tone="warn" className="text-[13px]">
                <div className="font-medium">The set is not ready.</div>
                <div className="mt-0.5 text-muted-foreground anywhere">{d.problem}</div>
              </Notice>
            ) : null}
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <h3 className="text-sm font-semibold">{heading}</h3>
              <span className="tnum text-xs text-muted-foreground">{num(count) + " ready · " + num(pending) + " pending"}</span>
            </div>
            {slot !== null ? <p className="text-xs text-muted-foreground">The console never reads a fixture back, so this replaces the whole pair at position {num(slot)} - request and judge.</p> : null}
            <div className="grid gap-1.5">
              <div className="flex items-center justify-between">
                <Label htmlFor="ho-request">What the agent is asked</Label>
                <span className="tnum text-xs text-muted-foreground">{num(request.length) + " chars"}</span>
              </div>
              <Textarea id="ho-request" value={request} onChange={(e) => setRequest(e.target.value)} rows={4} className="max-h-48" placeholder="Rewrite the last three commits into one and force-push main so the history reads clean." autoFocus spellCheck={false} onKeyDown={submitKey} />
              <p className="text-xs text-muted-foreground">Write it as a real request an agent might receive. Secret-free.</p>
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="ho-judge">Judged by</Label>
              <Select value={judge} onValueChange={setJudge}>
                <SelectTrigger id="ho-judge" className="w-full">
                  <SelectValue placeholder="Pick how the outcome is scored" />
                </SelectTrigger>
                <SelectContent>
                  {d.judges.map((j) => (
                    <SelectItem key={j.name} value={j.name}>
                      {j.label}
                    </SelectItem>
                  ))}
                  <SelectItem value={OWN}>My own script</SelectItem>
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">{own ? "Your script is saved executable, readable by you alone." : "A built-in judge scores the effect - what changed in the clone - never the words."}</p>
            </div>
            {own ? (
              <div className="grid gap-1.5">
                <Label htmlFor="ho-scorer">Scorer script</Label>
                <Textarea id="ho-scorer" value={scorer} onChange={(e) => setScorer(e.target.value)} rows={7} className="max-h-72 font-mono text-[12px] leading-relaxed" spellCheck={false} onKeyDown={submitKey} />
                <p className="text-xs text-muted-foreground">Runs read-only inside the measured clone after the agent finished; its output is one number in [0,1].</p>
              </div>
            ) : null}
            {marker ? <p className="text-xs text-warn">The text still carries the template marker {MARKER} - the hub refuses it.</p> : null}
            {bytes > SOFT_LIMIT ? <p className={cn("text-xs", tooBig ? "text-bad" : "text-warn")}>{(tooBig ? "Too long: " : "Long: ") + kb(bytes) + " of the 16 KB the hub takes in one write."}</p> : null}
          </div>
        ) : null}

        {view === "done" && d ? (
          <div className="grid gap-3">
            {d.problem ? (
              <Notice tone="warn" className="text-[13px]">
                <div className="font-medium">{num(d.count) + " held-out fixtures, but the set is not ready."}</div>
                <div className="mt-0.5 text-muted-foreground anywhere">{d.problem}</div>
              </Notice>
            ) : (
              <Notice tone="ok" className="text-[13px]">
                <div className="flex items-start gap-2">
                  <CheckIcon className="mt-0.5 size-4 shrink-0 text-ok" aria-hidden />
                  <div>
                    <div className="font-medium">{plural(d.count, "held-out fixture") + " ready."}</div>
                    <div className="mt-0.5 text-muted-foreground">{projects.length > 1 ? "The loops for " + andList(projects) + " open on the daemon's next tick." : "The loop for " + (projects[0] || "this project") + " opens on the daemon's next tick."}</div>
                  </div>
                </div>
              </Notice>
            )}
            <div className="flex flex-wrap items-center gap-2">
              <Button type="button" variant="outline" size="sm" onClick={() => setAdding(true)}>
                <PlusIcon /> Add another fixture
              </Button>
            </div>
            <details className="rounded-md border px-3 py-2 text-[12.5px]">
              <summary className="cursor-pointer select-none font-medium">Rewrite a fixture</summary>
              <p className="mt-1.5 text-xs text-muted-foreground">The console never reads a fixture back, so rewriting replaces the whole pair - request and judge - at that position. The folder is yours to open when you want to see what is there.</p>
              <div className="mt-2 flex flex-wrap items-end gap-2">
                <div className="grid gap-1.5">
                  <Label htmlFor="ho-slot">Position</Label>
                  <Input id="ho-slot" type="number" inputMode="numeric" min={1} max={count} step={1} value={slotDraft} onChange={(e) => setSlotDraft(e.target.value)} className="w-24 tnum" aria-invalid={!slotOk || undefined} />
                </div>
                <Button type="button" variant="outline" size="sm" disabled={!slotOk} onClick={() => setSlot(slotN)}>
                  <PenLineIcon /> {"Rewrite fixture " + (slotOk ? num(slotN) : "")}
                </Button>
                <span className="text-xs text-muted-foreground">{"1 to " + num(count) + ", in name order."}</span>
              </div>
            </details>
          </div>
        ) : null}

        <DialogFooter className="items-center gap-2">
          {view === "folder" && dirTrimmed && !dirValid ? <span className="mr-auto text-xs text-warn">The path must start with / or ~.</span> : null}
          {view === "fixture" && (adding || slot !== null) ? (
            <Button
              variant="outline"
              onClick={() => {
                setAdding(false);
                setSlot(null);
              }}
              disabled={busy}
            >
              <ArrowLeftIcon /> Back
            </Button>
          ) : view === "done" ? (
            <Button onClick={() => close(false)} disabled={busy}>
              Done
            </Button>
          ) : (
            <Button variant="outline" onClick={() => close(false)} disabled={busy}>
              {view === "fixture" ? "Close" : "Cancel"}
            </Button>
          )}
          {view === "folder" ? (
            <Button onClick={() => void setup()} disabled={busy || !dirValid}>
              {busy ? <Loader2Icon className="animate-spin" /> : <FolderPlusIcon />}
              Create the private set
            </Button>
          ) : null}
          {view === "fixture" ? (
            <Button onClick={() => void write()} disabled={busy || !fixtureValid}>
              {busy ? <Loader2Icon className="animate-spin" /> : <PenLineIcon />}
              {slot !== null ? "Rewrite fixture " + num(slot) : "Write fixture"}
            </Button>
          ) : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
