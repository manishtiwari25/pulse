"use client";

import { useEffect, useState } from "react";
import { ArrowLeftIcon, CheckIcon, CopyIcon, KeyRoundIcon, Loader2Icon, LockKeyholeIcon, PenLineIcon, ShieldCheckIcon, XIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { ConfirmDialog, showApiError } from "@/components/pulse/confirm";
import { Notice } from "@/components/pulse/panel";
import { Mono, When } from "@/components/pulse/who";
import { RowHead, SettingRow } from "@/components/settings/setting-row";
import { ApiError, writes, type CredItem, type CredPayload, type CredProbe, type CredProvider, type CredReport, type CredSetResult, type CredVault, type Setting } from "@/lib/api";
import { invalidate, useApi } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// The daemon's push key from the console (ADR-042). The row shows the state
// `pulse cred status` prints - which provider, which item, the fingerprint -
// never the key; the picker walks the owner from a provider to a reference,
// tests it against every project's remote and saves only when they answer.
// A 1Password service-account token is typed once here and posted once; the
// hub machine keeps it and this component forgets it.

const refreshAfterWrite = () => invalidate((p) => p === "cred" || p.startsWith("settings") || p.startsWith("events") || p.startsWith("runner") || p.startsWith("org"));

// What a reference names, for the row: "pulse push key in PULSE", "pulse-push / deploy", "~/.pulse/secrets/push_key".
function describe(reference: string, providers: CredProvider[]): { provider: string; item: string } {
  const [scheme, rest = ""] = reference.split("://", 2);
  const title = providers.find((p) => p.id === scheme)?.title ?? scheme;
  if (scheme === "op") {
    const [vault = "", item = ""] = rest.split("?")[0]!.split("/").map((s) => decodeURIComponent(s));
    return { provider: title, item: item && vault ? item + " in " + vault : rest };
  }
  if (scheme === "keychain") return { provider: title, item: rest.replace("/", " / ") };
  return { provider: title, item: "/" + rest.replace(/^\/+/, "") };
}

function copy(text: string, what: string) {
  const done = () => toast.success("Copied the " + what + ".");
  if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
    navigator.clipboard.writeText(text).then(done, () => toast.error("Could not copy - select it and copy it yourself."));
    return;
  }
  toast.error("Could not copy - select it and copy it yourself.");
}

// ----- the row ---------------------------------------------------------------

export function CredentialRow({ s, canEdit }: { s: Setting; canEdit: boolean }) {
  const q = useApi<CredPayload>("cred", { refresh: 30000 });
  const [open, setOpen] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [alsoToken, setAlsoToken] = useState(false);
  // A hub without the route (an older build): the plain row still saves the reference.
  if (q.error && !q.data) return <SettingRow s={s} canEdit={canEdit} />;
  const d = q.data;
  const report = d?.report;
  const set = !!report?.reference;
  const named = report?.reference && d ? describe(report.reference, d.providers) : null;
  const pushOff = (report?.landing_push ?? "off") === "off";
  return (
    <div className="grid gap-3 px-4 py-3 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
      <RowHead s={s}>
        {!d ? (
          <Skeleton className="h-4 w-64" />
        ) : set && named ? (
          <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[13px]">
            <KeyRoundIcon className="size-3.5 text-ok" aria-hidden />
            <span className="font-medium">{named.provider}</span>
            <span className="text-muted-foreground">·</span>
            <span className="anywhere">{named.item}</span>
            {report?.fingerprint ? <Mono className="text-[11px] text-muted-foreground">{report.fingerprint}</Mono> : null}
            {pushOff ? <span className="basis-full text-xs text-muted-foreground">Pushing after a landing is off - the key waits until <b>Push after landing</b> is set to main.</span> : null}
          </div>
        ) : (
          <p className="text-[13px] leading-snug text-muted-foreground anywhere">
            Not set - the daemon pushes with the git credentials this machine already has{pushOff ? "" : " (and pushes after every landing)"}. Choose a key of its own so a push never needs you.
          </p>
        )}
      </RowHead>
      <div className="flex flex-wrap items-center gap-2 md:justify-end">
        <Button size="sm" variant={set ? "outline" : "default"} disabled={!canEdit || !d} onClick={() => setOpen(true)}>
          <KeyRoundIcon /> {set ? "Change key…" : "Choose a key…"}
        </Button>
        {set && canEdit ? (
          <Button size="sm" variant="ghost" onClick={() => setRemoving(true)}>
            <XIcon /> Remove
          </Button>
        ) : null}
      </div>
      {d ? <CredentialPicker open={open} onOpenChange={setOpen} payload={d} /> : null}
      <ConfirmDialog
        open={removing}
        onOpenChange={(o) => {
          if (!o) setAlsoToken(false);
          setRemoving(o);
        }}
        title="Remove the push key?"
        description={<>The daemon goes back to pushing with the git credentials this machine already has. The key itself is not touched - only the reference <Mono>{report?.reference}</Mono> is forgotten.</>}
        action="Remove"
        destructive
        success="Push key removed."
        onConfirm={() => writes.credClear(alsoToken)}
        invalidates={(p) => p === "cred" || p.startsWith("settings") || p.startsWith("events")}
      >
        {report && report.token !== "none" ? (
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" className="size-4 accent-primary" checked={alsoToken} onChange={(e) => setAlsoToken(e.target.checked)} />
            Also forget the 1Password service-account token ({report.token})
          </label>
        ) : null}
      </ConfirmDialog>
    </div>
  );
}

