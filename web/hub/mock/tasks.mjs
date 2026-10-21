export const TASK_WORK_TITLE = "Await the price lookup before reading the total";
export const TASK_WORK_ID = "TASK-002";

const countKeys = ["jobs_total", "jobs_queued", "jobs_blocked", "jobs_running", "jobs_done", "jobs_failed"];

const stamp = (at, offset = 0) => new Date(Date.parse(at) + offset).toISOString();

function taskDefaults(task) {
  return {
    ...task,
    jobs_total: task.jobs_total ?? 0,
    jobs_queued: task.jobs_queued ?? 0,
    jobs_blocked: task.jobs_blocked ?? 0,
    jobs_running: task.jobs_running ?? 0,
    jobs_done: task.jobs_done ?? 0,
    jobs_failed: task.jobs_failed ?? 0,
    working: Array.isArray(task.working) ? task.working : [],
  };
}

function counts(jobs) {
  const out = { jobs_total: jobs.length, jobs_queued: 0, jobs_blocked: 0, jobs_running: 0, jobs_done: 0, jobs_failed: 0 };
  for (const job of jobs) {
    if (job.status === "queued") out.jobs_queued += 1;
    else if (job.status === "blocked") out.jobs_blocked += 1;
    else if (job.status === "leased" || job.status === "running") out.jobs_running += 1;
    else if (job.status === "done") out.jobs_done += 1;
    else if (job.status === "failed") out.jobs_failed += 1;
  }
  return out;
}

export function taskWorkFocus(detail, at) {
  const job = detail?.jobs?.find((j) => j.title === TASK_WORK_TITLE);
  if (!job) return null;
  return {
    job: job.id,
    kind: "improve",
    title: TASK_WORK_TITLE,
    agent: "cinder",
    since: stamp(at, -540_000),
    executor: "local",
  };
}

export function taskDetailScenario(detail, at) {
  const task = taskDefaults(detail.task || {});
  if (task.id !== TASK_WORK_ID) return { ...detail, task };
  const work = taskWorkFocus(detail, at);
  if (!work) return { ...detail, task };
  const jobs = (detail.jobs || []).map((job) => job.id === work.job ? { ...job, status: "leased", agent: "cinder" } : job);
  return {
    ...detail,
    jobs,
    task: {
      ...task,
      ...counts(jobs),
      working: [work],
    },
  };
}

export function taskScenarios(payload, at, details = {}) {
  return {
    ...payload,
    tasks: (payload.tasks || []).map((raw) => {
      const task = taskDefaults(raw);
      if (task.id !== TASK_WORK_ID) return task;
      const detail = taskDetailScenario(details[task.id] || { task, jobs: [] }, at);
      return { ...task, ...Object.fromEntries(countKeys.map((key) => [key, detail.task[key]])), working: detail.task.working };
    }),
  };
}
