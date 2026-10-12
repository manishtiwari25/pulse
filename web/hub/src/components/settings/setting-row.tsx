"use client";

import { useState } from "react";
import { CirclePauseIcon, EraserIcon, EyeOffIcon, PlayIcon, RotateCcwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Clamp } from "@/components/pulse/clamp";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { Mono, ToneBadge, When } from "@/components/pulse/who";
import { cn } from "@/lib/utils";
import { writes, type Runner, type Setting } from "@/lib/api";
import { settingHelp, settingLabel } from "@/lib/setting-labels";
import { useApi } from "@/lib/use-api";

// What a value should look like, shown while the owner types.
export function hint(s: Setting): string {
  if (s.secret) return "Private: saved on this machine, never shown here, to agents or in the activity log.";
  // The store also sends "decimal", which the API type does not list yet.
  switch (s.kind as string) {
    case "count":
      return "A whole number" + (s.min ? ", at least " + s.min : "") + ".";
    case "count-or-none":
      return "A whole number, or none.";
    case "credits":
      return "A whole number, at least " + (s.min ?? 30) + ".";
    case "number":
    case "decimal":
      return "A number, decimals allowed" + (s.min !== undefined && s.min !== null ? ", at least " + s.min : "") + ".";
    case "fraction":
      return "A fraction such as 2/3.";
    case "command":
      return "A program name or path.";
    case "identifier":
      return "A name, id or path. Never a secret.";
    case "reference":
      return "Where the key lives (op://VAULT/ITEM/FIELD, keychain://SERVICE/ACCOUNT or file:///PATH), never the key itself.";
    default:
      return "";
  }
}

// Who changed a row, as the owner reads it: the console writes "human" or
// "owner" for you; anything else is an agent or a tool.
function changedBy(who: string): string {
  return who === "human" || who === "owner" ? "you" : who;
}

const PULSE_SETTING = "Type this name after pulse setting set to change it from a terminal";

// The quiet last line of a row: who changed it and when (nothing while it
// was never changed), the default when the value differs from it, then the
// raw key, labelled, for people who use the terminal.
export function RowMeta({ s, className }: { s: Setting; className?: string }) {
  // "init" wrote the legacy rows at store creation: that is not a change either.
  const unset = s.updated_by === "default" || s.updated_by === "init" || !s.updated_at;
  const atDefault = s.value === s.default;
  return (
    <div className={cn("flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs leading-relaxed text-muted-foreground", className)}>
      {unset ? null : (
        <span>
          Changed by {changedBy(s.updated_by || "someone")}, <When d={s.updated_at} />
        </span>
      )}
      {!atDefault && !s.secret ? (
        <span>
          Default: <span className="font-mono text-[12px] text-foreground/75">{s.default || "empty"}</span>
        </span>
      ) : null}
      <SettingKey name={s.name} />
    </div>
  );
}

// The raw key, labelled so it never reads as part of the sentence before it.
export function SettingKey({ name, className }: { name: string; className?: string }) {
  return (
    <span className={cn("min-w-0 text-xs text-muted-foreground anywhere", className)} title={PULSE_SETTING}>
      Key: <code className="font-mono text-[12px] text-muted-foreground/90">{name}</code>
    </span>
  );
}

// The left column of a settings row: the human name, one line of help
// (folded, with "More" when it runs longer - ADR-042 D5), then the quiet
// default/changed line with the key.
export function RowHead({ s, title, badge, children }: { s: Setting; title?: React.ReactNode; badge?: React.ReactNode; children?: React.ReactNode }) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
        <span className="text-[15px] font-medium leading-snug text-foreground anywhere">{title ?? settingLabel(s.name)}</span>
        {s.secret ? (
          <ToneBadge tone="neutral" className="gap-1" title="Saved on this machine, never shown here or to agents">
            <EyeOffIcon className="size-3" aria-hidden /> Private
          </ToneBadge>
        ) : null}
        {badge}
      </div>
      {children ?? <Clamp lines={2}>{settingHelp(s.name, s.description)}</Clamp>}
      <RowMeta s={s} className="mt-0.5" />
    </div>
  );
}

// One setting row, two columns from md up: what it is on the left, the
// editor for its kind with Save right next to it on the right; the hint for
// the kind appears while the owner types. On phones the editor stacks under
// the text. Saving is confirmed, then POST /api/v1/settings.
type SettingLabels = { on?: string; off?: string; choices?: Record<string, string> };

const INVALIDATES = (p: string) => p.startsWith("settings") || p.startsWith("runner") || p.startsWith("compute") || p.startsWith("events") || p.startsWith("org");
const SAVED_NOTE = "PULSE saves it right away and records that you changed it. The runner uses the new value from its next pass.";

