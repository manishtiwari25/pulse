"use client";

import { useState } from "react";
import { CheckIcon, Loader2Icon, PlusIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { showApiError } from "@/components/pulse/confirm";
import { Markdown } from "@/components/pulse/markdown";
import { Notice } from "@/components/pulse/panel";
import { Mono, StatusBadge, When } from "@/components/pulse/who";
import { skillRepoPath, writes, type ProjectsPayload, type Skill } from "@/lib/api";
import { plural } from "@/lib/format";
import { EMPTY_SKILL_FORM, formFromSkill, scopeLabel, scopeTitle, skillFormReady, skillInput, skillPatch, validateSkillForm, type SkillForm } from "@/lib/mcp";
import { invalidate, useApi } from "@/lib/use-api";

// A skill is a markdown recipe every worker of a project (or of every
// project) is told about in its brief and reads with `pulse skill show`.
// The owner writes one here the way `pulse skill add` does; agents add
// their own from the shell.

export const refreshSkills = () => invalidate((p) => p === "skills" || p.startsWith("skills?") || p.startsWith("skills/") || p.startsWith("events"));

const ALL = "__all__";

// `initial` pre-fills a new skill's form (the toolkit's drafts use it).
export function SkillDialog({ open, onOpenChange, skill, project, initial }: { open: boolean; onOpenChange: (o: boolean) => void; skill?: Skill | null; project?: string; initial?: Partial<SkillForm> }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl">{open ? <SkillFormBody skill={skill ?? null} project={project ?? ""} initial={initial} close={() => onOpenChange(false)} /> : null}</DialogContent>
    </Dialog>
  );
}

