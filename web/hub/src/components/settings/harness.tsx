"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { ArrowRightIcon, CheckIcon, ClipboardIcon, ExternalLinkIcon, LogOutIcon, RefreshCwIcon, RotateCcwIcon } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { Disclosure, ErrorNotice, Kv, Notice } from "@/components/pulse/panel";
import { Mono, ToneBadge } from "@/components/pulse/who";
import { SettingKey } from "@/components/settings/setting-row";
import { errorText, writes, type HarnessDeviceFlow, type HarnessLogin, type HarnessModel, type HarnessModelsResponse, type HarnessPrompt, type HarnessProviderInfo, type HarnessStatus, type Runner, type Setting } from "@/lib/api";
import { fmtDate, num, rel } from "@/lib/format";
import { hrefs } from "@/lib/nav";
import { settingHelp, settingLabel } from "@/lib/setting-labels";
import { invalidate, useApi, useNow } from "@/lib/use-api";
import { cn } from "@/lib/utils";

// The Harness section of Settings (the runner points people here as
// "Settings -> Harness"): sign in to the model provider, pick the models,
// cap one run, and - folded away - the system prompt, the tool switches and
// the transcript sizes. These controls save as soon as they change.

export const HARNESS_SETTING_KEYS = ["provider", "harness.max_steps", "harness.task_max_steps", "harness.step_timeout", "harness.output_limit", "harness.max_cost", "harness.keep_observations", "harness.context_window", "harness.compact_at", "harness.reasoning", "harness.system_prompt", "harness.instructions", "harness.mcp_tools", "harness.skills", "harness.hooks", "harness.subagents", "harness.memory", "harness.teach", "harness.web_search", "harness.fetch_allow", "harness.allow_tools", "harness.deny_tools", "worker_model", "light_model", "measure_model", "ask.model"];
const SAME_MODEL = "__same__";

// How the chosen provider signs in: who stands behind its device code
// (GitHub for Copilot and GitHub Models, Microsoft for Azure OpenAI) and
// what its pasted secret is called. The hub says which flows a provider
// offers; this only picks the words.
interface SignInWords {
  idp: string;
  tokenLabel: string;
  tokenSummary: string;
  placeholder: string;
  modelsHint: string;
}

export function signInWords(p: HarnessProviderInfo | undefined): SignInWords {
  const name = p?.name ?? "copilot";
  const github = name === "copilot" || name === "github-models";
  const idp = github ? "GitHub" : name === "azure-openai" ? "Microsoft" : p?.label || "the provider";
  const tokenLabel = github ? "GitHub token" : "API key";
  const placeholder = name === "copilot" ? "gho_…" : name === "github-models" ? "github_pat_…" : name === "azure-openai" ? "key or bearer token" : "sk-…";
  const device = p ? p.device_login : true;
  return {
    idp,
    tokenLabel,
    tokenSummary: github ? "Use a GitHub token instead" : "Use an API key instead",
    placeholder,
    modelsHint: device ? "Sign in with " + idp : "Paste " + (github ? "a GitHub token" : "an API key"),
  };
}

function setting(rows: Setting[] | undefined, name: string): Setting | undefined {
  return rows?.find((s) => s.name === name);
}

function modelLabel(m: HarnessModel) {
  return m.name || m.id;
}

function saveSetting(name: string, value: string) {
  return writes.setSetting(name, value).then(() => {
    invalidate("harness");
    invalidate("settings");
  });
}

function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  return (
    <Button
      type="button"
      size="sm"
      variant="outline"
      onClick={() =>
        navigator.clipboard
          .writeText(text)
          .then(() => toast.success("Copied."))
          .catch(() => toast.error("Couldn't copy. Select the text and copy it yourself."))
      }
    >
      <ClipboardIcon /> {label}
    </Button>
  );
}

function sourceText(source: string): { label: string; borrowed: boolean } {
  const s = source.trim().toLowerCase();
  if (s === "device login") return { label: "Signed in here", borrowed: false };
  if (s === "token login") return { label: "With a pasted token", borrowed: false };
  if (s === "environment") return { label: "Borrowed from this machine's environment", borrowed: true };
  if (s === "editor login") return { label: "Borrowed from your editor", borrowed: true };
  return { label: source || "Unknown", borrowed: false };
}