export function SettingRow({ s, canEdit, labels, title }: { s: Setting; canEdit: boolean; labels?: SettingLabels; title?: React.ReactNode }) {
  const [draftState, setDraftState] = useState({ name: s.name, value: s.value, draft: s.value });
  const [pending, setPending] = useState<string | null>(null);
  let draft = draftState.draft;
  if (draftState.name !== s.name || draftState.value !== s.value) {
    draft = s.value;
    setDraftState({ name: s.name, value: s.value, draft: s.value });
  }
  const setDraft = (value: string) => setDraftState({ name: s.name, value: s.value, draft: value });
  const dirty = draft !== s.value;
  const isSwitch = s.kind === "switch";
  const isChoice = s.kind === "choice";
  const isOn = s.value === "1";
  const isStop = s.name === "stop";
  const secret = !!s.secret;
  const kind = s.kind as string;
  const numeric = kind === "count" || kind === "count-or-none" || kind === "credits" || kind === "number" || kind === "decimal";
  const wide = s.kind === "command" || s.kind === "text" || s.kind === "identifier" || s.kind === "reference";
  const label = title ?? settingLabel(s.name);
  const named = typeof label === "string" ? label : settingLabel(s.name);
  const h = hint(s);
  const show = (v: string) => labels?.choices?.[v] ?? (v || "empty");
  return (
    <div className="grid gap-3 px-4 py-4 sm:px-5 md:grid-cols-[minmax(0,1fr)_auto] md:items-start md:gap-x-8">
      <RowHead s={s} title={label} />
      <div className="flex min-w-0 flex-col gap-1.5 md:max-w-[22rem] md:items-end">
        <div className="flex min-w-0 flex-wrap items-center gap-2 md:justify-end">
          {isSwitch ? (
            <div className="flex min-h-9 items-center gap-2.5">
              <Switch id={"sw-" + s.name} checked={isOn} disabled={!canEdit} onCheckedChange={(v) => setPending(v ? "1" : "0")} aria-label={named} />
              <label htmlFor={"sw-" + s.name} className={cn("text-sm", isOn ? (isStop ? "font-semibold text-bad" : "font-medium text-foreground") : "text-muted-foreground")}>
                {isOn ? labels?.on ?? "On" : labels?.off ?? "Off"}
              </label>
            </div>
          ) : (
            <>
              {isChoice ? (
                <Select value={draft} onValueChange={setDraft} disabled={!canEdit}>
                  <SelectTrigger aria-label={named} className="h-9 w-64 max-w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {(s.choices || []).map((c) => (
                      <SelectItem key={c} value={c}>
                        {labels?.choices?.[c] ?? c}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <Input
                  type={secret ? "password" : "text"}
                  inputMode={s.kind === "count" || s.kind === "credits" ? "numeric" : kind === "number" || kind === "decimal" ? "decimal" : undefined}
                  aria-label={named}
                  value={secret && !dirty ? "" : draft}
                  disabled={!canEdit}
                  placeholder={secret ? (s.value ? "Set (never shown)" : "Not set") : s.default || "Empty"}
                  onChange={(e) => setDraft(e.target.value)}
                  onKeyDown={(e) => {
                    // preventDefault: the dialog focuses its button at once and the same keypress would activate it.
                    if (e.key === "Enter" && dirty && canEdit) {
                      e.preventDefault();
                      setPending(draft);
                    }
                    if (e.key === "Escape") setDraft(s.value);
                  }}
                  className={cn("h-9", numeric ? "w-28 shrink-0 tnum" : wide ? "w-64 max-w-full" : "w-32 shrink-0")}
                />
              )}
              <Button size="sm" className="h-9" variant={dirty ? "default" : "outline"} disabled={!canEdit || !dirty} onClick={() => setPending(draft)}>
                Save
              </Button>
              {dirty ? (
                <Button size="sm" className="h-9" variant="ghost" onClick={() => setDraft(s.value)} aria-label={"Undo the change to " + named}>
                  <RotateCcwIcon /> Undo
                </Button>
              ) : null}
              {secret && s.value && !dirty && canEdit ? (
                <Button size="sm" className="h-9" variant="ghost" onClick={() => setPending("")} aria-label={"Clear " + named}>
                  <EraserIcon /> Clear
                </Button>
              ) : null}
            </>
          )}
        </div>
        {h && (dirty || secret) ? <p className="text-xs leading-snug text-muted-foreground anywhere md:text-right">{h}</p> : null}
      </div>
      <ConfirmDialog
        open={pending !== null}
        onOpenChange={(o) => !o && setPending(null)}
        title={<>Change &ldquo;{named}&rdquo;?</>}
        description={
          <span className="grid gap-2">
            <span className="block">
              {isSwitch ? (
                <>Turn it {pending === "1" ? "on" : "off"}{(pending === "1" ? labels?.on : labels?.off) ? <>: {pending === "1" ? labels?.on : labels?.off}</> : null}.</>
              ) : secret ? (
                pending ? (s.value ? "Replace the private value. It is never shown here." : "Set the private value. It is never shown here.") : "Clear the private value."
              ) : (
                <>
                  From <Mono>{show(s.value)}</Mono> to <Mono>{show(pending ?? "")}</Mono>.
                </>
              )}
            </span>
            <span className="block">{SAVED_NOTE}</span>
            <span className="block text-xs text-muted-foreground">
              Setting <span className="font-mono">{s.name}</span>
            </span>
          </span>
        }
        action="Save"
        success={settingLabel(s.name) + " saved."}
        onConfirm={() => writes.setSetting(s.name, pending ?? "")}
        invalidates={INVALIDATES}
      />
    </div>
  );
}