function SkillFormBody({ skill, project, initial, close }: { skill: Skill | null; project: string; initial?: Partial<SkillForm>; close: () => void }) {
  const { data: projects } = useApi<ProjectsPayload>("projects", { refresh: false });
  const [f, setF] = useState<SkillForm>(() => (skill ? formFromSkill(skill) : { ...EMPTY_SKILL_FORM, project, ...initial }));
  const [busy, setBusy] = useState(false);
  const [touched, setTouched] = useState(false);
  const set = <K extends keyof SkillForm>(k: K, v: SkillForm[K]) => setF((cur) => ({ ...cur, [k]: v }));
  const errors = validateSkillForm(f);
  const slugs = (projects?.projects ?? []).filter((p) => p.status === "active").map((p) => p.slug);
  const knownProject = !f.project || slugs.includes(f.project);

  const submit = async () => {
    setTouched(true);
    if (errors.length) return;
    setBusy(true);
    const id = toast.loading(skill ? "Saving " + skill.name + "…" : "Adding " + f.name.trim() + "…");
    try {
      if (skill) {
        const patch = skillPatch(skill, f);
        if (!Object.keys(patch).length) {
          toast.dismiss(id);
          toast.message("Nothing changed.");
          close();
          return;
        }
        const r = await writes.updateSkill(skill.id, patch);
        toast.success(r.skill.name + " saved.", { id });
      } else {
        const r = await writes.addSkill(skillInput(f));
        toast.success(r.skill.name + " added.", { id, description: "Agents of " + scopeLabel(r.skill.project) + " see it in their brief from their next job." });
      }
      refreshSkills();
      close();
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, skill ? "The skill wasn't saved" : "The skill wasn't added");
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>{skill ? "Edit " + skill.name : "Add a skill"}</DialogTitle>
        <DialogDescription>A set of instructions agents read before work it covers: a setup, a debugging path, a check. Write it in Markdown.</DialogDescription>
      </DialogHeader>
      <div className="grid gap-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="grid content-start gap-1.5">
            <Label htmlFor="skill-name">Name</Label>
            <Input id="skill-name" value={f.name} onChange={(e) => set("name", e.target.value)} placeholder="release-check" maxLength={64} spellCheck={false} autoFocus={!skill} className="font-mono text-[13px]" />
            <p className="text-xs text-muted-foreground">Short and lowercase, like release-check. Unique across projects.</p>
          </div>
          <div className="grid content-start gap-1.5">
            <Label htmlFor="skill-project">Who can use it</Label>
            <Select value={f.project || ALL} onValueChange={(v) => set("project", v === ALL ? "" : v)}>
              <SelectTrigger id="skill-project" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>All projects</SelectItem>
                {!knownProject ? <SelectItem value={f.project}>{f.project}</SelectItem> : null}
                {slugs.map((slug) => (
                  <SelectItem key={slug} value={slug}>
                    {slug}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">Agents of every project, or of one.</p>
          </div>
        </div>
        <div className="grid content-start gap-1.5">
          <Label htmlFor="skill-description">When to use it</Label>
          <Input id="skill-description" value={f.description} onChange={(e) => set("description", e.target.value)} placeholder="How to verify a release before tagging it" maxLength={200} />
          <p className="text-xs text-muted-foreground">One line. Agents see it in their brief, so say what the skill covers and when to read it.</p>
        </div>
        <div className="grid content-start gap-1.5">
          <div className="flex items-center justify-between">
            <Label htmlFor="skill-body">Instructions (Markdown)</Label>
            <span className="text-xs text-muted-foreground tnum">{plural(f.body.length, "character")}</span>
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
              <Textarea id="skill-body" value={f.body} onChange={(e) => set("body", e.target.value)} rows={12} className="min-h-48 font-mono text-[13px]" placeholder={"## When\nBefore tagging a release.\n\n## Steps\n1. ...\n\n## Check\nThe command that proves it worked."} />
            </TabsContent>
            <TabsContent value="preview">
              <div className="min-h-48 rounded-md border p-3">
                <Markdown source={f.body} />
              </div>
            </TabsContent>
          </Tabs>
          <p className="text-xs text-muted-foreground">Never paste a secret here: every agent can read skills. Say where it is kept instead (a reference or an environment variable).</p>
        </div>
        {touched && errors.length ? (
          <Notice tone="warn">
            <ul className="list-disc pl-4">
              {errors.map((e) => (
                <li key={e}>{e}</li>
              ))}
            </ul>
          </Notice>
        ) : null}
      </div>
      <DialogFooter className="items-center gap-2">
        <Button variant="ghost" onClick={close} disabled={busy}>
          Cancel
        </Button>
        <Button onClick={() => void submit()} disabled={busy || !skillFormReady(f) || (touched && errors.length > 0)}>
          {busy ? <Loader2Icon className="animate-spin" /> : skill ? <CheckIcon /> : <PlusIcon />}
          {skill ? "Save" : "Add skill"}
        </Button>
      </DialogFooter>
    </>
  );
}

// The body as an agent reads it, with the facts the brief does not carry.
export function SkillViewDialog({ skill, onOpenChange, onEdit }: { skill: Skill | null; onOpenChange: (o: boolean) => void; onEdit?: (k: Skill) => void }) {
  return (
    <Dialog open={!!skill} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl">
        {skill ? (
          <>
            <DialogHeader>
              <DialogTitle className="flex flex-wrap items-center gap-2">
                <span className="anywhere">{skill.name}</span>
                <StatusBadge status={skill.status} />
              </DialogTitle>
              <DialogDescription className="anywhere">{skill.description}</DialogDescription>
            </DialogHeader>
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[13px] text-muted-foreground">
              <span>{scopeTitle(skill.project)}</span>
              <span>{skill.uses ? "Read " + (skill.uses === 1 ? "once" : skill.uses + " times") : "Not read yet"}</span>
              <span>
                {(skillRepoPath(skill) ? "Kept by the repository" : "Added by " + (skill.created_by === "human" || skill.created_by === "owner" ? "you" : skill.created_by)) + ", updated "}
                <When d={skill.updated_at} />
              </span>
              <span className="text-xs">{skill.id}</span>
            </div>
            <div className="rounded-xl border px-4 py-3">
              <Markdown source={skill.body} />
            </div>
            <p className="text-xs text-muted-foreground">
              Agents read it with <Mono>pulse skill show {skill.name}</Mono>; each read is counted.
              {skillRepoPath(skill) ? (
                <>
                  {" The text lives in "}
                  <Mono>{skillRepoPath(skill)}</Mono>
                  {" on the project's default branch: edit it there and commit; Pulse loads the new text by itself. Turning it off here lasts until the next commit that changes the project's skill folders or PULSE file; to drop it, delete the folder."}
                </>
              ) : null}
            </p>
            <DialogFooter>
              {onEdit ? (
                <Button variant="outline" size="sm" onClick={() => onEdit(skill)} aria-label={"Edit " + skill.name}>
                  Edit
                </Button>
              ) : null}
              <Button size="sm" onClick={() => onOpenChange(false)}>
                Close
              </Button>
            </DialogFooter>
          </>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
