import assert from "node:assert/strict";
import { test } from "node:test";
import { ASK_HISTORY_CAP, ASK_SEND_TURNS, answerMessage, askStorageKey, factsOnlyHint, elapsedWord, historyForRequest, readHistory, trimHistory, writeHistory } from "./ask.ts";

function memoryStorage(initial = {}) {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => void map.set(k, v),
    removeItem: (k) => void map.delete(k),
    map,
  };
}

const user = (content, at = "2026-10-12T10:00:00Z") => ({ role: "user", content, at });
const answer = { answer: "One job is blocked [job-1].", project: "demo", model: "m", provider: "copilot", facts_only: false, note: "", sources: [{ id: "job-1", source: "job", kind: "task", summary: "Fix it", date: "2026-10-12", project: "demo", thread: "" }], elapsed_ms: 1234, usage: { prompt_tokens: 1, completion_tokens: 1, cost: 0 } };

test("one conversation per project, and one for all projects", () => {
  assert.equal(askStorageKey("demo"), "pulse.ask.demo");
  assert.equal(askStorageKey(""), "pulse.ask.all");
  assert.equal(askStorageKey("  "), "pulse.ask.all");
});

test("history is capped and the model sees only the last turns, words only", () => {
  const many = Array.from({ length: 30 }, (_, i) => user("q" + i));
  const kept = trimHistory(many);
  assert.equal(kept.length, ASK_HISTORY_CAP);
  assert.equal(kept[0].content, "q10");
  const sent = historyForRequest([...many, answerMessage(answer, "2026-10-12T10:01:00Z")]);
  assert.equal(sent.length, ASK_SEND_TURNS);
  assert.deepEqual(sent[sent.length - 1], { role: "assistant", content: "One job is blocked [job-1]." });
  assert.equal("sources" in sent[sent.length - 1], false);
  assert.deepEqual(historyForRequest([user("  "), user("real")]), [{ role: "user", content: "real" }]);
});

test("an answer becomes a message that keeps its sources and note", () => {
  const m = answerMessage({ ...answer, facts_only: true, note: "not signed in", sources: undefined }, "2026-10-12T10:01:00Z");
  assert.equal(m.role, "assistant");
  assert.equal(m.facts_only, true);
  assert.equal(m.note, "not signed in");
  assert.deepEqual(m.sources, []);
  assert.equal(m.elapsed_ms, 1234);
});

test("facts-only hints say why and what to do", () => {
  assert.match(factsOnlyHint("not signed in"), /Sign in on the Runs page/);
  assert.match(factsOnlyHint("the model did not answer within 45 seconds"), /too slow/);
  assert.match(factsOnlyHint("the model provider \"nope\" did not open"), /provider did not open/);
  assert.match(factsOnlyHint("the model returned no words"), /No model answered \(the model returned no words\)/);
  assert.equal(elapsedWord(0), "");
  assert.equal(elapsedWord(640), "640 ms");
  assert.equal(elapsedWord(1234), "1.2 s");
});

test("history survives a round trip through storage and ignores junk", () => {
  const storage = memoryStorage({ "pulse.ask.demo": JSON.stringify([user("hi"), { role: "assistant", content: "x", at: "t", sources: [] }, { bogus: true }, "nope"]) });
  const read = readHistory(storage, "demo");
  assert.equal(read.length, 2);
  writeHistory(storage, "demo", [...read, user("more")]);
  assert.equal(readHistory(storage, "demo").length, 3);
  writeHistory(storage, "demo", []);
  assert.equal(storage.map.has("pulse.ask.demo"), false, "an empty history removes the key");
  assert.deepEqual(readHistory(memoryStorage({ "pulse.ask.all": "{not json" }), ""), []);
  assert.deepEqual(readHistory(null, "demo"), []);
});
