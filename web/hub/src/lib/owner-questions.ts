import type { OwnerQuestion } from "./api";

// The inbox order for questions that wait for the owner: the evolution
// loop's stop questions first (the loop is halted until one is answered),
// live threads before stale ones, newest first within each group.
export function sortOwnerQuestions<T extends Pick<OwnerQuestion, "stop" | "stale" | "date" | "id">>(qs: readonly T[]): T[] {
  return [...qs].sort((a, b) => Number(!!b.stop) - Number(!!a.stop) || Number(!!a.stale) - Number(!!b.stale) || (a.date < b.date ? 1 : a.date > b.date ? -1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

// The labels a post gives the owner's two buttons, from its own `Yes: ...`
// and `No: ...` lines (the security watcher writes "Yes: Upgrade it" /
// "No: Leave it to me"); plain Yes / No when the post names none. Only a
// short label counts: a sentence is the explanation under it, not a button.
export function decisionLabels(body: string | undefined, fallback: { yes: string; no: string } = { yes: "Yes", no: "No" }): { yes: string; no: string } {
  const pick = (word: "Yes" | "No"): string | null => {
    const m = new RegExp("^\\s*\\*{0,2}" + word + "\\*{0,2}:\\s*(.+?)\\s*$", "m").exec(body || "");
    const label = m ? m[1].replace(/[.!]+$/, "").trim() : "";
    return label && label.length <= 40 ? label : null;
  };
  return { yes: pick("Yes") ?? fallback.yes, no: pick("No") ?? fallback.no };
}

// What the card calls a post that waits for the owner, by its kind: a
// question is asked, a finding or a handoff is put to the owner for a call.
export function askTitle(kind: string | undefined, asker: string): string {
  switch (kind) {
    case "finding":
      return "Your call, from " + asker;
    case "handoff":
      return "Handed to you by " + asker;
    case "suggestion":
      return "Proposed to you by " + asker;
    default:
      return "Question from " + asker;
  }
}