// ----- the picker --------------------------------------------------------------

type Step = { kind: "provider" } | { kind: "op" } | { kind: "fields"; provider: CredProvider } | { kind: "manual" } | { kind: "done"; result: CredSetResult };

// A refusal that carried the key's public half: the remote said no, and the
// owner's next step is to add that key to it.
interface Refused {
  error: string;
  why: string;
  fix: string;
  public_key?: string;
  fingerprint?: string;
  probes?: CredProbe[];
}

export function CredentialPicker({ open, onOpenChange, payload }: { open: boolean; onOpenChange: (o: boolean) => void; payload: CredPayload }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90vh] gap-0 overflow-y-auto p-0 sm:max-w-xl">
        <PickerBody payload={payload} close={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}

// The picker's state lives here, inside the dialog's content, so it starts
// over every time the dialog opens and is gone when it closes.
function PickerBody({ payload, close }: { payload: CredPayload; close: () => void }) {
  const [step, setStep] = useState<Step>({ kind: "provider" });
  const [reference, setReference] = useState("");
  const [probe, setProbe] = useState(true);
  const [saving, setSaving] = useState(false);
  const [refused, setRefused] = useState<Refused | null>(null);
  // The report a write just returned wins until the next GET arrives.
  const [rep, setRep] = useState<{ base: CredReport; value: CredReport }>({ base: payload.report, value: payload.report });
  let report = rep.value;
  if (rep.base !== payload.report) {
    report = payload.report;
    setRep({ base: payload.report, value: payload.report });
  }
  const setReport = (value: CredReport) => setRep({ base: payload.report, value });

  const save = async (ref: string) => {
    setSaving(true);
    setRefused(null);
    try {
      const r = await writes.credSet(ref.trim(), probe);
      setReport(r.report);
      refreshAfterWrite();
      toast.success("Push key saved" + (r.result.probed ? " - every remote answered." : "."));
      setStep({ kind: "done", result: r.result });
    } catch (e) {
      if (e instanceof ApiError && (e.body.public_key || e.body.probes)) {
        setRefused({ error: e.message, why: e.why, fix: e.fix, public_key: e.body.public_key as string | undefined, fingerprint: e.body.fingerprint as string | undefined, probes: e.body.probes as CredProbe[] | undefined });
      } else showApiError(e, "The key was not saved");
    } finally {
      setSaving(false);
    }
  };

  const back = () => {
    setRefused(null);
    setReference("");
    setStep({ kind: "provider" });
  };
  const title = step.kind === "done" ? "Push key saved" : step.kind === "provider" ? "Choose the daemon's push key" : step.kind === "op" ? "1Password" : step.kind === "manual" ? "A reference by hand" : step.provider.title;

  return (
    <>
      <DialogHeader className="border-b px-5 py-4 text-left">
        <div className="flex items-center gap-2">
          {step.kind !== "provider" && step.kind !== "done" ? (
            <Button size="icon-sm" variant="ghost" onClick={back} aria-label="Back to the providers">
              <ArrowLeftIcon />
            </Button>
          ) : null}
          <DialogTitle className="text-base">{title}</DialogTitle>
        </div>
        <DialogDescription>
          {step.kind === "done"
            ? "The reference is in the store; the key itself stays where it lives and exists on disk only for the seconds of a push."
            : "A reference to where the key lives - never the key. Before saving, the hub reads it once and asks every project's remote whether it may push."}
        </DialogDescription>
      </DialogHeader>
      <div className="grid gap-4 px-5 py-4">
        {step.kind === "provider" ? <ProviderStep payload={{ ...payload, report }} onPick={(p) => setStep(p.id === "op" ? { kind: "op" } : { kind: "fields", provider: p })} onManual={() => setStep({ kind: "manual" })} /> : null}
        {step.kind === "op" ? <OpStep report={report} onReport={setReport} reference={reference} onReference={setReference} /> : null}
        {step.kind === "fields" ? <FieldsStep provider={step.provider} reference={reference} onReference={setReference} /> : null}
        {step.kind === "manual" ? (
          <div className="grid gap-1.5">
            <Label htmlFor="cred-ref">Reference</Label>
            <Input id="cred-ref" value={reference} onChange={(e) => setReference(e.target.value)} placeholder={payload.providers[0]?.example} autoFocus spellCheck={false} className="font-mono text-[12.5px]" />
            <p className="text-xs text-muted-foreground anywhere">{payload.shapes}</p>
          </div>
        ) : null}
        {step.kind === "done" ? <DoneStep result={step.result} report={report} /> : null}
        {refused ? <RefusedNotice r={refused} /> : null}
      </div>
      {step.kind === "fields" || step.kind === "manual" || (step.kind === "op" && report.token !== "none") ? (
        <DialogFooter className="sticky bottom-0 flex-col gap-2 border-t bg-background px-5 py-3 sm:flex-row sm:items-center sm:justify-between">
          <label className="flex items-center gap-2 text-xs text-muted-foreground">
            <input type="checkbox" className="size-3.5 accent-primary" checked={probe} onChange={(e) => setProbe(e.target.checked)} />
            Ask every remote first
          </label>
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="sm" onClick={close}>
              Cancel
            </Button>
            <Button size="sm" disabled={saving || !reference.trim()} onClick={() => void save(reference)}>
              {saving ? <Loader2Icon className="animate-spin" /> : <ShieldCheckIcon />} {saving ? (probe ? "Asking the remotes…" : "Saving…") : probe ? "Test and save" : "Save"}
            </Button>
          </div>
        </DialogFooter>
      ) : step.kind === "done" ? (
        <DialogFooter className="border-t px-5 py-3">
          <Button size="sm" onClick={close}>
            Done
          </Button>
        </DialogFooter>
      ) : null}
    </>
  );
}

