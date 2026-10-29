// The state behind the restart dialog, as a pure module so node tests can run
// it straight from the source like daemon-control-state.ts (the tsx test
// loader resolves imports relative to the importing file, so a `.tsx` under
// test cannot reach `@/`). `restart-now.tsx` renders what these return; the
// wording branches - drain by default, hold on demand - are pinned here.

// The "N run(s) in flight" phrase, or null when nothing runs. Callers pass
// the live count (the daemon controls) or the count the pending update
// drains on (the settings Running page).
export function restartRunsText(inflight: number): string | null {
  return inflight > 0 ? `${inflight} run${inflight === 1 ? "" : "s"} in flight` : null;
}

// The button names the count so the owner sees what a restart waits on; the
// settings strip renders it as text when the strip already said it.
export function restartButtonLabel(pending: boolean, inflight: number): string {
  if (pending) return "Ask again";
  const runs = restartRunsText(inflight);
  return runs ? `Restart · ${runs}` : "Restart now";
}

export function restartDialogTitle(holdNow: boolean): string {
  return holdNow ? "Restart PULSE now, holding its runs?" : "Restart PULSE?";
}

// The first sentence of the dialog body. With runs in flight it restates the
// count, capitalized, then the drain/hold detail follows in the component.
// With nothing in flight it promises the swap at once.
export function restartDialogLead(inflight: number): string {
  const runs = restartRunsText(inflight);
  return runs
    ? runs.charAt(0).toUpperCase() + runs.slice(1)
    : "Nothing is in flight, so the runner swaps in the new build at once.";
}

// The drain detail when the owner left the default: the runner starts nothing
// new and restarts once the runs in flight end, so nothing is held or lost.
export function restartDrainDetail(): string {
  return "The runner stops starting runs and restarts once they end (up to the drain budget), so nothing is held or lost.";
}

// The hold detail when the owner asked for the swap now: each run saves where
// it is and the new runner picks it back up; what cannot hold is stopped and
// its job goes back to the queue.
export function restartHoldDetail(): string {
  return "Each is asked to hold: it saves where it is, and the new runner picks it back up with the same agent, workspace and conversation. Anything that can't hold within two minutes is stopped and its job goes back to the queue.";
}

// The console's own conversations never ride on the restart.
export function restartConsoleDetail(): string {
  return "Conversations in this console carry on by themselves once the new console is up.";
}

// A runner older than this console may not hear the ask at all; then the
// restart waits for its run to finish, the way it always did.
export function restartOlderRunnerDetail(): string {
  return "A runner older than this console may not hear the ask; then it restarts once its run finishes, as before.";
}

export function restartActionLabel(holdNow: boolean): string {
  return holdNow ? "Hold them now" : "Restart when they finish";
}

export function restartSuccessMessage(holdNow: boolean): string {
  return holdNow
    ? "Asked. The runner holds its runs and restarts now."
    : "Asked. The runner starts nothing new and restarts once its runs finish.";
}

// The "hold them and restart now" checkbox only appears with runs in flight.
export function restartHoldShown(inflight: number): boolean {
  return restartRunsText(inflight) !== null;
}

export function restartHoldCheckboxLabel(inflight: number): string {
  return `Hold the ${inflight} run${inflight === 1 ? "" : "s"} and restart now instead`;
}
