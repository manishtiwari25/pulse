import type { OwnerQuestion } from "./api";

// The inbox order for questions that wait for the owner: the evolution
// loop's stop questions first (the loop is halted until one is answered),
// live threads before stale ones, newest first within each group.
export function sortOwnerQuestions<T extends Pick<OwnerQuestion, "stop" | "stale" | "date" | "id">>(qs: readonly T[]): T[] {
  return [...qs].sort((a, b) => Number(!!b.stop) - Number(!!a.stop) || Number(!!a.stale) - Number(!!b.stale) || (a.date < b.date ? 1 : a.date > b.date ? -1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}
