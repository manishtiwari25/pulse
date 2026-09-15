// Human names for the store's setting keys (ADR-042 D5). The key stays on
// the row in small type and in the search, so `pulse setting set KEY` is
// never a translation exercise; a key this map does not know gets a name
// derived from it, so a new cap never shows as a raw identifier.
const LABELS: Record<string, string> = {
  stop: "Kill switch",
  workers: "Local worker slots",
  parallel_per_project: "Jobs in flight per project",
  jobs_per_day: "Worker runs per day",
  minutes_per_job: "Minutes per run",
  max_ai_credits: "AI credits per run",
  runner_cmd: "Worker program",
  prune_after_days: "Prune an idle agent after (days)",
  worker_model: "Worker model",
  light_model: "Light model",
  landing: "Who merges verified work",
  "landing.push": "Push after landing",
  "push.credential": "Push key",
  quorum: "Votes to queue a suggestion",
  expiry_days: "A suggestion expires after (days)",
  stale_days: "A thread goes stale after (days)",
  claim_hours: "A claim lasts (hours)",
  amend_quorum: "Sessions to amend",
  amend_majority: "Majority to amend",
  "lead.idle_hours": "Idle hours before the lead finds work",
  "lead.idle_max_jobs": "Jobs an idle lead may queue",
  "lead.stuck_thread_jobs": "Empty jobs before a thread is held",
  "owner.presence": "Presence",
  "compute.provider": "Cloud provider",
  "compute.max_vms": "Machines at once",
  "compute.max_hours_per_day": "Machine-hours per day",
  "compute.auto_approve": "Approve within caps by itself",
  "compute.budget_credits": "AI credits for hands per day",
  "compute.self_heal": "Mend a refused login",
  "compute.self_retire": "Retire a machine that stays refused",
  "compute.retire_grace_hours": "Grace before deletion (hours)",
  "compute.azure.subscription": "Subscription",
  "compute.azure.resource_group": "Resource group",
  "compute.azure.location": "Region",
  "compute.azure.vm_size": "VM size",
  "compute.azure.image": "Image",
  "compute.aws.profile": "CLI profile",
  "compute.aws.region": "Region",
  "compute.aws.instance_type": "Instance type",
  "compute.aws.image": "AMI",
  "compute.aws.subnet": "Subnet",
  "compute.aws.security_group": "Security group",
  "compute.gcp.project": "Project",
  "compute.gcp.zone": "Zone",
  "compute.gcp.machine_type": "Machine type",
  "compute.gcp.image": "Image",
  "compute.gcp.network": "Network",
  "compute.multipass.size": "VM size",
  "compute.multipass.image": "Image",
  "evolve.loop": "Run the loop",
  "evolve.limits": "Loop budget",
  "evolve.tolerance": "Improvement a variant needs",
  "evolve.heldout": "Who writes the held-out set",
  "evolve.heldout_dir": "Held-out folder",
  "daemon.self_update": "Self-update",
  "daemon.self_update_retry_minutes": "Retry a failed build after (minutes)",
  "daemon.recover": "Recover failed jobs",
  "daemon.recover_days": "Recovery window (days)",
  "daemon.recover_limit": "Recoveries per job",
  "daemon.sweep": "Sweep leftovers",
  "daemon.sweep_branch_days": "Remove a dead branch after (days)",
  "daemon.sweep_grace_minutes": "Sweep grace (minutes)",
  "daemon.sweep_keep_days": "Keep logs and patches (days)",
  "daemon.min_free_mb": "Minimum free disk (MB)",
  "provider.openai.base_url": "OpenAI-compatible endpoint",
};

const PREFIX = /^(compute\.(azure|aws|gcp|multipass)|compute|evolve|daemon|lead|owner|landing|push|harness|provider)\./;

export function settingLabel(name: string): string {
  const known = LABELS[name];
  if (known) return known;
  const bare = name.replace(PREFIX, "").replace(/[._]+/g, " ").trim();
  return bare ? bare.charAt(0).toUpperCase() + bare.slice(1) : name;
}

// The providers Compute knows, in the order the store lists them; the
// settings page shows only the chosen one's target (ADR-042 D6).
export const COMPUTE_PROVIDERS: Record<string, string> = { azure: "Azure", aws: "AWS", gcp: "GCP", multipass: "Multipass" };
