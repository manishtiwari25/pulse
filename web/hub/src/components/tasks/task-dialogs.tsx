"use client";

import { useState } from "react";
import { Loader2Icon, PlusIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { showApiError } from "@/components/pulse/confirm";
import { Markdown } from "@/components/pulse/markdown";
import { PRIORITIES } from "@/lib/format";
import { cn } from "@/lib/utils";
import { writes, type Task } from "@/lib/api";
import { invalidate, useApi } from "@/lib/use-api";
import type { Org } from "@/lib/api";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

// "As an owner I would like to use the web UI and give a task." One dialog:
// project, what to do (the title), the details (a Markdown brief) and how
// urgent it is. The hub creates TASK-NNN,
// logs task.created, and the project's lead picks it up on its next wake.
// The priority levels (PRIORITIES) in the words the owner thinks in.
const URGENCY: Record<number, [string, string]> = {
  0: ["Low", "When nothing else is waiting"],
  10: ["Normal", "In turn with the other work"],
  30: ["High", "Ahead of the agents' own ideas"],
  50: ["Urgent", "Ahead of almost everything else"],
};

export function NewTaskButton({ project, onCreated, size = "sm", variant = "default" }: { project?: string; onCreated?: (t: Task) => void; size?: "sm" | "default"; variant?: "default" | "outline" }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size={size} variant={variant} onClick={() => setOpen(true)}>
        <PlusIcon /> New task
      </Button>
      <NewTaskDialog open={open} onOpenChange={setOpen} project={project} onCreated={onCreated} />
    </>
  );
}

