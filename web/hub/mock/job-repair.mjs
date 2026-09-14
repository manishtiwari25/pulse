// Synthetic recovery history for the offline console, never the owner's jobs.
export function createJobRepairMock({ projects, settings, now, emit }) {
  const active = projects.filter((p) => p.status === "active");
  const primary = active.find((p) => p.slug === "demo-app") || active[0];
  const stamp = (minutes) => new Date(Date.parse(now()) - minutes * 60000).toISOString().replace(/\.\d{3}Z$/, "Z");
  const jobs = active.length ? Array.from({ length: 1644 }, (_, i) => {
    const project = i < 8 ? primary : active[(i - 8) % active.length];
    const id = "job-mock-failed-" + String(1644 - i).padStart(4, "0");
    const transient = i === 1 || i === 7;
    return {
      id, project: project.slug, kind: i === 4 ? "answer" : "improve", tier: 1,
      title: i < 9 ? ["Correct a failing project check", "Retry a transient runner failure", "Install a missing prerequisite", "Earlier attempt with completed replacement", "Earlier answer attempt", "Owner-cancelled work", "Repeated failure at its cap", "Transient failure in backoff", "Correct another project's check"][i] : "Historical project improvement attempt",
      source: "mock:" + id, status: "failed", result: i === 2 ? "verify exit 127: required tool missing" : i === 5 ? "cancelled by the owner" : transient ? "runner error: connection reset" : "verify exit 1",
      finished_at: stamp(i === 7 ? 2 : i < 9 ? 30 : 30 * 24 * 60),
      class: i === 2 || i === 5 ? "structural" : transient ? "transient" : "deterministic",
      root: id, recoveries: i === 6 ? 3 : 0, recovery_limit: 3,
      ...(i === 3 ? { carrier_id: "job-mock-completed-carrier", carrier_status: "done" } : {}),
      last_run_id: "run-" + id, last_run_status: "failed", last_run_summary: "Synthetic earlier failure; no real work is run by this mock.",
    };
  }) : [];
  let sequence = 0;
  const setting = (name, fallback) => settings.settings.find((s) => s.name === name)?.value ?? fallback;
  const failure = (status, error, why, fix) => ({ status, body: { error, why, fix } });
  const success = (body) => ({ status: 200, body });

  function view(job) {
    const result = { ...job, eligible: false, category: "eligible", reason: "", fix: "", recovery_limit: Number(setting("daemon.recover_limit", "3")) };
    const hold = (category, reason, fix) => Object.assign(result, { category, reason, fix });
    if (job.result.startsWith("cancelled by the owner")) return hold("cancelled", "Cancelled by the owner.", "Leave this work stopped; repair cannot undo cancellation.");
    if (job.status !== "failed") {
      result.carrier_id = job.id;
      result.carrier_status = job.status;
      return hold("not_failed", "The job is now " + job.status + ".", "Refresh; do not duplicate work already queued or completed.");
    }
    if (job.carrier_id) return hold("superseded", "Work is already carried by " + job.carrier_id + " (" + job.carrier_status + ").", "Follow the newer carrier instead of retrying this historical attempt.");
    if (!projects.some((p) => p.slug === job.project && p.status === "active")) return hold("inactive_project", "The project is inactive.", "Restore the project before requesting more work.");
    if (setting("stop", "0") === "1") return hold("stopped", "The owner's pause is on (stop = 1).", "Resolve the owner's pause through its existing controls; repair never changes it.");
    if (["answer", "lead", "baseline", "experiment"].includes(job.kind)) return hold("manual_controller", "This job belongs to a separate controller.", "Use the lead, answer or evolution workflow.");
    if (job.class === "structural") return hold("structural", job.result, "Resolve the missing prerequisite or owner question first. This panel never overrides holds.");
    if (setting("daemon.recover", "on") === "off") return hold("recovery_off", "Automatic recovery is disabled.", "Review the owner's recovery setting.");
    if (Date.parse(job.finished_at) < Date.parse(now()) - Number(setting("daemon.recover_days", "7")) * 86400000) return hold("outside_window", "This failure is older than the recovery window.", "Review current work with the lead instead of replaying old failures.");
    if (job.recoveries >= result.recovery_limit) return hold("recovery_cap", "The recovery chain has reached its cap.", "Resolve the repeated cause with the lead; repair cannot raise caps.");
    const minutes = Math.min(360, 5 * 3 ** job.recoveries);
    result.retry_at = new Date(Date.parse(job.finished_at) + minutes * 60000).toISOString().replace(/\.\d{3}Z$/, "Z");
    if (result.retry_at > now()) return hold("backoff", "Recovery is waiting until " + result.retry_at + ".", "Wait for the existing backoff and refresh.");
    if (jobs.filter((j) => j.project === job.project && j.status === "queued").length >= 5) return hold("queue_full", "The project already has five queued repairs.", "Let queued work finish before requesting more.");
    return Object.assign(result, { eligible: true, category: "eligible", reason: job.result, fix: job.class === "transient" ? "Requeue the transient attempt; earlier runs remain history." : "Queue one corrected copy carrying the earlier failure." });
  }

  function read(query) {
    const project = query.get("project") || "";
    if (project && !projects.some((p) => p.slug === project)) return failure(404, "No project " + project, "Only registered projects have jobs.", "Choose a registered project.");
    const limits = query.getAll("limit");
    const limit = limits.length ? Number(limits[0]) : 50;
    if (limits.length > 1 || !Number.isInteger(limit) || limit < 1 || limit > 100) return failure(400, "Invalid limit", "Use a whole number from 1 to 100.", "Use limit=50.");
    const before = query.get("before") || "";
    let beforeAt = "", beforeID = "";
    if (before) {
      if (before.length > 1024) return failure(400, "Invalid before cursor", "Use the cursor returned by the previous page.", "Refresh the list.");
      const raw = Buffer.from(before, "base64url").toString();
      const parts = raw.split("\0");
      if (parts.length !== 2 || parts[0].length > 100 || !parts[1] || parts[1].length > 200) return failure(400, "Invalid before cursor", "Use the cursor returned by the previous page.", "Refresh the list.");
      [beforeAt, beforeID] = parts;
    }
    const all = jobs.filter((j) => j.status === "failed" && (!project || j.project === project))
      .sort((a, b) => a.finished_at === b.finished_at ? (a.id < b.id ? 1 : -1) : a.finished_at < b.finished_at ? 1 : -1);
    const remaining = before ? all.filter((j) => j.finished_at < beforeAt || j.finished_at === beforeAt && j.id < beforeID) : all;
    const page = remaining.slice(0, limit);
    const rows = page.map(view);
    const pageCounts = {};
    for (const job of rows) pageCounts[job.category] = (pageCounts[job.category] || 0) + 1;
    const last = page[page.length - 1];
    return success({
      project, historical_total: all.length, jobs: rows, page_counts: pageCounts,
      limit, max_selection: 5,
      next_before: remaining.length > limit && last ? Buffer.from(last.finished_at + "\0" + last.id).toString("base64url") : "",
    });
  }

  function repair(body, queryProject = "") {
    if (!body || Array.isArray(body) || typeof body !== "object" || Object.keys(body).some((k) => k !== "job_ids" && k !== "project")) return failure(400, "Invalid repair request", "Only job_ids and project are accepted; the actor is server-owned.", "Select explicit job IDs.");
    const ids = body.job_ids;
    if (!Array.isArray(ids) || ids.length < 1 || ids.length > 5 || new Set(ids).size !== ids.length || ids.some((id) => typeof id !== "string" || !id || id.trim() !== id || id.length > 200 || id.includes("\0"))) return failure(400, "Invalid job_ids", "Select 1 to 5 distinct existing job IDs.", "Do not request a bulk replay.");
    if ("project" in body && typeof body.project !== "string") return failure(400, "Invalid project", "project must be a string.", "Choose a registered project.");
    if (queryProject && body.project && queryProject !== body.project) return failure(400, "Conflicting projects", "Query and body name different projects.", "Use one project.");
    const project = body.project || queryProject;
    if (project && !projects.some((p) => p.slug === project)) return failure(404, "No project " + project, "Only registered projects have jobs.", "Choose a registered project.");
    const result = { requested: ids.length, queued: 0, skipped: 0, errors: 0, results: [] };
    for (const id of ids) {
      const job = jobs.find((j) => j.id === id && (!project || j.project === project));
      if (!job) {
        result.skipped++;
        result.results.push({ job_id: id, outcome: "skipped", category: "not_found", reason: "No such job in the selected project.", fix: "Refresh and select a listed job." });
        continue;
      }
      const current = view(job);
      const outcome = { job_id: id, outcome: "skipped", category: current.category, reason: current.reason, fix: current.fix, ...(current.carrier_id ? { carrier_id: current.carrier_id, carrier_status: current.carrier_status } : {}) };
      if (current.eligible) {
        const carrierID = job.class === "transient" ? job.id : "job-mock-repair-" + (++sequence);
        if (carrierID === job.id) {
          job.status = "queued";
          job.recoveries++;
        } else {
          jobs.push({ ...job, id: carrierID, status: "queued", recoveries: job.recoveries + 1 });
          job.carrier_id = carrierID;
          job.carrier_status = "queued";
        }
        outcome.outcome = job.class === "transient" ? "requeued" : "replanned";
        outcome.carrier_id = carrierID;
        outcome.carrier_status = "queued";
        result.queued++;
        emit("human", "job.recover", job.id, { project: job.project, outcome: outcome.outcome, root: job.root, job: carrierID });
      } else result.skipped++;
      result.results.push(outcome);
    }
    return success(result);
  }

  return { read, repair };
}