function ProviderStep({ payload, onPick, onManual }: { payload: CredPayload; onPick: (p: CredProvider) => void; onManual: () => void }) {
  return (
    <>
      <div className="grid gap-2">
        {payload.providers.map((p) => (
          <button
            key={p.id}
            type="button"
            disabled={!p.available}
            onClick={() => onPick(p)}
            className={cn(
              "flex items-start gap-3 rounded-lg border px-3.5 py-3 text-left transition-colors",
              p.available ? "hover:border-primary/50 hover:bg-muted/60 focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring" : "opacity-60",
            )}
          >
            <span className={cn("mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-md", p.id === "op" ? "bg-info/10 text-info" : p.id === "keychain" ? "bg-accent-2/10 text-accent-2" : "bg-primary/10 text-primary")} aria-hidden>
              {p.id === "op" ? <LockKeyholeIcon className="size-4" /> : <KeyRoundIcon className="size-4" />}
            </span>
            <span className="min-w-0">
              <span className="flex flex-wrap items-baseline gap-x-2">
                <span className="text-sm font-semibold">{p.title}</span>
                {!p.available ? <span className="text-xs text-warn">{p.unavailable || "not available on this machine"}</span> : p.id === "op" && payload.report.token !== "none" ? <span className="text-xs text-ok">connected</span> : null}
              </span>
              <span className="block text-[13px] leading-snug text-muted-foreground anywhere">{p.blurb}</span>
            </span>
          </button>
        ))}
      </div>
      <button type="button" onClick={onManual} className="flex items-center gap-1.5 self-start text-xs font-medium text-primary hover:underline">
        <PenLineIcon className="size-3.5" /> Type a reference by hand
      </button>
    </>
  );
}

