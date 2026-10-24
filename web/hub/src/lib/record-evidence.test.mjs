import assert from "node:assert/strict";
import { test } from "node:test";
import { evidenceLabel, recordEvidence } from "./record-evidence.ts";

test("record evidence links the producing job thread and ballot post, once each", () => {
  const body = [
    "Seen in run `run-20261004T221729Z-b07e83` of job `job-20261004T221802Z-28402f`.",
    "Voted on 20261004T221729Z-cinder-3; job-20261004T221802Z-28402f again.",
  ].join("\n");
  const links = recordEvidence("demo-app", body, { on_post: "20260905T173537Z-fable-scale-09051203" });
  assert.deepEqual(links, [
    { kind: "job", id: "job-20261004T221802Z-28402f", href: "/project/?slug=demo-app&tab=board&sub=threads&thread=evolution%2Fjob-20261004t221802z-28402f" },
    { kind: "post", id: "20261004T221729Z-cinder-3", href: "/project/?slug=demo-app&tab=board&sub=threads&post=20261004T221729Z-cinder-3" },
    { kind: "post", id: "20260905T173537Z-fable-scale-09051203", href: "/project/?slug=demo-app&tab=board&sub=threads&post=20260905T173537Z-fable-scale-09051203" },
  ]);
});

test("record evidence ignores run ids and records that name nothing", () => {
  assert.deepEqual(recordEvidence("demo-app", "run `run-20261004T221729Z-b07e83` only"), []);
  assert.deepEqual(recordEvidence("demo-app", ""), []);
});

test("evidence links read in plain words, with the time from the id", () => {
  assert.deepEqual(evidenceLabel({ kind: "job", id: "job-20261004T221802Z-28402f", href: "" }), { label: "What the job found, on the board", when: "4 Oct 2026, 22:18 UTC" });
  assert.deepEqual(evidenceLabel({ kind: "post", id: "20261004T221729Z-cinder-3", href: "" }), { label: "Board post by cinder-3", when: "4 Oct 2026, 22:17 UTC" });
  assert.deepEqual(evidenceLabel({ kind: "post", id: "20260905T173537Z-fable-scale-09051203", href: "" }), { label: "Board post by fable-scale", when: "5 Sep 2026, 17:35 UTC" });
});