// The emergency brake (the `stop` key): one sentence on what is happening
// now and one button that does the opposite. Same write as the switch it
// replaces - setSetting("stop", "1"|"0") behind a confirm.
export function PauseCard({ s, canEdit }: { s: Setting; canEdit: boolean }) {
  const [pending, setPending] = useState<string | null>(null);
  const paused = s.value === "1";
  // "Working normally" only while the runner checks in; with the runner off
  // (or not known yet) nothing is paused, but nothing is working either.
  const runner = useApi<Runner>("runner", { refresh: 30000 });
  const state = runner.data?.available ? runner.data.daemon.state : "";
  const live = state === "running" || state === "alive";
  const off = !!runner.data && !live;
  return (
    <div className={cn("flex min-w-0 flex-wrap items-start gap-4 rounded-2xl border bg-card p-4 sm:items-center sm:p-5", paused && "border-bad/40 bg-bad/[0.05]")}>
      <span className={cn("flex size-10 shrink-0 items-center justify-center rounded-full", paused ? "bg-bad/10 text-bad" : live ? "bg-ok/10 text-ok" : "bg-muted text-muted-foreground")} aria-hidden>
        {paused ? <CirclePauseIcon className="size-5" /> : <PlayIcon className="size-[18px]" />}
      </span>
      <div className="min-w-0 flex-1 basis-60">
        <p className="text-base font-semibold leading-snug text-foreground">{paused ? "All work is paused" : live ? "Agents are working normally" : "Nothing is paused"}</p>
        <p className="mt-0.5 max-w-2xl text-sm leading-relaxed text-muted-foreground">
          {paused
            ? "No new agent work starts on this machine until you resume. Nothing is lost while it waits."
            : off
              ? "The runner is off, so agents can't start new work right now anyway. Pausing keeps them from starting once it runs again."
              : "Your emergency brake. Pausing stops agents from starting new work on this machine; runs already going finish first."}
        </p>
        <RowMeta s={s} className="mt-1" />
      </div>
      {paused ? (
        <Button className="w-full sm:w-auto" disabled={!canEdit} onClick={() => setPending("0")}>
          <PlayIcon /> Resume work
        </Button>
      ) : (
        <Button variant="outline" className="w-full text-bad hover:text-bad sm:w-auto" disabled={!canEdit} onClick={() => setPending("1")}>
          <CirclePauseIcon /> Pause all work
        </Button>
      )}
      <ConfirmDialog
        open={pending !== null}
        onOpenChange={(o) => !o && setPending(null)}
        title={pending === "1" ? "Pause all work?" : "Resume work?"}
        description={
          <span className="grid gap-2">
            <span className="block">{pending === "1" ? "Agents stop starting new work on this machine. Runs already going finish first. You can resume at any time." : "Agents start picking up work again on their next pass."}</span>
            <span className="block text-xs text-muted-foreground">
              Setting <span className="font-mono">{s.name}</span> to {pending === "1" ? "1" : "0"}
            </span>
          </span>
        }
        action={pending === "1" ? "Pause all work" : "Resume work"}
        destructive={pending === "1"}
        success={pending === "1" ? "All work paused. Runs already going will finish." : "Work resumed."}
        onConfirm={() => writes.setSetting(s.name, pending ?? "")}
        invalidates={INVALIDATES}
      />
    </div>
  );
}
