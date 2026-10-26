import assert from "node:assert/strict";
import { test } from "node:test";
import { messageStatus } from "./message-status.ts";

const owner = { role: "user" };
const answer = { role: "answer" };

test("an owner message with nothing after it is sent", () => {
  assert.equal(messageStatus(owner, []), "sent");
});

test("an owner message followed only by more owner messages is sent", () => {
  assert.equal(messageStatus(owner, [owner, answer]), "sent");
});

test("an owner message followed by an agent reply is answered", () => {
  assert.equal(messageStatus(owner, [{ role: "assistant" }]), "answered");
});

test("a question or notice from the agent also counts as answered", () => {
  assert.equal(messageStatus(owner, [{ role: "question" }]), "answered");
  assert.equal(messageStatus(owner, [{ role: "notice" }]), "answered");
});

test("the answer may arrive after other owner messages", () => {
  assert.equal(messageStatus(owner, [answer, { role: "assistant" }]), "answered");
});