// 1Password: the token once (kept by the hub machine), then a vault, then
// the SSH Key items of that vault. Picking one fills the reference.
function OpStep({ report, onReport, reference, onReference }: { report: CredReport; onReport: (r: CredReport) => void; reference: string; onReference: (r: string) => void }) {
  const connected = report.token !== "none";
  const [token, setToken] = useState("");
  const [retoken, setRetoken] = useState(false);
  const [busy, setBusy] = useState(false);
  // null: loading. Reloaded when the connection changes (a new token).
  const [vaults, setVaults] = useState<CredVault[] | null>(null);
  const [vault, setVault] = useState("");
  const [loaded, setLoaded] = useState<{ vault: string; items: CredItem[] } | null>(null);
  const [problem, setProblem] = useState<{ error: string; why: string; fix: string } | null>(null);
  const items = loaded && loaded.vault === vault ? loaded.items : null;

  const connect = async () => {
    setBusy(true);
    try {
      const r = await writes.credToken(token);
      setToken("");
      setRetoken(false);
      setVaults(null);
      setVault("");
      onReport(r.report);
      refreshAfterWrite();
      toast.success("Token kept in " + r.stored_in + ".");
    } catch (e) {
      showApiError(e, "The token was not stored");
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    if (!connected || retoken) return;
    let cancelled = false;
    writes
      .credVaults()
      .then((r) => {
        if (cancelled) return;
        setVaults(r.vaults);
        setProblem(null);
        if (r.vaults.length === 1) setVault(r.vaults[0]!.id);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setVaults([]);
        setProblem(e instanceof ApiError ? { error: e.message, why: e.why, fix: e.fix } : { error: String(e), why: "", fix: "" });
      });
    return () => {
      cancelled = true;
    };
  }, [connected, retoken]);

  useEffect(() => {
    if (!vault) return;
    let cancelled = false;
    writes
      .credItems(vault)
      .then((r) => !cancelled && setLoaded({ vault, items: r.items }))
      .catch((e: unknown) => {
        if (cancelled) return;
        setLoaded({ vault, items: [] });
        showApiError(e, "Could not list the vault");
      });
    return () => {
      cancelled = true;
    };
  }, [vault]);

  if (!connected || retoken)
    return (
      <div className="grid gap-3">
        <Notice tone="info">
          The daemon reads the key through a <b>service account</b>, so no one has to click in 1Password at push time. Make one in 1Password → Developer → Service Accounts with read access to the vault that holds the key, and paste its token here once; this machine keeps it{report.token !== "none" ? "" : " in its keychain or a 0600 file"} and the console never shows it again.
        </Notice>
        <div className="grid gap-1.5">
          <Label htmlFor="cred-token">Service-account token</Label>
          <div className="flex gap-2">
            <Input id="cred-token" type="password" autoComplete="off" spellCheck={false} value={token} onChange={(e) => setToken(e.target.value)} placeholder="ops_…" autoFocus className="font-mono text-[12.5px]" onKeyDown={(e) => e.key === "Enter" && token.trim() && void connect()} />
            <Button size="sm" className="h-9" disabled={busy || !token.trim()} onClick={() => void connect()}>
              {busy ? <Loader2Icon className="animate-spin" /> : <LockKeyholeIcon />} Connect
            </Button>
            {retoken ? (
              <Button size="sm" variant="ghost" className="h-9" onClick={() => setRetoken(false)}>
                Keep the current one
              </Button>
            ) : null}
          </div>
        </div>
      </div>
    );

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <span>
          <CheckIcon className="mr-1 inline size-3.5 text-ok" aria-hidden />
          Connected - the token is in {report.token}.
        </span>
        <button type="button" className="font-medium text-primary hover:underline" onClick={() => setRetoken(true)}>
          Use a different token
        </button>
      </div>
      {problem ? (
        <Notice tone="bad">
          <b>{problem.error}</b>
          {problem.why ? <span className="block">{problem.why}</span> : null}
          {problem.fix ? <span className="block text-muted-foreground">{problem.fix}</span> : null}
        </Notice>
      ) : null}
      <div className="grid gap-1.5">
        <Label htmlFor="cred-vault">Vault</Label>
        {vaults === null ? (
          <Skeleton className="h-9 w-full" />
        ) : (
          <Select value={vault} onValueChange={setVault} disabled={!vaults.length}>
            <SelectTrigger id="cred-vault" className="w-full">
              <SelectValue placeholder={vaults.length ? "Pick the vault that holds the key" : "The service account sees no vault"} />
            </SelectTrigger>
            <SelectContent>
              {vaults.map((v) => (
                <SelectItem key={v.id} value={v.id}>
                  {v.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>
      {vault ? (
        <div className="grid gap-1.5">
          <Label>SSH keys in this vault</Label>
          {items === null ? (
            <div className="grid gap-2">
              <Skeleton className="h-12 w-full" />
              <Skeleton className="h-12 w-full" />
            </div>
          ) : items.length === 0 ? (
            <p className="rounded-lg border border-dashed px-3 py-4 text-center text-[13px] text-muted-foreground">
              No SSH Key item here. In 1Password, add an item of type <b>SSH Key</b> to this vault (generate one or import the daemon&apos;s), then come back.
            </p>
          ) : (
            <div role="radiogroup" className="grid gap-1.5">
              {items.map((it) => {
                const on = reference === it.reference;
                return (
                  <button
                    key={it.id}
                    type="button"
                    role="radio"
                    aria-checked={on}
                    onClick={() => onReference(it.reference)}
                    className={cn("flex items-center gap-3 rounded-lg border px-3 py-2 text-left transition-colors hover:bg-muted/60", on && "border-primary bg-primary/5 ring-1 ring-primary/40")}
                  >
                    <span className={cn("flex size-4 shrink-0 items-center justify-center rounded-full border", on ? "border-primary bg-primary text-primary-foreground" : "border-input")} aria-hidden>
                      {on ? <CheckIcon className="size-3" /> : null}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block text-sm font-medium">{it.title}</span>
                      <span className="block text-xs text-muted-foreground">
                        {it.updated_at ? (
                          <>
                            updated <When d={it.updated_at} />
                            {" · "}
                          </>
                        ) : null}
                        <Mono className="text-[11px]">{it.reference}</Mono>
                      </span>
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </div>
      ) : null}
    </div>
  );
}

// Keychain and key file: a field or two make the reference; the file
// provider can also mint a key of the daemon's own.
function FieldsStep({ provider, reference, onReference }: { provider: CredProvider; reference: string; onReference: (r: string) => void }) {
  const [values, setValues] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [generated, setGenerated] = useState<{ public_key: string; fingerprint: string; path: string; existed: boolean } | null>(null);
  const compose = (v: Record<string, string>) => {
    if (provider.id === "file") return v.path?.trim() ? "file://" + v.path.trim().replace(/^file:\/\//, "") : "";
    const parts = provider.fields.map((f) => (v[f.name] ?? "").trim());
    return parts.every(Boolean) ? provider.id + "://" + parts.join("/") : "";
  };
  const update = (name: string, value: string) => {
    const next = { ...values, [name]: value };
    setValues(next);
    onReference(compose(next));
  };
  const generate = async (replace: boolean) => {
    setBusy(true);
    try {
      const r = await writes.credGenerate(replace);
      setGenerated(r.generated);
      update("path", r.generated.path);
      toast.success(r.generated.existed ? "The daemon already has a key - here it is." : "A new ed25519 key was made for the daemon.");
    } catch (e) {
      showApiError(e, "No key was made");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="grid gap-3">
      <p className="text-[13px] leading-snug text-muted-foreground anywhere">{provider.blurb}</p>
      {provider.fields.map((f) => (
        <div key={f.name} className="grid gap-1.5">
          <Label htmlFor={"cred-" + f.name}>{f.label}</Label>
          <Input id={"cred-" + f.name} value={values[f.name] ?? ""} onChange={(e) => update(f.name, e.target.value)} placeholder={f.placeholder} spellCheck={false} autoFocus={f === provider.fields[0]} className="font-mono text-[12.5px]" />
        </div>
      ))}
      {reference ? (
        <p className="text-xs text-muted-foreground">
          Reference: <Mono>{reference}</Mono>
        </p>
      ) : null}
      {provider.generate ? (
        <div className="grid gap-2 rounded-lg border bg-muted/30 px-3.5 py-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-sm font-medium">No key yet? Let PULSE make one for the daemon.</span>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => void generate(false)}>
              {busy ? <Loader2Icon className="animate-spin" /> : <KeyRoundIcon />} {generated ? "Show it again" : "Generate a key"}
            </Button>
          </div>
          {generated ? (
            <>
              <PublicKey publicKey={generated.public_key} fingerprint={generated.fingerprint} />
              <p className="text-xs text-muted-foreground anywhere">
                Add this public key to each repository as a <b>deploy key with write access</b> (GitHub: Settings → Deploy keys), then <b>Test and save</b>. Private half: <Mono>{generated.path}</Mono>, mode 0600.
                {generated.existed ? (
                  <>
                    {" "}
                    <button type="button" className="font-medium text-primary hover:underline" onClick={() => void generate(true)}>
                      Replace it with a fresh key
                    </button>
                    .
                  </>
                ) : null}
              </p>
            </>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function PublicKey({ publicKey, fingerprint }: { publicKey: string; fingerprint?: string }) {
  return (
    <div className="grid gap-1.5">
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs font-medium text-muted-foreground">Public key{fingerprint ? <> · <Mono className="text-[11px]">{fingerprint}</Mono></> : null}</span>
        <Button size="xs" variant="outline" onClick={() => copy(publicKey, "public key")}>
          <CopyIcon /> Copy
        </Button>
      </div>
      <pre className="overflow-x-auto rounded-md border bg-background px-3 py-2 font-mono text-[11.5px] leading-snug whitespace-pre-wrap anywhere">{publicKey}</pre>
    </div>
  );
}

function RefusedNotice({ r }: { r: Refused }) {
  return (
    <Notice tone="bad" className="items-stretch">
      <div className="grid gap-2">
        <div>
          <b>{r.error}</b>
          {r.why && !r.probes?.length ? <span className="block text-[13px]">{r.why}</span> : null}
          {r.fix ? <span className="block text-[13px] text-muted-foreground">{r.fix}</span> : null}
        </div>
        {r.probes?.length ? <Probes probes={r.probes} /> : null}
        {r.public_key ? (
          <>
            <PublicKey publicKey={r.public_key} fingerprint={r.fingerprint} />
            <span className="text-xs text-muted-foreground">Nothing was saved. Add this public key to every remote that refused (a deploy key with write access), then try again.</span>
          </>
        ) : null}
      </div>
    </Notice>
  );
}

function Probes({ probes }: { probes: CredProbe[] }) {
  return (
    <ul className="grid gap-1 text-[13px]">
      {probes.map((p) => (
        <li key={p.project + "/" + p.branch} className="flex items-start gap-2">
          {p.ok ? <CheckIcon className="mt-0.5 size-3.5 shrink-0 text-ok" aria-hidden /> : p.skipped ? <span className="mt-0.5 size-3.5 shrink-0 text-center text-muted-foreground" aria-hidden>–</span> : <XIcon className="mt-0.5 size-3.5 shrink-0 text-bad" aria-hidden />}
          <span className="min-w-0 anywhere">
            <b>{p.project}</b>
            {p.remote ? <Mono className="ml-1.5 text-[11px] text-muted-foreground">{p.remote}</Mono> : null}
            <span className="block text-xs text-muted-foreground">{p.ok ? "answers this key" : p.skipped ? p.skipped + " - not asked" : p.why || "refused"}</span>
          </span>
        </li>
      ))}
    </ul>
  );
}

function DoneStep({ result, report }: { result: CredSetResult; report: CredReport }) {
  const pushOff = (report.landing_push || result.landing_push) === "off";
  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        <CheckIcon className="size-4 text-ok" aria-hidden />
        <span>
          Key read from <b>{result.source}</b>
        </span>
        <Mono className="text-[11px] text-muted-foreground">{result.fingerprint}</Mono>
      </div>
      {result.probed ? <Probes probes={result.probes} /> : <p className="text-[13px] text-muted-foreground">Saved without asking the remotes.</p>}
      <PublicKey publicKey={result.public_key} fingerprint={result.fingerprint} />
      {pushOff ? (
        <Notice tone="info">
          Pushing after a landing is <b>off</b>: the daemon lands but does not push. Set <b>Push after landing</b> to <Mono>main</Mono> in the row above this one to use the key.
        </Notice>
      ) : null}
    </div>
  );
}
