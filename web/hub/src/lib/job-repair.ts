import { api, query } from "./api";

export const JOB_REPAIR_MAX_SELECTION = 5;
export const FAILED_JOBS_PAGE_SIZE = 50;

export const JOB_REPAIR_CATEGORIES = {
  eligible: "Eligible for repair",
  structural: "Prerequisite or owner action",
  manual_controller: "Separate controller / lead",
  superseded: "Work has another carrier",
  active_run: "Run still active",
  recovery_cap: "Recovery cap reached",
  backoff: "Waiting for backoff",
  queue_full: "Project queue full",
  outside_window: "Outside recovery window",
  held: "Recorded recovery hold",
  cancelled: "Owner cancelled",
  stopped: "STOP",
  recovery_off: "Recovery disabled",
  inactive_project: "Project inactive",
  not_failed: "No longer failed",
} as const;

export type FailedJobCategory = keyof typeof JOB_REPAIR_CATEGORIES;

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
    return Promise.reject(new Error("Select 1 to 5 distinct eligible jobs."));
  }
  return api<JobRepairResponse>("jobs/repair", {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-Pulse-Job-Repair": "yes" },
    body: JSON.stringify({ job_ids: jobIDs, ...(project ? { project } : {}) }),
  });
}