export function NewTaskDialog({ open, onOpenChange, project, onCreated }: { open: boolean; onOpenChange: (o: boolean) => void; project?: string; onCreated?: (t: Task) => void }) {
  const { data: org } = useApi<Org>(open ? "org" : null, { refresh: false });
  const [slug, setSlug] = useState(project ?? "");
  const [title, setTitle] = useState("");
  const [brief, setBrief] = useState("");
  const [priority, setPriority] = useState("10");
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const projects = (org?.projects ?? []).filter((p) => p.present);
  const lead = (chosen: string) => (chosen ? "the " + chosen + " lead agent" : "the lead agent");
  const chosen = slug || project || "";
  const valid = chosen && title.trim().length >= 3 && brief.trim().length >= 1;

  const submit = async () => {
    setBusy(true);
    const id = toast.loading("Giving the task…");
    try {
      const r = await writes.newTask({ project: chosen, title: title.trim(), brief: brief.trim(), priority: Number(priority) });
      toast.success("Task given. " + sentenceCase(lead(chosen)) + " picks it up shortly and replies in its chat.", { id, description: r.task.id });
      invalidate((p) => p.startsWith("tasks") || p.startsWith("events") || p.startsWith("projects/"));
      onOpenChange(false);
      setTitle("");
      setBrief("");
      setConfirm(false);
      onCreated?.(r.task);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "The task wasn't given");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Give the agents a task</DialogTitle>
          <DialogDescription>Say what you want done. The project&apos;s lead agent turns it into steps, hands them to agents and reports back in the task&apos;s chat.</DialogDescription>
        </DialogHeader>
        <div className="grid gap-4">
          <div className="grid gap-1.5">
              <Label htmlFor="nt-project">Project</Label>
              {project ? (
                <Input id="nt-project" value={project} readOnly />
              ) : (
                <Select value={slug} onValueChange={setSlug}>
                  <SelectTrigger id="nt-project" className="w-full">
                    <SelectValue placeholder={projects.length ? "Pick a project" : "Loading projects…"} />
                  </SelectTrigger>
                  <SelectContent>
                    {projects.map((p) => (
                      <SelectItem key={p.slug} value={p.slug}>
                        {p.slug}
                        {p.brain ? " (PULSE's own source)" : ""}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="nt-title">What should the agents do?</Label>
            <Input id="nt-title" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="For example: Fix the flaky checkout test" maxLength={200} autoFocus />
            <p className="text-xs text-muted-foreground">One short line. It becomes the task&apos;s name.</p>
          </div>
          <BriefField id="nt-brief" value={brief} onChange={setBrief} placeholder={"What you want and why.\n\nHow you'll know it's done:\n- the check that proves it\n\nLeave alone:\n- files or behaviour that must not change"} />
          <UrgencyField value={priority} onChange={setPriority} />
        </div>
        <DialogFooter className="items-center gap-2">
          {confirm ? <span className="mr-auto text-[13px] leading-relaxed text-foreground anywhere">{"Give \u201c" + (title.trim() || "this task") + "\u201d to " + lead(chosen) + "?"}</span> : null}
          <Button variant="ghost" onClick={() => (confirm ? setConfirm(false) : onOpenChange(false))} disabled={busy}>
            {confirm ? "Back" : "Cancel"}
          </Button>
          {confirm ? (
            <Button onClick={() => void submit()} disabled={busy || !valid}>
              {busy ? <Loader2Icon className="animate-spin" /> : null}
              Yes, give it
            </Button>
          ) : (
            <Button onClick={() => setConfirm(true)} disabled={!valid} title={valid ? undefined : "Pick a project, say what to do (3 letters or more) and add the details."}>
              Give the task
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// Amend (shown as "Edit the task") = a new revision: title, brief, and a note
// saying what changed. A
// leased job sees the new brief on its next heartbeat; a queued one is
// re-planned; a done task cannot be amended.
export function AmendTaskDialog({ task, open, onOpenChange }: { task: Task; open: boolean; onOpenChange: (o: boolean) => void }) {
  const [title, setTitle] = useState(task.title);
  const [brief, setBrief] = useState(task.brief);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const changed = title.trim() !== task.title || brief.trim() !== task.brief;
  const valid = changed && note.trim().length > 0 && title.trim().length >= 3 && brief.trim().length > 0;
  const submit = async () => {
    setBusy(true);
    const id = toast.loading("Saving your changes…");
    try {
      const payload = title.trim() !== task.title ? { title: title.trim(), brief: brief.trim(), note: note.trim() } : { brief: brief.trim(), note: note.trim() };
      await writes.amendTask(task.id, payload);
      toast.success("Changes saved. The lead agent and any running step read the new version.", { id, description: task.id + ", version " + (task.current_rev + 1) });
      invalidate((p) => p.startsWith("tasks") || p.startsWith("events"));
      onOpenChange(false);
      setConfirm(false);
      setNote("");
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Your changes weren't saved");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Edit the task</DialogTitle>
          <DialogDescription>Your changes become a new version of the task. The lead agent and any step already running read it and adapt.</DialogDescription>
        </DialogHeader>
        <div className="grid gap-4">
          <div className="grid gap-1.5">
            <Label htmlFor="at-title">What should the agents do?</Label>
            <Input id="at-title" value={title} onChange={(e) => setTitle(e.target.value)} maxLength={200} />
          </div>
          <BriefField id="at-brief" value={brief} onChange={setBrief} rows={12} />
          <div className="grid gap-1.5">
            <Label htmlFor="at-note">What changed, and why?</Label>
            <Input id="at-note" value={note} onChange={(e) => setNote(e.target.value)} placeholder="For example: only the command line for now, the web page is a separate task" maxLength={300} />
            <p className="text-xs text-muted-foreground">Needed: the lead agent reads this to know how to adapt.</p>
          </div>
        </div>
        <DialogFooter className="items-center gap-2">
          {!changed ? <span className="mr-auto text-[13px] text-muted-foreground">Nothing changed yet.</span> : confirm ? <span className="mr-auto text-[13px] text-foreground">Save these changes and tell the lead agent?</span> : null}
          <Button variant="ghost" onClick={() => (confirm ? setConfirm(false) : onOpenChange(false))} disabled={busy}>
            {confirm ? "Back" : "Cancel"}
          </Button>
          {confirm ? (
            <Button onClick={() => void submit()} disabled={busy || !valid}>
              {busy ? <Loader2Icon className="animate-spin" /> : null}
              Yes, save them
            </Button>
          ) : (
            <Button onClick={() => setConfirm(true)} disabled={!valid} title={valid ? undefined : changed ? "Say what changed, and why." : undefined}>
              Save the changes
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

const sentenceCase = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

// The details of a task: Markdown, with a preview a tab away. `min-w-0` on
// the tabs keeps a long pasted token or code line from widening the dialog.
export function BriefField({ id, value, onChange, placeholder, rows = 9, label = "Details", hint = "Markdown works: headings, lists, links and code. The lead agent and every step see this." }: { id: string; value: string; onChange: (v: string) => void; placeholder?: string; rows?: number; label?: string; hint?: string }) {
  return (
    <div className="grid gap-1.5">
      <Tabs defaultValue="write" className="min-w-0 gap-1.5">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <Label htmlFor={id}>{label}</Label>
          <TabsList className="h-8">
            <TabsTrigger value="write" className="px-2.5 text-[13px]">
              Write
            </TabsTrigger>
            <TabsTrigger value="preview" className="px-2.5 text-[13px]">
              Preview
            </TabsTrigger>
          </TabsList>
        </div>
        <TabsContent value="write">
          <Textarea id={id} value={value} onChange={(e) => onChange(e.target.value)} rows={rows} className="min-h-40 text-sm leading-relaxed" placeholder={placeholder} />
        </TabsContent>
        <TabsContent value="preview">
          <div className="min-h-40 rounded-lg border bg-card px-3 py-2">{value.trim() ? <Markdown source={value} /> : <p className="text-sm text-muted-foreground">Nothing to preview yet.</p>}</div>
        </TabsContent>
      </Tabs>
      <p className="text-xs text-muted-foreground">{hint}</p>
    </div>
  );
}

// How urgent the task is, as a row of plain choices instead of a number.
export function UrgencyField({ value, onChange, id = "nt-urgency" }: { value: string; onChange: (v: string) => void; id?: string }) {
  return (
    <div className="grid gap-1.5">
      <span id={id} className="text-sm font-medium">How urgent is it?</span>
      <div role="radiogroup" aria-labelledby={id} className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        {PRIORITIES.map(([n]) => {
          const [word, hint] = URGENCY[n] ?? [String(n), ""];
          const on = value === String(n);
          return (
            <button
              key={n}
              type="button"
              role="radio"
              aria-checked={on}
              onClick={() => onChange(String(n))}
              onKeyDown={(e) => {
                const i = PRIORITIES.findIndex(([p]) => p === n);
                const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
                if (!step) return;
                e.preventDefault();
                const next = PRIORITIES[(i + step + PRIORITIES.length) % PRIORITIES.length]![0];
                onChange(String(next));
                (e.currentTarget.parentElement?.querySelector<HTMLButtonElement>('[data-prio="' + next + '"]'))?.focus();
              }}
              data-prio={n}
              tabIndex={on ? 0 : -1}
              className={cn("grid gap-0.5 rounded-lg border px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none", on ? "border-primary bg-primary/[0.06] ring-1 ring-primary" : "bg-card hover:bg-muted/50")}
            >
              <span className="text-sm font-medium text-foreground">{word}</span>
              <span className="text-xs leading-snug text-muted-foreground">{hint}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
