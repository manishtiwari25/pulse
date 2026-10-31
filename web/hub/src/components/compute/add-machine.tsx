"use client";

import { useState } from "react";
import { CheckCircle2Icon, CloudIcon, KeyRoundIcon, Loader2Icon, PlusIcon, ServerIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { showApiError } from "@/components/pulse/confirm";
import { Mono } from "@/components/pulse/who";
import Link from "next/link";
import { hrefs } from "@/lib/nav";
import { writes, type AddMachineInput, type ComputeCaps, type ComputeRequest, type MachineAdded, type Pairing, type ProjectsPayload } from "@/lib/api";
import { rel } from "@/lib/format";
import { invalidate, useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// Adding a machine from the console, the three ways the shell offers:
//
// - Pair (`pulse pair admin@host --name NAME`): the hub reaches an admin
//   account over ssh, creates the pulse user, installs the hands, registers
//   and probes the machine. It takes minutes, so the dialog closes at once
//   and the pairing shows in the machines list with its report as it lands.
// - Register (`pulse executor add`): a machine someone prepared by hand -
//   the hands already installed and a `pulse` user that answers ssh - is
//   added as drained and probed right away when asked.
// - Ask the cloud (`pulse compute request` + approve): the owner asks the
//   configured provider for a machine for a few hours, like an agent would.

export const refreshMachines = () => invalidate((p) => p === "compute" || p === "runner" || p === "org" || p.startsWith("org?") || p.startsWith("projects/"));

const NAME = /^[a-z][a-z0-9-]*$/;
const ADDRESS = /^[A-Za-z0-9._-]+@[A-Za-z0-9._-]+(:\d{1,5})?$/;
const WORD = /^[a-z0-9][a-z0-9._:-]*$/;

function slug(text: string) {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
}

function splitWords(text: string) {
  return text
    .split(/[\s,]+/)
    .map((w) => w.trim().toLowerCase())
    .filter(Boolean);
}

const field = "grid gap-1.5";
const hint = "text-[12.5px] leading-relaxed text-muted-foreground anywhere";
const bad = "text-[12.5px] leading-relaxed text-bad anywhere";

export function AddMachineButton({ caps, className }: { caps?: ComputeCaps; className?: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button type="button" variant="outline" size="sm" className={cn("h-7 gap-1.5 px-2.5 text-[13px]", className)} onClick={() => setOpen(true)}>
        <PlusIcon className="size-3.5" />
        Add machine
      </Button>
      <AddMachineDialog open={open} onOpenChange={setOpen} caps={caps} />
    </>
  );
}

export function AddMachineDialog({ open, onOpenChange, caps }: { open: boolean; onOpenChange: (o: boolean) => void; caps?: ComputeCaps }) {
  const [busy, setBusy] = useState(false);
  return (
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-xl">
        {open ? <AddMachineBody busy={busy} setBusy={setBusy} onOpenChange={onOpenChange} caps={caps} /> : null}
      </DialogContent>
    </Dialog>
  );
}

type Mode = "pair" | "register" | "request";

function AddMachineBody({ busy, setBusy, onOpenChange, caps }: { busy: boolean; setBusy: (b: boolean) => void; onOpenChange: (o: boolean) => void; caps?: ComputeCaps }) {
  const [mode, setMode] = useState<Mode>("pair");
  const provider = caps?.provider || "";
  return (
    <>
      <DialogHeader>
        <DialogTitle>Add a machine</DialogTitle>
        <DialogDescription className="anywhere">Another computer your agents can run on. PULSE sets it up over ssh, takes one you prepared, or asks your cloud provider for one.</DialogDescription>
      </DialogHeader>
      <Tabs value={mode} onValueChange={(v) => !busy && setMode(v as Mode)} className="min-w-0 gap-4">
        <TabsList aria-label="How to add it" className="w-full justify-start">
          <TabsTrigger value="pair" className="flex-1 gap-1.5">
            <KeyRoundIcon className="size-3.5" aria-hidden />
            Set up over ssh
          </TabsTrigger>
          <TabsTrigger value="register" className="flex-1 gap-1.5">
            <ServerIcon className="size-3.5" aria-hidden />
            Already prepared
          </TabsTrigger>
          <TabsTrigger value="request" className="flex-1 gap-1.5">
            <CloudIcon className="size-3.5" aria-hidden />
            Ask the cloud
          </TabsTrigger>
        </TabsList>
        <TabsContent value="pair" className="min-w-0">
          <PairForm busy={busy} setBusy={setBusy} onOpenChange={onOpenChange} />
        </TabsContent>
        <TabsContent value="register" className="min-w-0">
          <RegisterForm busy={busy} setBusy={setBusy} onOpenChange={onOpenChange} />
        </TabsContent>
        <TabsContent value="request" className="min-w-0">
          <RequestForm busy={busy} setBusy={setBusy} onOpenChange={onOpenChange} provider={provider} caps={caps} />
        </TabsContent>
      </Tabs>
    </>
  );
}

type FormProps = { busy: boolean; setBusy: (b: boolean) => void; onOpenChange: (o: boolean) => void };

// The fields pairing and registering share: name, folder, workers, projects, labels.
function useMachineFields() {
  const [name, setName] = useState("");
  const [workdir, setWorkdir] = useState("/srv/pulse");
  const [workers, setWorkers] = useState("1");
  const [projects, setProjects] = useState("");
  const [labels, setLabels] = useState("");
  const stored = slug(name);
  const nameOk = stored.length > 0 && NAME.test(stored) && stored !== "local" && stored !== "x";
  const workdirOk = workdir.trim().startsWith("/") && !workdir.split("/").includes("..");
  const workersN = Number(workers);
  const workersOk = Number.isInteger(workersN) && workersN >= 1 && workersN <= 64;
  const badLabels = splitWords(labels).filter((w) => !WORD.test(w));
  const common = {
    name: stored,
    workdir: workdir.trim() || "/srv/pulse",
    workers: workersOk ? workersN : 1,
    projects: projects.trim(),
    labels: splitWords(labels).join(","),
  };
  const ok = nameOk && workdirOk && workersOk && badLabels.length === 0;
  const fields = (busy: boolean, idPrefix: string) => (
    <>
      <div className="grid gap-3 sm:grid-cols-[1fr_7rem]">
        <div className={field}>
          <Label htmlFor={idPrefix + "-name"}>Name</Label>
          <Input id={idPrefix + "-name"} value={name} onChange={(e) => setName(e.target.value)} placeholder="mac-mini" disabled={busy} autoFocus spellCheck={false} autoCapitalize="off" aria-invalid={(name && !nameOk) || undefined} />
          {name && !nameOk ? <p className={bad}>Lowercase words with dashes, and not local.</p> : stored && stored !== name.trim() ? <p className={hint}>{"Stored as " + stored + "."}</p> : <p className={hint}>How the machine is listed and addressed as ssh host.</p>}
        </div>
        <div className={field}>
          <Label htmlFor={idPrefix + "-workers"}>Workers</Label>
          <Input id={idPrefix + "-workers"} type="number" min={1} max={64} inputMode="numeric" value={workers} onChange={(e) => setWorkers(e.target.value)} disabled={busy} aria-invalid={!workersOk || undefined} />
          <p className={hint}>Jobs at once.</p>
        </div>
      </div>
      <div className={field}>
        <Label htmlFor={idPrefix + "-workdir"}>Folder on the machine</Label>
        <Input id={idPrefix + "-workdir"} value={workdir} onChange={(e) => setWorkdir(e.target.value)} disabled={busy} spellCheck={false} autoCapitalize="off" aria-invalid={!workdirOk || undefined} />
        {workdirOk ? <p className={hint}>Where its clones and runs live; an absolute path.</p> : <p className={bad}>An absolute path on the machine, such as /srv/pulse.</p>}
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className={field}>
          <Label htmlFor={idPrefix + "-labels"}>Offers</Label>
          <Input id={idPrefix + "-labels"} value={labels} onChange={(e) => setLabels(e.target.value)} placeholder="xcode, gpu" disabled={busy} spellCheck={false} autoCapitalize="off" aria-invalid={badLabels.length > 0 || undefined} />
          {badLabels.length ? <p className={bad}>{badLabels.map((w) => "`" + w + "`").join(", ") + ": lowercase letters, digits, dots and dashes."}</p> : <p className={hint}>Words a project can need, besides the system and chip it finds itself.</p>}
        </div>
        <div className={field}>
          <Label htmlFor={idPrefix + "-projects"}>Only for</Label>
          <Input id={idPrefix + "-projects"} value={projects} onChange={(e) => setProjects(e.target.value)} placeholder="every project" disabled={busy} spellCheck={false} autoCapitalize="off" />
          <p className={hint}>Project slugs, comma-separated; empty for all.</p>
        </div>
      </div>
    </>
  );
  return { common, ok, fields, name: stored };
}

function PairForm({ busy, setBusy, onOpenChange }: FormProps) {
  const m = useMachineFields();
  const [admin, setAdmin] = useState("");
  const [dryRun, setDryRun] = useState(false);
  const adminOk = ADDRESS.test(admin.trim());
  const can = m.ok && adminOk && !busy;
  const submit = async () => {
    if (!can) return;
    setBusy(true);
    const id = toast.loading(dryRun ? "Looking at " + admin.trim() + "…" : "Pairing " + m.name + "…");
    try {
      const input: AddMachineInput = { mode: "pair", admin: admin.trim(), dry_run: dryRun, ...m.common };
      await writes.addMachine(input);
      toast.success((dryRun ? "Inspecting " : "Pairing ") + m.name + ". It runs on the hub; the report lands in the machines list" + (dryRun ? "." : ", and the machine takes work once its probe passes."), { id });
      refreshMachines();
      onOpenChange(false);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "The machine wasn't paired");
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <p className={hint}>
        The hub signs in as an administrator, creates a <Mono>pulse</Mono> user that can run only PULSE&apos;s hands, installs them, and checks the machine. The account needs ssh from this computer and <Mono>sudo</Mono> without a password. Nothing else on the machine changes; the report lists every step and how to undo it.
      </p>
      <div className={field}>
        <Label htmlFor="pair-admin">Administrator</Label>
        <Input id="pair-admin" value={admin} onChange={(e) => setAdmin(e.target.value)} placeholder="admin@10.0.0.5 or admin@host:2222" disabled={busy} spellCheck={false} autoCapitalize="off" aria-invalid={(admin && !adminOk) || undefined} />
        {admin && !adminOk ? <p className={bad}>user@host, with :port when it is not 22.</p> : <p className={hint}>An account with passwordless sudo this computer can ssh to.</p>}
      </div>
      {m.fields(busy, "pair")}
      <label className="flex items-start gap-3 rounded-xl border px-3 py-2.5 text-sm">
        <Switch checked={dryRun} onCheckedChange={setDryRun} disabled={busy} aria-label="Only look first" className="mt-0.5" />
        <span className="min-w-0">
          <span className="font-medium">Only look first</span>
          <span className={cn("block", hint)}>Inspect the machine and show what pairing would do, without changing anything.</span>
        </span>
      </label>
      <DialogFooter>
        <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
          Not now
        </Button>
        <Button type="submit" disabled={!can}>
          {busy ? <Loader2Icon className="animate-spin" /> : <KeyRoundIcon />}
          {dryRun ? "Look at it" : "Pair it"}
        </Button>
      </DialogFooter>
    </form>
  );
}

function RegisterForm({ busy, setBusy, onOpenChange }: FormProps) {
  const m = useMachineFields();
  const [ssh, setSsh] = useState("");
  const [probe, setProbe] = useState(true);
  const sshOk = ADDRESS.test(ssh.trim());
  const can = m.ok && sshOk && !busy;
  const submit = async () => {
    if (!can) return;
    setBusy(true);
    const id = toast.loading(probe ? "Adding " + m.name + " and reaching it…" : "Adding " + m.name + "…");
    try {
      const input: AddMachineInput = { mode: "register", ssh: ssh.trim(), probe, ...m.common };
      const r = (await writes.addMachine(input)) as MachineAdded;
      if (r.probe) {
        if (r.probe.ok) toast.success(m.name + " is added and answered: " + r.probe.note, { id });
        else toast.warning(m.name + " is added but did not pass its check", { id, description: r.probe.note + "\nIt stays drained until a probe you ask for passes: fix what the note says, then `pulse executor probe " + m.name + "`." });
      } else {
        toast.success(m.name + " is added. It takes work once a probe passes; the runner probes it on its own.", { id });
      }
      refreshMachines();
      onOpenChange(false);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "The machine wasn't added");
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <p className={hint}>
        For a machine someone already set up: PULSE&apos;s hands installed and a <Mono>pulse</Mono> user this computer can ssh to. It is listed at once and starts taking work after its first check passes.
      </p>
      <div className={field}>
        <Label htmlFor="register-ssh">Address</Label>
        <Input id="register-ssh" value={ssh} onChange={(e) => setSsh(e.target.value)} placeholder="pulse@10.0.0.5 or pulse@host:2222" disabled={busy} spellCheck={false} autoCapitalize="off" aria-invalid={(ssh && !sshOk) || undefined} />
        {ssh && !sshOk ? <p className={bad}>user@host, with :port when it is not 22.</p> : <p className={hint}>The account the daemon reaches the hands with, usually pulse@host.</p>}
      </div>
      {m.fields(busy, "register")}
      <label className="flex items-start gap-3 rounded-xl border px-3 py-2.5 text-sm">
        <Switch checked={probe} onCheckedChange={setProbe} disabled={busy} aria-label="Check it now" className="mt-0.5" />
        <span className="min-w-0">
          <span className="font-medium">Check it now</span>
          <span className={cn("block", hint)}>Reach the machine right away (a minute at most) so it can take work today.</span>
        </span>
      </label>
      <DialogFooter>
        <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
          Not now
        </Button>
        <Button type="submit" disabled={!can}>
          {busy ? <Loader2Icon className="animate-spin" /> : <ServerIcon />}
          Add it
        </Button>
      </DialogFooter>
    </form>
  );
}

const SIZES: Record<string, string[]> = {
  azure: ["Standard_B2s", "Standard_B4ms", "Standard_D4s_v5", "Standard_D8s_v5"],
  aws: ["t3.medium", "t3.large", "m6i.xlarge", "c6i.2xlarge"],
  gcp: ["e2-medium", "e2-standard-4", "n2-standard-8"],
  multipass: ["small", "medium", "large"],
};

function RequestForm({ busy, setBusy, onOpenChange, provider, caps }: FormProps & { provider: string; caps?: ComputeCaps }) {
  const projects = useApi<ProjectsPayload>("projects", { refresh: 60000 });
  const active = (projects.data?.projects ?? []).filter((p) => p.status === "active" && p.slug !== "commons");
  const [project, setProject] = useState("");
  const [size, setSize] = useState("");
  const [hours, setHours] = useState("4");
  const [reason, setReason] = useState("");
  const [approve, setApprove] = useState(true);
  const hoursN = Number(hours);
  const hoursOk = Number.isInteger(hoursN) && hoursN >= 1;
  const chosen = project || (active.length === 1 ? active[0].slug : "");
  const can = Boolean(provider) && Boolean(chosen) && hoursOk && reason.trim().length > 0 && !busy;
  const full = caps ? caps.vms_now >= caps.max_vms : false;
  const submit = async () => {
    if (!can) return;
    setBusy(true);
    const id = toast.loading("Asking " + provider + "…");
    try {
      const input: AddMachineInput = { mode: "request", project: chosen, size: size.trim(), hours: hoursN, reason: reason.trim(), approve };
      const r = (await writes.addMachine(input)) as { request: ComputeRequest };
      toast.success(r.request.status === "approved" ? "Approved: the runner starts a " + (r.request.size || "machine") + " for " + hoursN + "h on its next pass, and the machine joins the list when it answers." : "Asked. It waits under \"Waiting for you\" until you approve it.", { id });
      refreshMachines();
      invalidate((p) => p === "compute" || p.startsWith("events"));
      onOpenChange(false);
    } catch (e) {
      toast.dismiss(id);
      showApiError(e, "The machine wasn't requested");
    } finally {
      setBusy(false);
    }
  };
  if (!provider)
    return (
      <div className="grid gap-3">
        <p className="rounded-xl border bg-muted/40 px-3 py-2.5 text-sm leading-relaxed anywhere">
          <span className="font-medium">No cloud provider is set.</span>{" "}
          <span className="text-muted-foreground">
            Set one under{" "}
            <Link href={hrefs.settings("compute")} className="underline decoration-foreground/25 underline-offset-4 hover:decoration-foreground/60">
              Settings → Limits
            </Link>{" "}
            (the provider, its credentials and a machine size) and PULSE can start machines for a few hours when its agents, or you, ask.
          </span>
        </p>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Close
          </Button>
        </DialogFooter>
      </div>
    );
  const sizes = SIZES[provider] ?? [];
  return (
    <form
      className="grid gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <p className={hint}>
        {"Asks " + provider + " for one machine for a few hours, the way an agent does. The runner starts it, pairs it, and stops it when the hours are up; it counts against the limits below."}
        {caps ? " " + caps.vms_now + " of " + caps.max_vms + " machines are up now" + (caps.max_hours_per_day ? ", " + caps.hours_today + " of " + caps.max_hours_per_day + " hours used today." : ".") : ""}
      </p>
      {full ? <p className={cn(bad, "rounded-lg border border-warn/40 bg-warn/10 px-3 py-2 text-foreground")}>Every allowed machine is up already; the request waits until one stops, or raise the limit in Settings.</p> : null}
      <div className="grid gap-3 sm:grid-cols-2">
        <div className={field}>
          <Label htmlFor="request-project">For project</Label>
          <Select value={chosen} onValueChange={setProject} disabled={busy || active.length === 0}>
            <SelectTrigger id="request-project" aria-label="Project" className="w-full">
              <SelectValue placeholder={projects.data ? (active.length ? "Choose a project" : "No active project") : "Loading…"} />
            </SelectTrigger>
            <SelectContent>
              {active.map((p) => (
                <SelectItem key={p.slug} value={p.slug}>
                  {p.name || p.slug}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <p className={hint}>Whose work it runs; its hours are logged there.</p>
        </div>
        <div className={field}>
          <Label htmlFor="request-hours">Hours</Label>
          <Input id="request-hours" type="number" min={1} inputMode="numeric" value={hours} onChange={(e) => setHours(e.target.value)} disabled={busy} aria-invalid={!hoursOk || undefined} />
          <p className={hint}>It stops on its own after this.</p>
        </div>
      </div>
      <div className={field}>
        <Label htmlFor="request-size">Size</Label>
        <Input id="request-size" value={size} onChange={(e) => setSize(e.target.value)} placeholder={"the size set in Settings"} disabled={busy} spellCheck={false} autoCapitalize="off" list={sizes.length ? "request-sizes" : undefined} />
        {sizes.length ? (
          <datalist id="request-sizes">
            {sizes.map((s) => (
              <option key={s} value={s} />
            ))}
          </datalist>
        ) : null}
        <p className={hint}>{"A " + provider + " machine size; empty takes the one in Settings."}</p>
      </div>
      <div className={field}>
        <Label htmlFor="request-reason">Why</Label>
        <Textarea id="request-reason" value={reason} rows={2} onChange={(e) => setReason(e.target.value)} placeholder="A long build the laptop cannot hold; the release before Friday." disabled={busy} />
        <p className={hint}>One line, kept with the request.</p>
      </div>
      <label className="flex items-start gap-3 rounded-xl border px-3 py-2.5 text-sm">
        <Switch checked={approve} onCheckedChange={setApprove} disabled={busy} aria-label="Start it right away" className="mt-0.5" />
        <span className="min-w-0">
          <span className="font-medium">Start it right away</span>
          <span className={cn("block", hint)}>Approve the request as you make it. Off, it waits under &quot;Waiting for you&quot;.</span>
        </span>
      </label>
      <DialogFooter>
        <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
          Not now
        </Button>
        <Button type="submit" disabled={!can}>
          {busy ? <Loader2Icon className="animate-spin" /> : <CloudIcon />}
          {approve ? "Start a machine" : "Ask for a machine"}
        </Button>
      </DialogFooter>
    </form>
  );
}

// A pairing the hub is running, or finished and not yet dismissed, listed
// above the machines. Running: the last line so far. Finished: the verdict,
// the report unfolded on request, and Dismiss.
export function PairingRow({ p }: { p: Pairing }) {
  const [openReport, setOpenReport] = useState(p.status === "failed");
  const [busy, setBusy] = useState(false);
  const running = p.status === "running";
  const last = p.lines.length ? p.lines[p.lines.length - 1] : "";
  const dismiss = async () => {
    setBusy(true);
    try {
      await writes.dismissPairing(p.name);
      refreshMachines();
    } catch (e) {
      showApiError(e, "The report wasn't dismissed");
    } finally {
      setBusy(false);
    }
  };
  return (
    <li className={cn("min-w-0 rounded-xl border bg-card px-3 py-2.5 text-sm", p.status === "failed" ? "border-bad/40" : running ? "border-primary/40" : "border-ok/40")} aria-live={running ? "polite" : undefined}>
      <div className="flex min-w-0 items-start gap-3">
        {running ? <Loader2Icon className="mt-0.5 size-4 shrink-0 animate-spin text-primary" aria-hidden /> : p.ok ? <CheckCircle2Icon className="mt-0.5 size-4 shrink-0 text-ok" aria-hidden /> : <XIcon className="mt-0.5 size-4 shrink-0 text-bad" aria-hidden />}
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">
            <span className="font-medium text-foreground anywhere">{p.name}</span>
            <span className="text-[13px] text-muted-foreground anywhere">{(p.dry_run ? "inspecting " : "pairing ") + p.admin}</span>
          </div>
          <p className="mt-0.5 text-[13px] leading-relaxed text-muted-foreground anywhere">
            {running ? (
              <>
                <span className="font-medium text-foreground/80">Working</span>
                {" · started " + rel(p.started) + (last ? " · " + last : "")}
              </>
            ) : p.ok ? (
              <>
                <span className="font-medium text-ok">{p.dry_run ? "Looked" : "Paired"}</span>
                {" · " + rel(p.finished || p.started) + (p.dry_run ? " · nothing changed" : " · it takes work once its probe passes")}
              </>
            ) : (
              <>
                <span className="font-medium text-bad">Did not finish</span>
                {" · " + rel(p.finished || p.started) + (last ? " · " + last : "")}
              </>
            )}
          </p>
          {!running && p.lines.length > 1 ? (
            <button type="button" className="mt-1 text-[12.5px] font-medium text-muted-foreground underline-offset-2 hover:text-foreground hover:underline" onClick={() => setOpenReport((o) => !o)} aria-expanded={openReport}>
              {openReport ? "Hide the report" : "Show the report (" + p.lines.length + " lines)"}
            </button>
          ) : null}
          {openReport && !running ? <pre className="mt-2 max-h-72 overflow-auto rounded-lg bg-muted/50 p-2.5 font-mono text-[12px] leading-relaxed whitespace-pre-wrap break-words text-foreground/90">{p.lines.join("\n")}</pre> : null}
        </div>
        {!running ? (
          <Button type="button" variant="ghost" size="sm" className="-my-1 -mr-1 h-7 shrink-0 px-2 text-[13px]" disabled={busy} onClick={() => void dismiss()}>
            Dismiss
          </Button>
        ) : null}
      </div>
    </li>
  );
}
