import assert from "node:assert/strict";
import { test } from "node:test";
import { recordEvidence } from "./record-evidence.ts";

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
