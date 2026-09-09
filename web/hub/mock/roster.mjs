// Synthetic WU-024 scenarios shared by fixture generation and the live mock.
// Runtime clocks move only presence evidence, never useful-output/history dates.
export function rosterScenarios(rows, at, returned = true, focus = null) {
  const clock = Date.parse(at);
  const stamp = (offset = 0) => new Date(clock + offset).toISOString();
  const observation = (state, session = "", offset = 0) => ({
    presence: state,
    presence_reason: state === "online" ? "supervised execution" : state === "idle" ? "reusable; no active run" : state === "offline" ? "no valid heartbeat for five minutes" : "no supervised heartbeat evidence",
    presence_checked_at: stamp(),
    presence_session: session,
    last_heartbeat_at: session ? stamp(offset) : "",
    presence_expires_at: state === "online" ? stamp(offset + 300_000) : "",
  });
  const baseFocus = { current_task: "", current_task_title: "", current_project: "", current_source: "" };
  const result = rows.filter((r) => r.agent !== "granite" && !(r.agent === "delta" && r.session === "run-delta-current")).map((r) => {
    const base = { ...baseFocus, ...r, ...observation("unknown"), session: "", sessions: r.sessions || [] };
    switch (r.agent) {
      case "aurora":
        return { ...base, kind: "worker", sessions: ["run-aurora"], session_count: 1, ...observation("online", "run-aurora"),
          active_claims: [{ id: "mock-live-claim", slug: "demo-app", thread_key: "agents/heartbeat", summary: "Quiet supervised work", expires: stamp(3_600_000) }],
          current_thread: "agents/heartbeat", focus_since: stamp(-180_000) };
      case "demo-app-lead":
        return { ...base, kind: "lead", sessions: ["run-app-lead"], session_count: 1, ...observation("online", "run-app-lead"),
          current_job: "JOB-004", current_job_kind: "answer", current_job_title: "Answer the open governance question",
          current_task: "", current_task_title: "", current_project: "demo-app", current_source: "",
          current_thread: "governance/review-depth", executor: "local", focus_since: stamp(-120_000) };
      case "demo-lib-lead":
        return { ...base, kind: "lead", ...observation("idle") };
      case "cinder":
        return { ...base, kind: "worker", sessions: ["run-cinder"], session_count: 1, ...observation("online", "run-cinder"),
          current_job: focus?.job || "", current_job_kind: focus?.kind || "improve", current_job_title: focus?.title || "Await the price lookup before reading the total",
          current_task: "TASK-002", current_task_title: "Fix the flaky checkout test", current_project: "demo-app", current_source: "task:TASK-002",
          current_thread: "", executor: focus?.executor || "local", focus_since: stamp(-540_000) };
      case "borealis":
        return { ...base, sessions: ["run-offline"], session_count: 1, ...observation("offline", "run-offline", -360_000),
          active_claims: [{ id: "mock-offline-claim", slug: "demo-app", thread_key: "agents/old-claim", summary: "An unexpired claim is not liveness", expires: stamp(3_600_000) }] };
      case "ember":
        return { ...base, kind: "legacy", ...observation("legacy") };
      case "delta":
        return { ...base, kind: "worker", session: "run-delta-old", sessions: ["run-delta-old"], session_count: 1,
          ...observation("offline", "run-delta-old", -3_600_000), presence_checked_at: stamp(-60_000) };
      case "fjord":
        // An older API row without any presence fields must not appear online.
        return Object.fromEntries(Object.entries(base).filter(([key]) => key !== "presence" && !key.startsWith("presence_") && key !== "last_heartbeat_at"));
      default:
        return base;
    }
  });
  const delta = result.find((r) => r.agent === "delta");
  if (delta) result.push({
    ...delta, session: "run-delta-current", sessions: ["run-delta-current"], session_count: 1,
    posts: 0, store_posts: 0, votes: 0, claims: 0, jobs_done: 0, jobs_failed: 0, active_claims: [],
    last_seen: stamp(-30_000), ...observation(returned ? "online" : "offline", "run-delta-current", returned ? 0 : -360_000),
  });
  result.push({
    agent: "granite", name: "granite", names: ["granite"], kind: "worker", status: "pruned", agent_status: "pruned",
    runner: "copilot-cli", model: "gpt-6-astra", session: "", sessions: ["run-granite"], session_count: 1,
    jobs_done: 2, jobs_failed: 1, posts: 3, store_posts: 3, votes: 1, claims: 0, questions: 0, answers: 0,
    first_seen: "2026-08-01T00:00:00Z", last_seen: "2026-08-20T00:00:00Z", last_useful_at: "2026-08-20T00:00:00Z",
    pruned_at: "2026-09-07T09:00:00Z", pruned_reason: "no useful output since 2026-08-20T00:00:00Z (limit 14 days)",
    projects: ["demo-app"], active_claims: [], days_since: 18, approximate: false, name_shared: false, shared_with: [],
    current_job: "", current_job_kind: "", current_job_title: "", current_thread: "", executor: "", focus_since: "",
    current_task: "", current_task_title: "", current_project: "", current_source: "",
    ...observation("pruned"),
  });
  return result;
}
