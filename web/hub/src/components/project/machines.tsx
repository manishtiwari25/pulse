"use client";

import Link from "next/link";
import { useState } from "react";
import { CheckIcon, Loader2Icon, PencilLineIcon, ServerIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { showApiError } from "@/components/pulse/confirm";
import { Panel } from "@/components/pulse/panel";
import { Mono, ToneBadge } from "@/components/pulse/who";
import { writes, type ProjectNeeds, type ProjectNeedsMachine } from "@/lib/api";
import { plural } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { invalidate, useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// Where a project's jobs run. A project says what kind of machine its work
// needs in a few words - macos and xcode for an iOS app, linux and gpu for a
// model - and PULSE hands each job only to a machine that offers every
// word; the rest of the fleet takes the other projects' work. This panel
// shows those words, which paired machines fit, and lets the owner change
// them (`pulse init PATH --needs ...`, or `needs:` in the manifest). Queued
// jobs follow a change; a running one keeps its machine.

export const refreshNeeds = (slug: string) => invalidate((p) => p === "projects/" + encodeURIComponent(slug) + "/needs" || p === "projects" || p === "org" || p.startsWith("org?"));

const underline = "underline decoration-foreground/25 underline-offset-4 hover:decoration-foreground/60";

function offersOf(m: ProjectNeedsMachine) {
  return m.offers.replace(/\s*\(reserved\)$/, "");
}

export function MachinesPanel({ slug, canDecide = true }: { slug: string; canDecide?: boolean }) {
  const q = useApi<ProjectNeeds>(slug === "commons" ? null : "projects/" + encodeURIComponent(slug) + "/needs", { refresh: 60000 });
  const [editing, setEditing] = useState(false);
  if (slug === "commons") return null;
  const d = q.data;
  const words = d?.words ?? [];
  const machines = d?.machines ?? [];
  const fits = machines.filter((m) => m.fits);
  const active = fits.filter((m) => m.status === "active");
  const nobody = Boolean(d) && words.length > 0 && active.length === 0;
  const summary = d ? (words.length ? plural(active.length, "machine") + " fit" + (active.length === 1 ? "s" : "") : plural(machines.filter((m) => m.status === "active").length, "machine")) : undefined;
  return (
    <Panel
      title="Runs on"
      icon={ServerIcon}
      actions={
        <>
          {summary ? <span>{summary}</span> : null}
          {d && canDecide ? (
            <Button type="button" variant="ghost" size="sm" className="h-7 px-2 text-[13px]" onClick={() => setEditing(true)}>
              <PencilLineIcon className="size-3.5" />
              Change
            </Button>
          ) : null}
        </>
      }
    >
      {!d ? (
        <p className="text-[13px] text-muted-foreground">{q.error ? "The machines didn't load (" + q.error + "). This refreshes on its own." : "Asking the machines…"}</p>
      ) : (
        <div className="grid gap-3">
          <p className="text-sm leading-relaxed anywhere">
            {words.length === 0 ? (
              <>
                <span className="font-medium">Any machine.</span> <span className="text-muted-foreground">This project&apos;s jobs run wherever a worker is free. Say what they need - an operating system, a chip, a tool - and PULSE keeps them to machines that have it.</span>
              </>
            ) : (
              <>
                <span className="font-medium">A machine that offers</span> {words.map((w) => <Mono key={w} className="mr-1">{w}</Mono>)}
                <span className="text-muted-foreground">- every word, not one of them. Other projects&apos; work takes the rest of the fleet.</span>
              </>
            )}
          </p>
          {nobody ? (
            <p className="rounded-lg border border-warn/40 bg-warn/10 px-3 py-2 text-[13px] leading-relaxed text-foreground anywhere" role="status">
              <span className="font-medium">Nothing can take this project&apos;s jobs right now.</span> They wait in the queue as <Mono>blocked</Mono> until a machine offers {words.join(", ")}: pair one on the{" "}
              <Link href={hrefs.compute()} className={underline}>
                Compute
              </Link>{" "}
              page, or add the word this machine is missing under{" "}
              <Link href={hrefs.settings("daemon")} className={underline}>
                Settings → This machine
              </Link>
              .
            </p>
          ) : null}
          {machines.length ? (
            <ul className="grid gap-1.5" aria-label="Machines">
              {machines.map((m) => (
                <li key={m.name} className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[13px] leading-relaxed">
                  {m.fits ? <CheckIcon className="size-3.5 shrink-0 text-ok" aria-label="fits" /> : <XIcon className="size-3.5 shrink-0 text-muted-foreground" aria-label="does not fit" />}
                  <span className={cn("font-medium", !m.fits && "text-muted-foreground")}>{m.name}</span>
                  {m.status !== "active" ? <ToneBadge tone="neutral">{m.status}</ToneBadge> : null}
                  <span className="min-w-0 text-muted-foreground anywhere">{m.missing.length ? "lacks " + m.missing.join(", ") : offersOf(m) ? "offers " + offersOf(m) : "offers nothing named"}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-[13px] text-muted-foreground">No machine is paired yet. The hub&apos;s own machine appears here once the daemon runs.</p>
          )}
          <p className="text-[12.5px] leading-relaxed text-muted-foreground anywhere">
            From a shell: <Mono>{"pulse init " + slug + " --needs " + (words.length ? words.join(",") : "macos,xcode")}</Mono>, or <Mono>needs:</Mono> in the repository&apos;s <Mono>.pulse/pulse.yaml</Mono>.
          </p>
        </div>
      )}
      {d ? <NeedsDialog open={editing} onOpenChange={setEditing} slug={slug} current={d} /> : null}
    </Panel>
  );
}

// The change dialog: one line of words, the words the paired machines
// offer as one-click suggestions, and what the change means before it is
// made - which machines will fit. Saving tells the hub; queued jobs follow.
function NeedsDialog({ open, onOpenChange, slug, current }: { open: boolean; onOpenChange: (o: boolean) => void; slug: string; current: ProjectNeeds }) {
  const [busy, setBusy] = useState(false);
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-lg">
        {open ? <NeedsBody slug={slug} current={current} busy={busy} setBusy={setBusy} onOpenChange={onOpenChange} /> : null}
      </DialogContent>
    </Dialog>
  );
}

const WORD = /^[a-z0-9][a-z0-9._:-]*$/;

function splitWords(text: string) {
  return text
    .split(/[\s,]+/)
    .map((w) => w.trim().toLowerCase())
    .filter(Boolean);
}

function NeedsBody({ slug, current, busy, setBusy, onOpenChange }: { slug: string; current: ProjectNeeds; busy: boolean; setBusy: (b: boolean) => void; onOpenChange: (o: boolean) => void }) {
  const [draft, setDraft] = useState(current.words.join(", "));
  const words = Array.from(new Set(splitWords(draft)));
  const bad = words.filter((w) => !WORD.test(w));
  const unchanged = words.slice().sort().join(",") === current.words.slice().sort().join(",");
  const willFit = current.machines.filter((m) => m.status === "active" && words.every((w) => offersOf(m).split(/\s+/).includes(w)));
  const suggestions = current.known.filter((w) => !words.includes(w));
  const toggle = (w: string) => setDraft(words.includes(w) ? words.filter((x) => x !== w).join(", ") : [...words, w].join(", "));
  const apply = async () => {
    if (bad.length || unchanged) return;
    setBusy(true);
    const id = toast.loading("Telling the machines…");
    try {
      const r = await writes.setProjectNeeds(slug, words.join(","));
      toast.success(r.words.length ? slug + " now runs on a machine that offers " + r.words.join(", ") + ". Queued jobs follow; running ones keep their machine." : slug + " now runs on any machine.", { id });
      refreshNeeds(slug);
      invalidate((p) => p.startsWith("tasks"));
      onOpenChange(false);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "The machines weren't changed");
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="contents"
      onSubmit={(e) => {
        e.preventDefault();
        void apply();
      }}
    >
      <DialogHeader>
        <DialogTitle className="anywhere">{"Where " + slug + " runs"}</DialogTitle>
        <DialogDescription className="anywhere">
          Words a machine has to offer, all of them: an operating system (<Mono>macos</Mono>, <Mono>linux</Mono>, <Mono>windows</Mono>), a chip (<Mono>arm64</Mono>, <Mono>amd64</Mono>) or anything a machine is labelled with, such as <Mono>xcode</Mono> or <Mono>gpu</Mono>. Leave it empty for any machine.
        </DialogDescription>
      </DialogHeader>
      <div className="grid gap-1.5">
        <Label htmlFor="project-needs">Needs</Label>
        <Input id="project-needs" value={draft} onChange={(e) => setDraft(e.target.value)} autoFocus placeholder="any machine" disabled={busy} aria-invalid={bad.length > 0 || undefined} spellCheck={false} autoCapitalize="off" />
        {bad.length ? (
          <p className="text-[13px] text-bad anywhere">{bad.map((w) => "`" + w + "`").join(", ") + " " + (bad.length === 1 ? "is" : "are") + " not a word a machine can offer: lowercase letters, digits, dots and dashes."}</p>
        ) : (
          <p className="text-[13px] text-muted-foreground anywhere">
            {words.length === 0 ? "Any machine with a free worker takes this project's jobs." : willFit.length ? willFit.length + " of " + plural(current.machines.filter((m) => m.status === "active").length, "active machine") + " would fit: " + willFit.map((m) => m.name).join(", ") + "." : "No active machine offers all of these yet; jobs would wait as blocked until one does."}
          </p>
        )}
      </div>
      {suggestions.length ? (
        <div className="grid gap-1.5">
          <span className="text-[12.5px] text-muted-foreground">Words the paired machines offer</span>
          <div className="flex flex-wrap gap-1.5" role="group" aria-label="Suggested words">
            {suggestions.map((w) => (
              <button key={w} type="button" disabled={busy} onClick={() => toggle(w)} className="rounded-full border border-border bg-background px-2.5 py-0.5 font-mono text-[12.5px] text-foreground hover:bg-muted disabled:opacity-50">
                {w}
              </button>
            ))}
          </div>
        </div>
      ) : null}
      <DialogFooter>
        <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
          Keep as is
        </Button>
        <Button type="submit" disabled={busy || bad.length > 0 || unchanged}>
          {busy ? <Loader2Icon className="animate-spin" /> : <ServerIcon />}
          {words.length ? "Run on these machines" : "Run on any machine"}
        </Button>
      </DialogFooter>
    </form>
  );
}
