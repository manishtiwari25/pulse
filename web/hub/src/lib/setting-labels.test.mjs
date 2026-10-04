import assert from "node:assert/strict";
import { test } from "node:test";
import { settingHelp, settingLabel } from "./setting-labels.ts";

test("a curated key keeps its curated help", () => {
  assert.equal(settingHelp("stop", "the store's text"), "Pause all agent work on this machine until you turn it back on.");
});

test("an unclassified key shows the store's own description, not the generic blurb", () => {
  const help = settingHelp("daemon.new_cap", "hours a new cap waits before it applies");
  assert.equal(help, "Hours a new cap waits before it applies.");
  assert.ok(!help.startsWith("Controls "));
  assert.equal(settingLabel("daemon.new_cap"), "New cap");
});

test("the generic line is only the last resort", () => {
  assert.equal(settingHelp("daemon.new_cap", ""), "Controls new cap.");
  assert.equal(settingHelp("daemon.new_cap", "  "), "Controls new cap.");
});