// One block of the card: a small heading, a one-line note, then its controls.
function Block({ title, note, children, right, className }: { title: React.ReactNode; note?: React.ReactNode; children: React.ReactNode; right?: React.ReactNode; className?: string }) {
  return (
    <div className={cn("grid gap-3 border-t px-4 py-4 first:border-t-0 sm:px-5", className)}>
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <div className="min-w-0">
          <h3 className="text-[15px] font-semibold leading-snug">{title}</h3>
          {note ? <p className="mt-0.5 max-w-2xl text-sm leading-relaxed text-muted-foreground anywhere">{note}</p> : null}
        </div>
        {right ? <div className="flex flex-wrap items-center gap-2">{right}</div> : null}
      </div>
      {children}
    </div>
  );
}

function KeyName({ name }: { name: string }) {
  return <SettingKey name={name} />;
}

// The runner's login sentences name this very section ("Settings -> Harness")
// and offer a command. Here they read as plain words; the command goes to
// the "Prefer the terminal?" fold.
const LOGIN_CLI = /[,;]?\s*or run\s+`?(pulse harness login[^`]*?)`?\s*\.?$/i;

export function loginCommand(text: string): string {
  return LOGIN_CLI.exec(text.trim())?.[1]?.trim() ?? "";
}

export function plainLogin(text: string, signedIn: boolean): string {
  const t = text.trim();
  if (!t) return "";
  if (/wait(s|ing)? for .*login/i.test(t)) return signedIn ? "Agents on this machine were waiting for a sign-in. They pick up work again on their next pass." : "Agents on this machine wait until you sign in again.";
  let say = t
    .replace(LOGIN_CLI, "")
    .replace(/\s+in\s+(the console\s+)?\(?Settings\s*->\s*Harness\)?/gi, " below")
    .replace(/\s*\(?Settings\s*->\s*Harness\)?/gi, "")
    .trim();
  say = say.charAt(0).toUpperCase() + say.slice(1);
  return /[.!?]$/.test(say) ? say : say + ".";
}

function LoginHealth({ login, runner, expired }: { login?: HarnessLogin; runner?: Runner; expired: boolean }) {
  const source = sourceText(login?.source || "");
  const fresh = !!login?.logged_in && !expired;
  const hold = runner?.hold ? <Notice tone="warn">{plainLogin(runner.hold, fresh)}</Notice> : null;
  if (!login?.logged_in) {
    return (
      <div className="grid gap-2">
        <Notice tone="warn">{login?.detail ? plainLogin(login.detail, false) : "Not signed in. Agents can't run until you sign in."}</Notice>
        {hold}
      </div>
    );
  }
  const rows: [React.ReactNode, React.ReactNode][] = [["How", source.label]];
  if (login.plan) rows.push(["Plan", login.plan.charAt(0).toUpperCase() + login.plan.slice(1)]);
  if (login.expires_at && !expired)
    rows.push([
      "Expires",
      <span key="exp" title={fmtDate(login.expires_at)}>
        {rel(login.expires_at)}
      </span>,
    ]);
  return (
    <div className="grid gap-3">
      <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        <span className={cn("size-2 shrink-0 rounded-full", expired ? "bg-warn" : "bg-ok")} aria-hidden />
        {expired ? (
          <span>
            Your sign-in as <b className="font-semibold">{login.user}</b> expired{" "}
            <span title={fmtDate(login.expires_at)}>{rel(login.expires_at)}</span>. Sign in again so agents can run.
          </span>
        ) : (
          <span>
            Signed in as <b className="font-semibold">{login.user}</b>
          </span>
        )}
      </p>
      <Kv rows={rows} className="rounded-xl bg-muted/50 px-4 py-3" />
      {source.borrowed ? <p className="text-[13px] text-muted-foreground">A borrowed sign-in isn&apos;t shared with your other machines.</p> : null}
      {login.detail ? <Notice>{plainLogin(login.detail, fresh)}</Notice> : null}
      {hold}
    </div>
  );
}

function DeviceLogin({ onDone, idp, label }: { onDone: () => void; idp: string; label?: string }) {
  const button = label || "Sign in with " + idp;
  const [flow, setFlow] = useState<HarnessDeviceFlow | null>(null);
  const [status, setStatus] = useState<"idle" | "starting" | "pending" | "expired" | "denied">("idle");
  const [pollDelay, setPollDelay] = useState(5);
  const [polls, setPolls] = useState(0);
  const [error, setError] = useState("");
  const [startedAt, setStartedAt] = useState(0);
  const now = useNow(1000);
  const remaining = flow ? Math.max(0, Math.ceil((startedAt + flow.expires_in * 1000 - now) / 1000)) : 0;

  const start = () => {
    setStatus("starting");
    setError("");
    writes
      .harnessLogin()
      .then((r) => {
        setFlow(r);
        setStartedAt(Date.now());
        setPollDelay(Math.max(1, r.interval || 5));
        setPolls(0);
        setStatus("pending");
      })
      .catch((e: unknown) => {
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
        setStatus("idle");
      });
  };

  useEffect(() => {
    if (!flow || status !== "pending") return;
    const id = setTimeout(() => {
      writes
        .harnessLoginPoll(flow.device_code)
        .then((r) => {
          setPolls((n) => n + 1);
          if (r.status === "slow_down") setPollDelay((n) => n + 5);
          if (r.status === "expired") setStatus("expired");
          if (r.status === "denied") setStatus("denied");
          if (r.status === "complete" && r.login) {
            toast.success("Signed in as " + r.login.user + ".");
            invalidate("harness");
            invalidate("harness/models");
            setFlow(null);
            setStatus("idle");
            onDone();
          }
        })
        .catch((e: unknown) => {
          const t = errorText(e);
          setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
          setStatus("idle");
        });
    }, pollDelay * 1000);
    return () => clearTimeout(id);
  }, [flow, status, pollDelay, polls, onDone]);

  if (!flow) {
    return (
      <div className="grid gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" onClick={start} disabled={status === "starting"}>
            {status === "starting" ? <RefreshCwIcon className="animate-spin" /> : null} {button}
          </Button>
          <span className="text-[13px] text-muted-foreground">You&apos;ll get a short code to enter on {idp}.</span>
        </div>
        {error ? <p className="text-[13px] text-bad anywhere">{error}</p> : null}
      </div>
    );
  }

  return (
    <div className="grid gap-3 rounded-xl border bg-muted/40 p-4">
      <ol className="grid gap-1 text-sm text-muted-foreground">
        <li>1. Open {idp} and enter this code.</li>
        <li>2. Approve the sign-in. This page notices by itself.</li>
      </ol>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="font-mono text-2xl font-semibold tracking-[0.12em] text-foreground" aria-label={"Code " + flow.user_code.split("").join(" ")}>
          {flow.user_code}
        </p>
        <div className="flex flex-wrap gap-2">
          <CopyButton text={flow.user_code} label="Copy code" />
          <Button asChild size="sm">
            <a href={flow.verification_uri} target="_blank" rel="noreferrer">
              Open {idp} <ExternalLinkIcon />
            </a>
          </Button>
        </div>
      </div>
      <p className="text-[13px] text-muted-foreground" aria-live="polite">
        {status === "pending" ? "Waiting for " + idp + ". " : status === "expired" ? "The code expired. Try again for a new one. " : "The sign-in was turned down on " + idp + ". "}
        {remaining > 0 && status === "pending" ? "The code works for " + num(remaining) + " more seconds." : ""}
      </p>
      {status === "expired" || status === "denied" ? (
        <div className="flex gap-2">
          <Button size="sm" onClick={start}>
            Try again
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setFlow(null)}>
            Cancel
          </Button>
        </div>
      ) : (
        <Button size="sm" variant="ghost" className="w-fit" onClick={() => setFlow(null)}>
          Cancel
        </Button>
      )}
    </div>
  );
}

function TokenLogin({ onDone, words, only }: { onDone: () => void; words: SignInWords; only: boolean }) {
  const [open, setOpen] = useState(false);
  const [token, setToken] = useState("");
  const [error, setError] = useState("");
  const submit = () => {
    setError("");
    const id = toast.loading("Signing in…");
    writes
      .harnessLoginToken(token)
      .then((r) => {
        toast.success("Signed in as " + r.login.user + ".", { id });
        setToken("");
        setOpen(false);
        invalidate("harness");
        invalidate("harness/models");
        onDone();
      })
      .catch((e: unknown) => {
        toast.dismiss(id);
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
      });
  };
  const form = (
    <div className="flex flex-wrap items-end gap-2">
      <div className="grid min-w-52 gap-1.5">
        <Label htmlFor="harness-token">{words.tokenLabel}</Label>
        <Input id="harness-token" type="password" value={token} onChange={(e) => setToken(e.target.value)} onKeyDown={(e) => e.key === "Enter" && token.trim() && submit()} placeholder={words.placeholder} />
      </div>
      <Button type="button" onClick={submit} disabled={!token.trim()}>
        Sign in
      </Button>
      {error ? <p className="basis-full text-[13px] text-bad anywhere">{error}</p> : null}
    </div>
  );
  // A provider without a device code signs in with its key alone, so the
  // form stands open instead of folding behind "instead".
  if (only) {
    return (
      <div className="grid gap-2">
        <p className="text-[13px] text-muted-foreground">Paste the {words.tokenLabel === "API key" ? "API key" : "token"} from your {words.idp} account. It stays on this machine.</p>
        {form}
      </div>
    );
  }
  return (
    <Disclosure summary={words.tokenSummary} open={open} onToggle={setOpen}>
      {form}
    </Disclosure>
  );
}

function ModelSelect({ name, label, note, current, models, disabled }: { name: "worker_model" | "light_model" | "measure_model" | "ask.model"; label: string; note: string; current: string; models: HarnessModel[]; disabled?: boolean }) {
  const [custom, setCustom] = useState(current);
  const ids = new Set(models.map((m) => m.id));
  // The light, measuring and Ask models fall back to another one when empty.
  const sameAs = name === "light_model" || name === "measure_model" ? "Same as the main model" : name === "ask.model" ? "Same as the light model" : null;
  const selectValue = sameAs && !current ? SAME_MODEL : ids.has(current) ? current : "__custom__";
  // The closed picker shows the model's name only; ids and badges stay in the list.
  const known = models.find((m) => m.id === current);
  const shown = selectValue === SAME_MODEL && sameAs ? sameAs : known ? modelLabel(known) : current || "The default model";
  // Typing an id is the rare path: folded unless the current model isn't in the list, or there is no list.
  const [typing, setTyping] = useState<boolean | null>(null);
  const typeOpen = typing ?? ((!!current && !known) || models.length === 0);
  const save = (value: string) => {
    const v = value === SAME_MODEL ? "" : value;
    const id = toast.loading("Saving the " + label.toLowerCase() + "…");
    saveSetting(name, v)
      .then(() => toast.success(label + " saved.", { id }))
      .catch((e: unknown) => {
        toast.dismiss(id);
        const t = errorText(e);
        toast.error(t.error, { description: [t.why, t.fix].filter(Boolean).join(" ") });
      });
  };
  return (
    <div className="grid min-w-0 content-start gap-1.5">
      <Label id={"model-" + name}>{label}</Label>
      <p className="text-[13px] leading-snug text-muted-foreground">{note}</p>
      <Select value={selectValue} onValueChange={save} disabled={disabled}>
        <SelectTrigger className="w-full" aria-labelledby={"model-" + name}>
          <SelectValue>{shown}</SelectValue>
        </SelectTrigger>
        <SelectContent>
          {sameAs ? <SelectItem value={SAME_MODEL}>{sameAs}</SelectItem> : null}
          {models.map((m) => (
            <SelectItem key={m.id} value={m.id}>
              <span className="flex min-w-0 items-center gap-1.5">
                <span>{modelLabel(m)}</span>
                <span className="text-xs text-muted-foreground">{m.id}</span>
                {m.premium > 1 ? <Badge variant="outline" title="Uses more model credits per call">{"×" + m.premium}</Badge> : null}
                {m.preview ? <Badge variant="outline">Preview</Badge> : null}
                {m.default ? <Badge variant="secondary">Default</Badge> : null}
              </span>
            </SelectItem>
          ))}
          {!ids.has(current) && current ? <SelectItem value="__custom__">{current}</SelectItem> : null}
        </SelectContent>
      </Select>
      <Disclosure summary="Type a model id" open={typeOpen} onToggle={setTyping} contentClassName="grid gap-1.5">
        <Input
          aria-label={label + ": type a model id"}
          value={custom}
          onChange={(e) => setCustom(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") save(custom.trim());
            if (e.key === "Escape") setCustom(current);
          }}
          onBlur={() => {
            if (custom.trim() !== current) save(custom.trim());
          }}
          placeholder="For example claude-sonnet-4.5"
          className="w-full font-mono text-[13px]"
          disabled={disabled}
        />
        <KeyName name={name} />
      </Disclosure>
    </div>
  );
}

// An on/off choice setting of the harness (repository instructions, MCP
// tools) as a switch that saves as soon as it flips.
function OnOffSwitch({ s, onSave }: { s: Setting; onSave: (name: string, value: string) => void }) {
  const on = s.value !== "off";
  return (
    <div className="grid gap-1.5">
      <div className="flex min-h-9 flex-wrap items-center gap-x-3 gap-y-1.5">
        <Switch id={"sw-" + s.name} checked={on} onCheckedChange={(v) => onSave(s.name, v ? "on" : "off")} aria-label={settingLabel(s.name)} />
        <label htmlFor={"sw-" + s.name} className="text-sm font-medium text-foreground">
          {settingLabel(s.name)}
        </label>
        <KeyName name={s.name} />
      </div>
      <p className="text-[13px] leading-snug text-muted-foreground">{settingHelp(s.name, s.description)}</p>
    </div>
  );
}

function ChoiceSelect({ s, onSave, labels }: { s: Setting; onSave: (name: string, value: string) => void; labels?: Record<string, string> }) {
  const choices = s.choices ?? [];
  const value = choices.includes(s.value) ? s.value : s.default;
  return (
    <div className="grid gap-1.5">
      <Label id={"choice-" + s.name}>{settingLabel(s.name)}</Label>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <Select value={value} onValueChange={(v) => onSave(s.name, v)}>
          <SelectTrigger className="w-56 max-w-full" aria-labelledby={"choice-" + s.name}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {choices.map((c) => (
              <SelectItem key={c} value={c}>
                {labels?.[c] ?? c}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <KeyName name={s.name} />
      </div>
      <p className="text-[13px] leading-snug text-muted-foreground">{settingHelp(s.name, s.description)}</p>
    </div>
  );
}

const WEB_SEARCH_LABELS: Record<string, string> = {
  chat: "Conversations only",
  on: "Conversations and jobs",
  off: "Nowhere",
};

const REASONING_LABELS: Record<string, string> = {
  default: "Model default (send nothing)",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra high",
};

function LimitInput({ s }: { s: Setting }) {
  const [draftState, setDraftState] = useState({ name: s.name, value: s.value, draft: s.value });
  const [error, setError] = useState("");
  const draft = draftState.name === s.name && draftState.value === s.value ? draftState.draft : s.value;
  const setDraft = (value: string) => setDraftState({ name: s.name, value: s.value, draft: value });
  const save = () => {
    if (draft === s.value) return;
    if (s.kind === "number" ? !/^\d+(\.\d+)?$/.test(draft) : !/^\d+$/.test(draft)) {
      setError(s.kind === "number" ? "Enter a number." : "Enter digits only.");
      return;
    }
    if (s.min !== null && Number(draft) < s.min) {
      setError("Enter " + s.min + " or more.");
      return;
    }
    setError("");
    saveSetting(s.name, draft)
      .then(() => toast.success(settingLabel(s.name) + " saved."))
      .catch((e: unknown) => {
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
      });
  };
  return (
    <div className="grid min-w-0 content-start gap-1.5">
      <Label htmlFor={"harness-" + s.name}>{settingLabel(s.name)}</Label>
      <Input
        id={"harness-" + s.name}
        value={draft}
        inputMode={s.kind === "number" ? "decimal" : "numeric"}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          if (e.key === "Enter") save();
          if (e.key === "Escape") setDraft(s.value);
        }}
        aria-invalid={!!error}
        aria-describedby={"harness-" + s.name + "-help"}
        className="w-32 tnum"
      />
      <p id={"harness-" + s.name + "-help"} className={cn("text-[13px] leading-snug", error ? "text-bad" : "text-muted-foreground")}>
        {error || settingHelp(s.name, s.description)}
      </p>
      <KeyName name={s.name} />
    </div>
  );
}

// A free-text setting (the fetch tool's allowed hosts) that saves on blur
// or Enter, like a limit does.
function TextInput({ s, placeholder }: { s: Setting; placeholder?: string }) {
  const [draftState, setDraftState] = useState({ name: s.name, value: s.value, draft: s.value });
  const [error, setError] = useState("");
  const draft = draftState.name === s.name && draftState.value === s.value ? draftState.draft : s.value;
  const setDraft = (value: string) => setDraftState({ name: s.name, value: s.value, draft: value });
  const save = () => {
    if (draft.trim() === s.value.trim()) return;
    setError("");
    saveSetting(s.name, draft.trim())
      .then(() => toast.success(settingLabel(s.name) + " saved."))
      .catch((e: unknown) => {
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
      });
  };
  return (
    <div className="grid min-w-0 content-start gap-1.5">
      <Label htmlFor={"harness-" + s.name}>{settingLabel(s.name)}</Label>
      <Input
        id={"harness-" + s.name}
        value={draft}
        placeholder={placeholder}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          if (e.key === "Enter") save();
          if (e.key === "Escape") setDraft(s.value);
        }}
        aria-invalid={!!error}
        aria-describedby={"harness-" + s.name + "-help"}
        className="max-w-xl font-mono text-[13px]"
      />
      <p id={"harness-" + s.name + "-help"} className={cn("text-[13px] leading-snug", error ? "text-bad" : "text-muted-foreground")}>
        {error || settingHelp(s.name, s.description)}
      </p>
      <KeyName name={s.name} />
    </div>
  );
}

function PromptEditor() {
  const prompt = useApi<HarnessPrompt>("harness/prompt", { refresh: 30000 });
  const [draftState, setDraftState] = useState({ marker: "", draft: "" });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const d = prompt.data;
  const marker = d ? (d.custom ? "custom:" + d.template : "default:" + d.default) : "";
  const draft = draftState.marker === marker ? draftState.draft : d?.custom ? d.template : d?.default || "";
  const dirty = !!d && draft !== (d.custom ? d.template : d.default);
  const save = (value: string, label: string) => {
    setSaving(true);
    setError("");
    writes
      .setSetting("harness.system_prompt", value)
      .then(() => {
        toast.success(label);
        invalidate("harness");
        invalidate("harness/prompt");
        invalidate("settings");
        return prompt.reload();
      })
      .catch((e: unknown) => {
        const t = errorText(e);
        setError([t.error, t.why, t.fix].filter(Boolean).join(" "));
      })
      .finally(() => setSaving(false));
  };
  return (
    <Disclosure
      variant="row"
      className="border-t px-3 py-2 sm:px-4"
      contentClassName="mt-1 grid gap-3 pb-3"
      summary={<RowSummary title="System prompt" note="The instructions every run starts with. Most people never change them." badge={d ? <ToneBadge tone={d.custom ? "accent" : "neutral"}>{d.custom ? "Your own" : "Built-in"}</ToneBadge> : null} />}
    >
      <>
        {prompt.error && !d ? <ErrorNotice error={prompt.error} onRetry={() => void prompt.reload()} /> : null}
        {d ? (
          <>
            <p className="text-[13px] text-muted-foreground">PULSE fills in the parts in double braces for each run. Leave it as it is to keep the built-in text.</p>
            <Textarea
              aria-label="System prompt template"
              value={draft}
              onChange={(e) => {
                setDraftState({ marker, draft: e.target.value });
                setError("");
              }}
              rows={10}
              spellCheck={false}
              className={cn("min-h-48 font-mono text-[12.5px] leading-relaxed", !d.custom && !dirty && "bg-muted/40 text-muted-foreground")}
            />
            <div className="flex flex-wrap items-center gap-2">
              <Button type="button" size="sm" onClick={() => save(draft, "System prompt saved.")} disabled={saving || !dirty}>
                {saving ? <RefreshCwIcon className="animate-spin" /> : <CheckIcon />} Save
              </Button>
              <Button type="button" size="sm" variant="outline" onClick={() => save("", "System prompt reset to the built-in text.")} disabled={saving || (!d.custom && !dirty)}>
                <RotateCcwIcon /> Reset to built-in
              </Button>
              {dirty ? (
                <Button type="button" size="sm" variant="ghost" onClick={() => setDraftState({ marker, draft: d.custom ? d.template : d.default })} disabled={saving}>
                  Undo changes
                </Button>
              ) : null}
              <KeyName name="harness.system_prompt" />
            </div>
            {d.error || error ? <Notice tone="bad">{error || d.error}</Notice> : null}
            <Disclosure summary="Preview the prompt as a run sees it">
              <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-muted/50 p-3 font-mono text-xs leading-relaxed">{d.rendered || "(empty)"}</pre>
            </Disclosure>
          </>
        ) : !prompt.error ? (
          <p className="text-sm text-muted-foreground">Loading the prompt…</p>
        ) : null}
      </>
    </Disclosure>
  );
}

// The title and note of a folded row inside the card, as the summary of a Disclosure.
function RowSummary({ title, note, badge }: { title: string; note: string; badge?: React.ReactNode }) {
  return (
    <span className="block min-w-0">
      <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className="text-[15px] font-semibold leading-snug">{title}</span>
        {badge}
      </span>
      <span className="mt-0.5 block text-sm font-normal leading-relaxed text-muted-foreground">{note}</span>
    </span>
  );
}

export function HarnessSettingsCard({ className }: { className?: string }) {
  const h = useApi<HarnessStatus>("harness", { refresh: 15000 });
  const runner = useApi<Runner>("runner", { refresh: 15000 });
  const models = useApi<HarnessModelsResponse>("harness/models", { refresh: 60000 });
  const d = h.data;
  const rows = d?.settings;
  const provider = setting(rows, "provider");
  const budgets = ["harness.max_steps", "harness.task_max_steps", "minutes_per_job", "harness.max_cost"].map((k) => setting(rows, k)).filter((s): s is Setting => !!s);
  const limits = ["harness.step_timeout", "harness.output_limit", "harness.keep_observations", "harness.context_window", "harness.compact_at"].map((k) => setting(rows, k)).filter((s): s is Setting => !!s);
  const toggles = ["harness.instructions", "harness.mcp_tools", "harness.skills", "harness.hooks", "harness.subagents", "harness.memory", "harness.teach"].map((k) => setting(rows, k)).filter((s): s is Setting => !!s);
  const fetchAllow = setting(rows, "harness.fetch_allow");
  const webSearch = setting(rows, "harness.web_search");
  const allowTools = setting(rows, "harness.allow_tools");
  const denyTools = setting(rows, "harness.deny_tools");
  const reasoning = setting(rows, "harness.reasoning");
  const words = signInWords(d?.provider);
  const now = useNow();
  const login = d?.provider.login;
  const signedIn = login?.logged_in === true;
  // An expired sign-in is signed in on paper only: offer to sign in again.
  const expired = signedIn && !!login?.expires_at && new Date(login.expires_at).getTime() < now;
  const commands = [...new Set([loginCommand(login?.detail || ""), loginCommand(runner.data?.hold || "")].filter(Boolean))];
  const currentModels = models.data?.models ?? [];
  const modelError = models.error && !models.data ? models.error : "";

  const saveChoice = (name: string, value: string) => {
    const id = toast.loading("Saving " + settingLabel(name).toLowerCase() + "…");
    saveSetting(name, value)
      .then(() => toast.success(settingLabel(name) + " saved.", { id }))
      .catch((e: unknown) => {
        toast.dismiss(id);
        const t = errorText(e);
        toast.error(t.error, { description: [t.why, t.fix].filter(Boolean).join(" ") });
      });
  };

  return (
    <div className={cn("min-w-0 overflow-hidden rounded-2xl border bg-card", className)}>
      {h.error && !d ? (
        <div className="p-4 sm:p-5">
          <ErrorNotice error={h.error} onRetry={() => void h.reload()} />
        </div>
      ) : null}
      {!d && !h.error ? <p className="px-4 py-5 text-sm text-muted-foreground sm:px-5">Loading sign-in and models…</p> : null}
      {d ? (
        <>
          <Block
            title="Sign-in"
            note="Agents call the model provider with this sign-in. It stays on this machine."
            right={
              signedIn ? (
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    writes.harnessLogout().then((r) => {
                      toast.success(r.login.logged_in ? "Still signed in." : "Signed out.");
                      invalidate("harness");
                      invalidate("harness/models");
                    })
                  }
                >
                  <LogOutIcon /> Sign out
                </Button>
              ) : null
            }
          >
            <div className="grid gap-1.5">
              <Label id="harness-provider-label">Model provider</Label>
              {provider ? (
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                  <Select value={provider.value || d.provider.name} onValueChange={(v) => saveChoice("provider", v)}>
                    <SelectTrigger className="w-64 max-w-full" aria-labelledby="harness-provider-label">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {d.providers.map((p) => (
                        <SelectItem key={p.name} value={p.name}>
                          {p.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <KeyName name="provider" />
                </div>
              ) : (
                <span className="text-sm">{d.provider.label}</span>
              )}
            </div>
            <LoginHealth login={login} runner={runner.data} expired={expired} />
            {!signedIn || expired ? (
              <div className="grid gap-3">
                {d.provider.device_login ? <DeviceLogin onDone={() => void h.reload()} idp={words.idp} label={expired ? "Sign in again" : undefined} /> : null}
                {d.provider.token_login ? <TokenLogin onDone={() => void h.reload()} words={words} only={!d.provider.device_login} /> : null}
              </div>
            ) : null}
            {commands.length ? (
              <Disclosure summary="Prefer the terminal?">
                <p className="text-[13px] text-muted-foreground anywhere">
                  Sign in from a terminal on this machine:{" "}
                  {commands.map((c, i) => (
                    <span key={c}>
                      {i ? " or " : null}
                      <Mono>{c}</Mono>
                    </span>
                  ))}
                </p>
              </Disclosure>
            ) : null}
          </Block>

          <Block title="Models" note="Which models agents use. A change saves as soon as you pick it.">
            {modelError ? (
              <Notice tone={modelError.includes("not signed in") ? "warn" : "bad"}>
                <span className="grid gap-1">
                  <span>The model list didn&apos;t load: {modelError}</span>
                  <span>
                    Main model now: <Mono>{d.models.worker || "the default"}</Mono>. Light model now: <Mono>{d.models.light || "same as the main model"}</Mono>.
                  </span>
                  <span>
                    {words.modelsHint} to see the {d.provider.label} models, or type the exact model id under Type a model id.
                  </span>
                </span>
              </Notice>
            ) : null}
            <div className="grid gap-5 md:grid-cols-2 xl:grid-cols-4">
              <ModelSelect name="worker_model" label="Main model" note="For the coding work itself." current={d.models.worker} models={currentModels} disabled={false} />
              <ModelSelect name="light_model" label="Light model" note="For smaller jobs like planning, answering and voting." current={d.models.light} models={currentModels} disabled={false} />
              <ModelSelect name="measure_model" label="Measuring model" note="For measuring baselines, experiments and merges; a cheap, steady model keeps the loop honest and affordable." current={d.models.measure ?? ""} models={currentModels} disabled={false} />
              <ModelSelect name="ask.model" label="Ask model" note="For your questions in Ask; a fast model keeps answers quick." current={d.models.ask ?? ""} models={currentModels} disabled={false} />
            </div>
          </Block>

          <Block title="Limits per run" note="A run stops when it reaches any of these. Each one saves when you leave the field.">
            <div className="grid gap-5 sm:grid-cols-3">{budgets.map((s) => <LimitInput key={s.name} s={s} />)}</div>
          </Block>

          <PromptEditor />

          <Disclosure
            variant="row"
            className="border-t px-3 py-2 sm:px-4"
            contentClassName="mt-1 grid gap-5 pb-3"
            summary={<RowSummary title="Tools and transcript size" note="What an agent may use while it works, and how much command output a run keeps." />}
          >
            <>
              {toggles.length ? (
                <div className="grid gap-4 sm:grid-cols-2">
                  {toggles.map((s) => (
                    <OnOffSwitch key={s.name} s={s} onSave={saveChoice} />
                  ))}
                </div>
              ) : null}
              {fetchAllow ? <TextInput s={fetchAllow} placeholder="docs.rs, *.rust-lang.org" /> : null}
              {webSearch ? <ChoiceSelect s={webSearch} onSave={saveChoice} labels={WEB_SEARCH_LABELS} /> : null}
              {denyTools ? <TextInput s={denyTools} placeholder="bash(rm -rf *), read(.env*), write(*.lock)" /> : null}
              {allowTools ? <TextInput s={allowTools} placeholder="bash(git *), bash(cargo *), edit(src/*)" /> : null}
              {reasoning ? <ChoiceSelect s={reasoning} onSave={saveChoice} labels={REASONING_LABELS} /> : null}
              <div className="grid gap-5 sm:grid-cols-2 lg:grid-cols-4">{limits.map((s) => <LimitInput key={s.name} s={s} />)}</div>
            </>
          </Disclosure>

          <div className="flex flex-wrap items-center justify-between gap-3 border-t bg-muted/40 px-4 py-3 sm:px-5">
            <p className="min-w-0 text-[13px] leading-relaxed text-muted-foreground">
              Today: {num(d.sessions.today)} {d.sessions.today === 1 ? "run" : "runs"}, {num(d.sessions.submitted_today)} finished, {num(d.sessions.failed_today)} failed. {num(d.sessions.running)} running now. {num(d.sessions.cost_today)} model credits used.
            </p>
            <Button asChild size="sm" variant="outline">
              <Link href={hrefs.harness()}>
                Open Runs <ArrowRightIcon />
              </Link>
            </Button>
          </div>
        </>
      ) : null}
    </div>
  );
}

export function isHarnessSetting(name: string): boolean {
  return HARNESS_SETTING_KEYS.includes(name);
}
