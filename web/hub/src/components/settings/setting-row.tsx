"use client";

import { useState } from "react";
import { EraserIcon, EyeOffIcon, RotateCcwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Clamp } from "@/components/pulse/clamp";
import { ConfirmDialog } from "@/components/pulse/confirm";
import { Mono, ToneBadge, When } from "@/components/pulse/who";
import { cn } from "@/lib/utils";
import { writes, type Setting } from "@/lib/api";
import { settingLabel } from "@/lib/setting-labels";

export function hint(s: Setting): string {
  if (s.secret) return "owner-private: stored, but never shown here, to agents or in events";
  switch (s.kind) {
    case "count":
      return "a whole number" + (s.min ? ", at least " + s.min : "");
    case "count-or-none":
      return "a whole number, or none";
    case "credits":
      return "a whole number, at least " + (s.min ?? 30);
    case "number":
      return "a number, decimals allowed" + (s.min !== undefined && s.min !== null ? ", at least " + s.min : "");
    case "fraction":
      return "a fraction such as 2/3";
    case "command":
      return "a program name or path";
    case "identifier":
      return "a name, id or path - never a secret";
    case "reference":
      return "where the key lives - op://VAULT/ITEM/FIELD, keychain://SERVICE/ACCOUNT or file:///PATH - never the key itself";
    default:
      return "";
  }
}

// The left column of a settings row: the human name first, the key in small
// type beside it with the last change, then the description folded to one
// line (ADR-042 D5). A row at its default says so with one quiet badge.
export function RowHead({ s, title, badge, children }: { s: Setting; title?: React.ReactNode; badge?: React.ReactNode; children?: React.ReactNode }) {
  // "init" wrote the legacy rows at store creation: that is not a change either.
  const unset = s.updated_by === "default" || s.updated_by === "init" || !s.updated_at;
  const atDefault = s.value === s.default;
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
        <span className="text-sm font-semibold">{title ?? settingLabel(s.name)}</span>
        <Mono className="text-[11.5px] text-muted-foreground">{s.name}</Mono>
        {s.secret ? (
          <ToneBadge tone="warn" className="gap-1 self-center">
            <EyeOffIcon className="size-3" /> secret
          </ToneBadge>
        ) : null}
        {badge}
        {unset || atDefault ? (
          <span className="self-center rounded-full border px-1.5 text-[10.5px] font-medium uppercase tracking-wider text-muted-foreground" title={"default " + (s.default || "(empty)")}>
            default
          </span>
        ) : null}
        {!unset ? (
          <span className="text-xs text-muted-foreground">
            changed <When d={s.updated_at} />
            {s.updated_by ? <> by <Mono className="text-[11px]">{s.updated_by}</Mono></> : null}
            {!atDefault && s.default ? <> · default <Mono className="text-[11px]">{s.default}</Mono></> : null}
          </span>
        ) : null}
      </div>
      {children ?? <Clamp>{s.description}</Clamp>}
    </div>
  );
}

// One setting row, two columns from md up: what it is on the left, the
// editor for its kind with Save right next to it on the right; the hint for
// the kind appears while the owner types. On phones the editor stacks under
// the text. Saving is confirmed, then POST /api/v1/settings.
type SettingLabels = { on?: string; off?: string; choices?: Record<string, string> };

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
  const wide = s.kind === "command" || s.kind === "text" || s.kind === "identifier" || s.kind === "reference";
  const label = title ?? settingLabel(s.name);
  const h = hint(s);
  return (
    <div className="grid gap-3 px-4 py-3 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
      <RowHead s={s} title={label} />
      <div className="flex min-w-0 flex-col gap-1.5 md:items-end">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
        {isSwitch ? (
          <div className="flex min-h-8 items-center gap-2">
            <Switch id={"sw-" + s.name} checked={isOn} disabled={!canEdit} onCheckedChange={(v) => setPending(v ? "1" : "0")} aria-label={s.name} />
            <label htmlFor={"sw-" + s.name} className={cn("text-[13px]", isOn ? (isStop ? "font-semibold text-bad" : "text-ok") : isStop ? "text-ok" : "text-muted-foreground")}>
              {isOn ? labels?.on ?? "on" : labels?.off ?? "off"}
            </label>
          </div>
        ) : (
          <>
            {isChoice ? (
              <Select value={draft} onValueChange={setDraft} disabled={!canEdit}>
                <SelectTrigger aria-label={s.name} className="h-8 w-56">
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
                inputMode={s.kind === "count" || s.kind === "credits" ? "numeric" : s.kind === "number" ? "decimal" : undefined}
                aria-label={s.name}
                value={secret && !dirty ? "" : draft}
                disabled={!canEdit}
                placeholder={secret ? (s.value ? "(set - value never shown)" : "(not set)") : s.default || "empty"}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  // preventDefault: the dialog focuses its button at once and the same keypress would activate it.
                  if (e.key === "Enter" && dirty && canEdit) {
                    e.preventDefault();
                    setPending(draft);
                  }
                  if (e.key === "Escape") setDraft(s.value);
                }}
                className={cn("h-8", s.kind === "count" || s.kind === "count-or-none" || s.kind === "credits" || s.kind === "number" ? "w-32 shrink-0 tnum" : wide ? "w-64 max-w-full" : "w-32 shrink-0")}
              />
            )}
            <Button size="sm" variant={dirty ? "default" : "outline"} disabled={!canEdit || !dirty} onClick={() => setPending(draft)}>
              Save
            </Button>
            {dirty ? (
              <Button size="sm" variant="ghost" onClick={() => setDraft(s.value)} aria-label="Undo">
                <RotateCcwIcon /> Undo
              </Button>
            ) : null}
            {secret && s.value && !dirty && canEdit ? (
              <Button size="sm" variant="ghost" onClick={() => setPending("")} aria-label={"Clear " + s.name}>
                <EraserIcon /> Clear
              </Button>
            ) : null}
          </>
        )}
        </div>
        {h && (dirty || secret) ? <p className="text-xs leading-snug text-muted-foreground anywhere">{h}</p> : null}
      </div>
      <ConfirmDialog
        open={pending !== null}
        onOpenChange={(o) => !o && setPending(null)}
        title={<>Change {label}?</>}
        description={
          <span>
            <Mono>{s.name}</Mono>{": "}
            {isSwitch ? (pending === "1" ? "turn it on" : "turn it off") : secret ? (
              pending ? (s.value ? "replace the private value (never shown here)" : "set the private value (never shown here)") : "clear the private value"
            ) : <>
              from <Mono>{s.value || "(empty)"}</Mono> to <Mono>{pending || "(empty)"}</Mono>
            </>}
            . Saved to the store at once, recorded with who made it; the daemon reads it on its next loop.
          </span>
        }
        action="Save"
        success={settingLabel(s.name) + " saved."}
        onConfirm={() => writes.setSetting(s.name, pending ?? "")}
        invalidates={(p) => p.startsWith("settings") || p.startsWith("runner") || p.startsWith("compute") || p.startsWith("events") || p.startsWith("org")}
      />
    </div>
  );
}
