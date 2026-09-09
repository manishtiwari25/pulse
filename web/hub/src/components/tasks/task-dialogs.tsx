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
import { writes, type Task } from "@/lib/api";
import { invalidate, useApi } from "@/lib/use-api";
import type { Org } from "@/lib/api";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

// "As an owner I would like to use the web UI and give a task." One dialog:
// project, title, a Markdown brief, a priority. The hub creates TASK-NNN,
// logs task.created, and the project's lead picks it up on its next wake.
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
  const chosen = slug || project || "";
  const valid = chosen && title.trim().length >= 3 && brief.trim().length >= 1;

  const submit = async () => {
    setBusy(true);
    const id = toast.loading("Creating the task…");
    try {
      const r = await writes.newTask({ project: chosen, title: title.trim(), brief: brief.trim(), priority: Number(priority) });
      toast.success(r.task.id + " created - the " + chosen + "-lead reads it on its next wake.", { id });
      invalidate((p) => p.startsWith("tasks") || p.startsWith("events") || p.startsWith("projects/"));
      onOpenChange(false);
      setTitle("");
      setBrief("");
      setConfirm(false);
      onCreated?.(r.task);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not create the task");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="scroll-thin max-h-[90vh] overflow-y-auto rounded-xl sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>New task</DialogTitle>
          <DialogDescription>Tell the project&apos;s lead what you want. The brief is Markdown; the lead breaks it into jobs and reports back in the chat.</DialogDescription>
        </DialogHeader>
        <div className="grid gap-3">
          <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
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
                        {p.brain ? " (framework)" : ""}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
              <p className="text-xs text-muted-foreground">The project lead receives the task on its next wake.</p>
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="nt-priority">Priority</Label>
              <Select value={priority} onValueChange={setPriority}>
                <SelectTrigger id="nt-priority" className="w-full sm:w-56">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PRIORITIES.map(([n, label]) => (
                    <SelectItem key={n} value={String(n)}>
                      {label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground">Urgent and high tasks run before routine work.</p>
            </div>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="nt-title">Title</Label>
            <Input id="nt-title" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="One line: what should be true when this is done" maxLength={200} autoFocus />
            <p className="text-xs text-muted-foreground">Keep it short and outcome-shaped.</p>
          </div>
          <div className="grid gap-1.5">
            <div className="flex items-center justify-between">
              <Label htmlFor="nt-brief">Brief (Markdown)</Label>
              <span className="text-xs text-muted-foreground tnum">{brief.length} chars</span>
            </div>
            <Tabs defaultValue="write">
              <TabsList className="h-7">
                <TabsTrigger value="write" className="h-6 px-2 text-xs">
                  Write
                </TabsTrigger>
                <TabsTrigger value="preview" className="h-6 px-2 text-xs">
                  Preview
                </TabsTrigger>
              </TabsList>
              <TabsContent value="write">
                <Textarea id="nt-brief" value={brief} onChange={(e) => setBrief(e.target.value)} rows={10} className="min-h-40 font-mono text-[13px]" placeholder={"## Goal\nWhat and why.\n\n## Done when\n- the check that proves it\n\n## Constraints\n- files or behaviour that must not change"} />
              </TabsContent>
              <TabsContent value="preview">
                <div className="min-h-40 rounded-md border p-3">
                  <Markdown source={brief} />
                </div>
              </TabsContent>
            </Tabs>
            <p className="text-xs text-muted-foreground">Markdown is saved as the task brief and shown to the lead, jobs, and chat.</p>
          </div>
        </div>
        <DialogFooter className="items-center gap-2">
          {confirm ? <span className="mr-auto text-xs text-muted-foreground">{"Create " + (title.trim() || "this task") + " in " + chosen + "?"}</span> : null}
          <Button variant="ghost" onClick={() => (confirm ? setConfirm(false) : onOpenChange(false))} disabled={busy}>
            {confirm ? "Back" : "Cancel"}
          </Button>
          {confirm ? (
            <Button onClick={() => void submit()} disabled={busy || !valid}>
              {busy ? <Loader2Icon className="animate-spin" /> : null}
              Confirm and create
            </Button>
          ) : (
            <Button onClick={() => setConfirm(true)} disabled={!valid}>
              Create task
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// Amend = a new revision (title, brief, and a note saying what changed). A
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
    const id = toast.loading("Saving revision " + (task.current_rev + 1) + "…");
    try {
      const payload = title.trim() !== task.title ? { title: title.trim(), brief: brief.trim(), note: note.trim() } : { brief: brief.trim(), note: note.trim() };
      await writes.amendTask(task.id, payload);
      toast.success(task.id + " amended - revision " + (task.current_rev + 1) + ". The lead and any running job read the new brief.", { id });
      invalidate((p) => p.startsWith("tasks") || p.startsWith("events"));
      onOpenChange(false);
      setConfirm(false);
      setNote("");
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "Could not amend the task");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="scroll-thin max-h-[90vh] overflow-y-auto rounded-xl sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{"Amend " + task.id}</DialogTitle>
          <DialogDescription>{"Revision " + (task.current_rev + 1) + " of this task. Say what changed in the note; the lead reads it."}</DialogDescription>
        </DialogHeader>
        <div className="grid gap-3">
          <div className="grid gap-1.5">
            <Label htmlFor="at-title">Title</Label>
            <Input id="at-title" value={title} onChange={(e) => setTitle(e.target.value)} maxLength={200} />
            <p className="text-xs text-muted-foreground">Changing the title updates the task list and the next revision.</p>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="at-brief">Brief (Markdown)</Label>
            <Textarea id="at-brief" value={brief} onChange={(e) => setBrief(e.target.value)} rows={12} className="min-h-40 font-mono text-[13px]" />
            <p className="text-xs text-muted-foreground">The new Markdown brief becomes revision {task.current_rev + 1}.</p>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="at-note">What changed and why</Label>
            <Input id="at-note" value={note} onChange={(e) => setNote(e.target.value)} placeholder="e.g. narrowed the scope to the CLI; the UI is a separate task" maxLength={300} />
            <p className="text-xs text-muted-foreground">Required: this note tells the lead how to adapt.</p>
          </div>
        </div>
        <DialogFooter className="items-center gap-2">
          {!changed ? <span className="mr-auto text-xs text-muted-foreground">Nothing changed yet.</span> : null}
          <Button variant="ghost" onClick={() => (confirm ? setConfirm(false) : onOpenChange(false))} disabled={busy}>
            {confirm ? "Back" : "Cancel"}
          </Button>
          {confirm ? (
            <Button onClick={() => void submit()} disabled={busy || !valid}>
              {busy ? <Loader2Icon className="animate-spin" /> : null}
              Confirm revision {task.current_rev + 1}
            </Button>
          ) : (
            <Button onClick={() => setConfirm(true)} disabled={!valid}>
              Save revision
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
