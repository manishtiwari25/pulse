import { api, query } from "./api";

export const JOB_REPAIR_MAX_SELECTION = 5;
export const FAILED_JOBS_PAGE_SIZE = 50;

// Why a failed job can or can't be retried, as the owner reads it.
export const JOB_REPAIR_CATEGORIES = {
  eligible: "Can be retried",
  structural: "Needs a fix first",
  manual_controller: "Handled by its lead",
  superseded: "Another job carries the work",
  active_run: "Still running",
  recovery_cap: "Retried too many times",
  backoff: "Waiting before the next retry",
  queue_full: "Project queue is full",
  outside_window: "Too old to retry",
  held: "On hold",
  cancelled: "You cancelled it",
  stopped: "Work is stopped",
  recovery_off: "Automatic retries are off",
  inactive_project: "Project is inactive",
  not_failed: "No longer failed",
} as const;

export type FailedJobCategory = keyof typeof JOB_REPAIR_CATEGORIES;

// The same reasons as one plain sentence for a failed job's card: what it
// means for the owner and whether anything is needed from them.
export const JOB_REPAIR_SENTENCE: Record<FailedJobCategory, string> = {
  eligible: "It stopped before finishing. You can retry it if the work still matters.",
  structural: "It keeps failing the same way, so retrying won't help until the cause is fixed.",
  manual_controller: "Its lead agent decides what happens next, so there's nothing to do here.",
  superseded: "A newer job already picked up this work. Nothing to do.",
  active_run: "It's running again right now. Check back when it finishes.",
  recovery_cap: "PULSE already retried it as many times as allowed. Look at why before trying again.",
  backoff: "PULSE will retry it on its own shortly. Nothing to do.",
  queue_full: "The project has too much waiting right now. Retry once the queue clears.",
  outside_window: "It failed too long ago to retry. Give the task again if it still matters.",
  held: "It's on hold, so it can't be retried until the hold is lifted.",
  cancelled: "You cancelled it, so PULSE leaves it alone.",
  stopped: "Work is stopped on this machine, so nothing can be retried until it's resumed.",
  recovery_off: "Automatic retries are off for this project. You can still retry it by hand.",
  inactive_project: "Its project isn't active, so it can't run right now.",
  not_failed: "It finished after all. Nothing to do.",
};

// What kind of failure it was (ADR-036), in plain words.
export const JOB_FAILURE_CLASS: Record<string, string> = {
  transient: "Temporary problem",
  deterministic: "Problem in the work",
  structural: "Needs a fix first",
};

export interface FailedJob {
  id: string;
  project: string;
  kind: string;
  tier: number;
  title: string;
  source: string;
  status: string;
  result: string;
  finished_at: string;
  class: "structural" | "transient" | "deterministic";
  category: FailedJobCategory;
  reason: string;
  fix: string;
  eligible: boolean;
  root: string;
  recoveries: number;
  recovery_limit: number;
  retry_at?: string;
  carrier_id?: string;
  carrier_status?: string;
  last_run_id?: string;
  last_run_status?: string;
  last_run_summary?: string;
}

export interface FailedJobsPage {
  project: string;
  // All ages, but only jobs still marked failed; not a count of failed runs.
  historical_total: number;
  jobs: FailedJob[];
  page_counts: Record<string, number>;
  limit: number;
  next_before: string;
  max_selection: number;
}

export interface JobRepairResult {
  job_id: string;
  outcome: "requeued" | "replanned" | "skipped" | "error";
  category: FailedJobCategory | "not_found" | "error";
  reason: string;
  fix: string;
  carrier_id?: string;
  carrier_status?: string;
}

export interface JobRepairResponse {
  requested: number;
  queued: number;
  skipped: number;
  errors: number;
  results: JobRepairResult[];
}

export function failedJobsPath(project?: string, before?: string, limit = FAILED_JOBS_PAGE_SIZE): string {
  return "jobs/failed" + query({ project, before, limit });
}

export function repairFailedJobs(jobIDs: string[], project?: string): Promise<JobRepairResponse> {
  if (jobIDs.length < 1 || jobIDs.length > JOB_REPAIR_MAX_SELECTION || new Set(jobIDs).size !== jobIDs.length || jobIDs.some((id) => !id.trim())) {
    return Promise.reject(new Error("Pick 1 to 5 different jobs that can be retried."));
  }
  return api<JobRepairResponse>("jobs/repair", {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-Pulse-Job-Repair": "yes" },
    body: JSON.stringify({ job_ids: jobIDs, ...(project ? { project } : {}) }),
  });
}
