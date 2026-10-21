import assert from "node:assert/strict";
import { test } from "node:test";
import { ApiError, api } from "./api.ts";

function fakeFetch(status, body) {
  return async () =>
    new Response(body, {
      status,
      headers: { "Content-Type": "application/json" },
    });
}

test("api surfaces a 200 with a non-JSON body as an ApiError instead of data", async () => {
  const saved = globalThis.fetch;
  globalThis.fetch = fakeFetch(200, "<html>proxy down</html>");
  try {
    await assert.rejects(api("org"), (e) => {
      assert.ok(e instanceof ApiError);
      assert.equal(e.status, 200);
      assert.equal(e.message, "unreadable response");
      assert.match(e.why, /HTTP 200/);
      assert.match(e.why, /not JSON/);
      assert.ok(e.fix.length > 0);
      return true;
    });
  } finally {
    globalThis.fetch = saved;
  }
});

test("api keeps the status and explains a failed reply whose body is not JSON", async () => {
  const saved = globalThis.fetch;
  globalThis.fetch = fakeFetch(502, "Bad Gateway");
  try {
    await assert.rejects(api("org"), (e) => {
      assert.ok(e instanceof ApiError);
      assert.equal(e.status, 502);
      assert.equal(e.message, "HTTP 502");
      assert.match(e.why, /not JSON/);
      return true;
    });
  } finally {
    globalThis.fetch = saved;
  }
});

test("api still returns parsed JSON on success", async () => {
  const saved = globalThis.fetch;
  globalThis.fetch = fakeFetch(200, JSON.stringify({ ok: true }));
  try {
    assert.deepEqual(await api("org"), { ok: true });
  } finally {
    globalThis.fetch = saved;
  }
});
