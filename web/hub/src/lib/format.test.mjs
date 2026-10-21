import assert from "node:assert/strict";
import { test } from "node:test";
import { firstSentence, sentence } from "./format.ts";

test("sentence trims whitespace and capitalizes the first letter", () => {
  assert.equal(sentence("  which option should agents use?  "), "Which option should agents use?");
  assert.equal(sentence("already capitalized."), "Already capitalized.");
  assert.equal(sentence(""), "");
});

test("firstSentence returns the first complete sentence when present", () => {
  assert.equal(firstSentence("first reason. second detail."), "First reason.");
  assert.equal(firstSentence("why did it stop? because a gate failed."), "Why did it stop?");
  assert.equal(firstSentence("no punctuation yet"), "No punctuation yet");
});
