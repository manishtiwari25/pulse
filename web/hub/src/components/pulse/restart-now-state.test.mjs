import assert from "node:assert/strict";
import { test } from "node:test";
import {
  restartActionLabel,
  restartButtonLabel,
  restartDialogLead,
  restartDialogTitle,
  restartHoldCheckboxLabel,
  restartHoldShown,
  restartRunsText,
  restartSuccessMessage,
} from "./restart-now-state.ts";

test("restartRunsText counts the runs in flight, singular and plural", () => {
  assert.equal(restartRunsText(0), null);
  assert.equal(restartRunsText(1), "1 run in flight");
  assert.equal(restartRunsText(3), "3 runs in flight");
});

test("restartButtonLabel says Ask again once an ask is pending, else names the count", () => {
  assert.equal(restartButtonLabel(true, 3), "Ask again");
  assert.equal(restartButtonLabel(true, 0), "Ask again");
  assert.equal(restartButtonLabel(false, 3), "Restart · 3 runs in flight");
  assert.equal(restartButtonLabel(false, 1), "Restart · 1 run in flight");
  assert.equal(restartButtonLabel(false, 0), "Restart now");
});

test("restartDialogLead capitalizes the count and never claims an empty runner when runs are in flight", () => {
  assert.equal(restartDialogLead(3), "3 runs in flight");
  assert.equal(restartDialogLead(1), "1 run in flight");
  assert.match(restartDialogLead(0), /^Nothing is in flight/);
  assert.doesNotMatch(restartDialogLead(2), /Nothing is in flight/);
});

test("the hold path is the exception: title, action and success switch together", () => {
  assert.equal(restartDialogTitle(false), "Restart PULSE?");
  assert.equal(restartDialogTitle(true), "Restart PULSE now, holding its runs?");
  assert.equal(restartActionLabel(false), "Restart when they finish");
  assert.equal(restartActionLabel(true), "Hold them now");
  assert.match(restartSuccessMessage(false), /restarts once its runs finish/);
  assert.match(restartSuccessMessage(true), /holds its runs and restarts now/);
});

test("the hold checkbox only appears with runs in flight and counts them", () => {
  assert.equal(restartHoldShown(0), false);
  assert.equal(restartHoldShown(2), true);
  assert.equal(restartHoldCheckboxLabel(1), "Hold the 1 run and restart now instead");
  assert.equal(restartHoldCheckboxLabel(4), "Hold the 4 runs and restart now instead");
});
