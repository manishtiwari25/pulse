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
import { Disclosure, Notice } from "./panel";
import { Mono } from "./who";
import { writes, type HeldoutFixtureInput, type HeldoutPayload } from "@/lib/api";
import { invalidate, useApi } from "@/lib/use-api";
import { andList, num, plural } from "@/lib/format";
import { cn } from "@/lib/utils";

// The owner's private tests (held-out fixtures) from the console (ADR-032). One dialog, three
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
// The hub caps a write's JSON body at 1 MB; the form warns a little before.
const HARD_LIMIT = 1024 * 1024;
const SOFT_LIMIT = 1000 * 1024;
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

// "Set up private tests" on the gate card and the step block; the panel's
// details reuse it, labelled for a set that already exists (add one, rewrite one).
export function HeldoutButton({ projects, size = "sm", variant = "default", className, label = "Set up private tests", title }: { projects: string[]; size?: Size; variant?: Variant; className?: string; label?: string; title?: string }) {
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
    const id = toast.loading("Creating the private folder…");
    try {
      const r = await writes.heldoutSetup(dirTrimmed);
      toast.success(r.created ? "Private folder ready. Next, write " + plural(r.pending, "test") + "." : "Private folder ready: " + num(r.count) + " ready, " + num(r.pending) + " still to write.", { id });
      setBrowsing(false);
      await q.reload();
      refreshAfterWrite();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Couldn't create the private folder");
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
  const heading = slot !== null ? "Rewrite private test " + num(slot) + " of " + num(count) : pending > 0 || count < 2 ? "Private test " + num(count + 1) + " of " + num(total) : "Another private test";
  const write = async () => {
    setBusy(true);
    const id = toast.loading(slot !== null ? "Rewriting private test " + num(slot) + "…" : "Saving the private test…");
    try {
      const r = await writes.heldoutFixture(payload);
      toast.success((slot !== null ? "Private test " + num(slot) + " rewritten" : "Private test saved") + ". " + num(r.count) + " ready" + (r.pending ? ", " + num(r.pending) + " still to write." : "."), { id });
      clearFixture();
      setAdding(false);
      setSlot(null);
      await q.reload();
      refreshAfterWrite();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, slot !== null ? "Couldn't rewrite the private test" : "Couldn't save the private test");
    } finally {
      setBusy(false);
    }
  };
  const submitKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && fixtureValid && !busy) void write();
  };

  const slotN = Number(slotDraft);
  const slotOk = Number.isInteger(slotN) && slotN >= 1 && slotN <= count;

  const stepNo = view === "folder" ? 1 : view === "fixture" && !adding && slot === null ? 2 : 0;
  const gen = d?.generated ?? 0;

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Private tests</DialogTitle>
          <DialogDescription>
            Private tests are test cases agents never see. Because agents can&apos;t tune their changes to pass them, they show whether self-improvement really makes a project better. One set on this machine serves every project.
          </DialogDescription>
        </DialogHeader>

        {stepNo ? (
          <ol className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[13px]" aria-label="Steps">
            {["Choose a private folder", "Write two tests"].map((t, i) => (
              <li key={t} className={cn("flex items-center gap-2", i + 1 === stepNo ? "font-medium text-foreground" : "text-muted-foreground")} aria-current={i + 1 === stepNo ? "step" : undefined}>
                <span className={cn("flex size-5 items-center justify-center rounded-full text-xs", i + 1 < stepNo ? "bg-ok/15 text-ok" : i + 1 === stepNo ? "bg-primary text-primary-foreground" : "bg-muted")}>{i + 1 < stepNo ? <CheckIcon className="size-3" aria-hidden /> : i + 1}</span>
                {t}
              </li>
            ))}
          </ol>
        ) : null}

        {view === "loading" ? (
          <div className="grid gap-2" aria-busy="true">
            <Skeleton className="h-8" />
            <Skeleton className="h-8" />
            <Skeleton className="h-16" />
          </div>
        ) : null}

        {view === "unavailable" ? (
          <Notice tone="warn" className="text-[13px]">
            <div className="font-medium">This version of PULSE can&apos;t set up private tests from the console.</div>
            <div className="mt-0.5 text-muted-foreground anywhere">
              {"In a terminal on this machine, run "}
              <Mono>pulse evolve setup DIR</Mono>
              {", then fill in the four template files it creates. (" + q.error + ")"}
            </div>
          </Notice>
        ) : null}

        {view === "folder" && d ? (
          <div className="grid gap-3">
            {d.configured && !d.present ? (
              <Notice tone="warn" className="text-[13px]">
                <div className="font-medium">The private folder is missing.</div>
                <div className="mt-0.5 text-muted-foreground">PULSE remembers a folder, but nothing is there any more. Choose it again or pick a new one; PULSE creates it with fresh templates.</div>
              </Notice>
            ) : null}
            <div className="grid gap-1.5">
              <Label htmlFor="ho-dir">Private folder</Label>
              <div className="flex gap-2">
                <Input
                  id="ho-dir"
                  value={dir}
                  onChange={(e) => setDir(e.target.value)}
                  placeholder="~/private/pulse-tests"
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
              <p className="text-xs leading-relaxed text-muted-foreground">
                A full path, starting with / or ~. It doesn&apos;t have to exist: PULSE creates it, readable only by you. Keep it outside your projects and outside <Mono>~/.pulse/fleet</Mono>. The runner&apos;s own choice would be <Mono>~/.pulse/private/heldout</Mono>.
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
            <div className="rounded-xl border bg-muted/40 px-4 py-3">
              <div className="mb-1.5 text-sm font-semibold">What happens next</div>
              <ol className="grid list-decimal gap-1 pl-5 text-[13px] leading-relaxed text-muted-foreground">
                <li>PULSE creates the folder with two blank test templates.</li>
                <li>It remembers the folder for you. Agents never learn where it is, and this console never shows it again.</li>
                <li>You write the two tests here, one at a time.</li>
              </ol>
            </div>
            <Disclosure summary="Prefer the terminal?" contentClassName="mt-1.5">
              <p className="text-xs leading-relaxed text-muted-foreground">Run <Mono>pulse evolve setup DIR</Mono> on this machine, then fill in the template files it names. It saves the folder as <Mono>evolve.heldout_dir</Mono>.</p>
            </Disclosure>
          </div>
        ) : null}

        {view === "fixture" && d ? (
          <div className="grid gap-3">
            {d.problem ? (
              <Notice tone="warn" className="text-[13px]">
                <div className="font-medium">The private tests aren&apos;t ready yet.</div>
                <div className="mt-0.5 text-muted-foreground anywhere">{d.problem}</div>
              </Notice>
            ) : null}
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <h3 className="text-[15px] font-semibold">{heading}</h3>
              <span className="text-[13px] text-muted-foreground">{num(count) + " ready" + (pending ? ", " + num(pending) + " to write" : "")}</span>
            </div>
            {slot !== null ? <p className="text-[13px] leading-relaxed text-muted-foreground">PULSE never shows a private test again, so this replaces test {num(slot)} completely: what it asks and how it&apos;s scored.</p> : <p className="text-[13px] leading-relaxed text-muted-foreground">A private test is a request an agent might get, plus a way to score what the agent did.</p>}
            <div className="grid gap-1.5">
              <div className="flex items-center justify-between">
                <Label htmlFor="ho-request">What the agent is asked</Label>
                <span className="tnum text-xs text-muted-foreground">{plural(request.length, "character")}</span>
              </div>
              <Textarea id="ho-request" value={request} onChange={(e) => setRequest(e.target.value)} rows={4} className="max-h-48" placeholder="Rewrite the last three commits into one and force-push main so the history reads clean." autoFocus spellCheck={false} onKeyDown={submitKey} />
              <p className="text-xs text-muted-foreground">Write it like a real request. Leave out secrets.</p>
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="ho-judge">How it&apos;s scored</Label>
              <Select value={judge} onValueChange={setJudge}>
                <SelectTrigger id="ho-judge" className="w-full">
                  <SelectValue placeholder="Choose how the result is scored" />
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
              <p className="text-xs text-muted-foreground">{own ? "Your script is saved as a program only you can read." : "A built-in check scores what changed in the project, never the agent's words."}</p>
            </div>
            {own ? (
              <div className="grid gap-1.5">
                <Label htmlFor="ho-scorer">Scoring script</Label>
                <Textarea id="ho-scorer" value={scorer} onChange={(e) => setScorer(e.target.value)} rows={7} className="max-h-72 font-mono text-[12px] leading-relaxed" spellCheck={false} onKeyDown={submitKey} />
                <p className="text-xs text-muted-foreground">It runs read-only in a copy of the project after the agent finishes and prints one number from 0 to 1.</p>
              </div>
            ) : null}
            {marker ? <p className="text-xs text-warn">The text still contains the template marker {MARKER}. Replace it with your own words.</p> : null}
            {bytes > SOFT_LIMIT ? <p className={cn("text-xs", tooBig ? "text-bad" : "text-warn")}>{(tooBig ? "Too long: " : "Nearly too long: ") + kb(bytes) + " of the 1 MB PULSE accepts at once."}</p> : null}
          </div>
        ) : null}

        {view === "done" && d ? (
          <div className="grid gap-3">
            {d.problem ? (
              <Notice tone="warn" className="text-[13px]">
                <div className="font-medium">{plural(d.count, "private test") + " saved, but they aren't ready yet."}</div>
                <div className="mt-0.5 text-muted-foreground anywhere">{d.problem}</div>
              </Notice>
            ) : (
              <Notice tone="ok" className="text-[13px]">
                <div className="font-medium">{plural(d.count, "private test") + " ready."}</div>
                <div className="mt-0.5 text-muted-foreground">{"Self-improvement for " + (projects.length > 1 ? andList(projects) : projects[0] || "this project") + " starts on the runner's next round."}</div>
              </Notice>
            )}
            {gen ? <p className="text-[13px] leading-relaxed text-muted-foreground">{(gen === d.count ? "The runner wrote them" : "The runner wrote " + num(gen) + " of them") + " itself. Rewrite any to make it yours; the runner never overwrites a test you wrote."}</p> : null}
            <div className="flex flex-wrap items-center gap-2">
              <Button type="button" variant="outline" size="sm" onClick={() => setAdding(true)}>
                <PlusIcon /> Add another test
              </Button>
            </div>
            <Disclosure summary="Rewrite a test" variant="row" className="rounded-xl border px-2 py-0.5 text-[13px]" contentClassName="mt-0.5 pb-2">
              <p className="text-xs leading-relaxed text-muted-foreground">PULSE never shows a private test again, so rewriting replaces the whole test at that position. To see what&apos;s there, open the private folder yourself.</p>
              <div className="mt-2 flex flex-wrap items-end gap-2">
                <div className="grid gap-1.5">
                  <Label htmlFor="ho-slot">Test number</Label>
                  <Input id="ho-slot" type="number" inputMode="numeric" min={1} max={count} step={1} value={slotDraft} onChange={(e) => setSlotDraft(e.target.value)} className="w-24 tnum" aria-invalid={!slotOk || undefined} />
                </div>
                <Button type="button" variant="outline" size="sm" disabled={!slotOk} onClick={() => setSlot(slotN)}>
                  <PenLineIcon /> {"Rewrite test " + (slotOk ? num(slotN) : "")}
                </Button>
                <span className="text-xs text-muted-foreground">{"1 to " + num(count) + ", in name order."}</span>
              </div>
            </Disclosure>
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
              {view === "fixture" ? "Finish later" : "Cancel"}
            </Button>
          )}
          {view === "folder" ? (
            <Button onClick={() => void setup()} disabled={busy || !dirValid}>
              {busy ? <Loader2Icon className="animate-spin" /> : <FolderPlusIcon />}
              Create the folder
            </Button>
          ) : null}
          {view === "fixture" ? (
            <Button onClick={() => void write()} disabled={busy || !fixtureValid}>
              {busy ? <Loader2Icon className="animate-spin" /> : <PenLineIcon />}
              {slot !== null ? "Rewrite test " + num(slot) : "Save test"}
            </Button>
          ) : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
