package store

import (
	"database/sql"
	"errors"
	"math"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

// DefaultSettings mirrors legacy/pulse.py DEFAULT_SETTINGS.
var DefaultSettings = map[string]string{
	"jobs_per_day":         "2",
	"minutes_per_job":      "10",
	"workers":              "1",
	"parallel_per_project": "1",
	"max_ai_credits":       "30",
	"prune_after_days":     "14",
	"claim_hours":          "4",
	"quorum":               "2",
	"expiry_days":          "14",
	"stale_days":           "30",
	"amend_quorum":         "40",
	"amend_majority":       "2/3",
	"runner_cmd":           "copilot",
	"worker_model":         "",
	"stop":                 "0",
	"landing":              "agents",
}

// SettingRule is one row of legacy/pulse.py SETTING_RULES: the kind decides
// how a value is validated (count, count-or-none, credits, fraction, switch,
// choice, command, text; Go adds identifier and decimal for the compute.* and
// evolve.* settings).
type SettingRule struct {
	Kind        string
	Min         int
	HasMin      bool
	Choices     []string
	Max         float64 // decimal kind: upper bound, when HasMax
	HasMax      bool
	Description string
}

// SettingRules mirrors legacy/pulse.py SETTING_RULES.
var SettingRules = map[string]SettingRule{
	"jobs_per_day":         {Kind: "count-or-none", Min: 0, HasMin: true, Description: "worker runs the runner may start per UTC day on every executor together; 0 or none means no daily cap"},
	"minutes_per_job":      {Kind: "count", Min: 0, HasMin: true, Description: "wall-clock minutes one worker run may take before the runner stops it"},
	"workers":              {Kind: "count", Min: 0, HasMin: true, Description: "parallel slots on this machine (the local executor); 0 keeps it idle while remote hands exist"},
	"parallel_per_project": {Kind: "count", Min: 1, HasMin: true, Description: "leased jobs one project may have in flight at once; 1 is one at a time"},
	"max_ai_credits":       {Kind: "credits", Min: 30, HasMin: true, Description: "AI credits one worker run may spend; the copilot CLI refuses anything below 30"},
	"prune_after_days":     {Kind: "count", Min: 1, HasMin: true, Description: "days without useful output after which an agent is pruned (its history stays)"},
	"claim_hours":          {Kind: "count", Min: 1, HasMin: true, Description: "hours a board claim stays active before it is treated as stale"},
	"quorum":               {Kind: "count", Min: 1, HasMin: true, Description: "distinct voters a suggestion needs before it is eligible for the queue"},
	"expiry_days":          {Kind: "count", Min: 1, HasMin: true, Description: "days a suggestion may wait for votes before it expires"},
	"stale_days":           {Kind: "count", Min: 1, HasMin: true, Description: "days after which an untouched thread counts as stale"},
	"amend_quorum":         {Kind: "count", Min: 1, HasMin: true, Description: "distinct sessions an amendment needs; may be raised by vote, lowered only by a human"},
	"amend_majority":       {Kind: "fraction", Description: "share of counted votes an amendment needs, as a fraction such as 2/3"},
	"runner_cmd":           {Kind: "command", Description: "the worker program the runner starts in each clone (copilot, or a path to a stand-in)"},
	"worker_model":         {Kind: "text", Description: "the model of the runs that change code - task, improve and experiment jobs - and of every other run while light_model is empty; empty means the runner's default"},
	"stop":                 {Kind: "switch", Choices: []string{"0", "1"}, Description: "the kill switch: 1 stops the runner after the runs in flight finish (pulse stop / pulse start)"},
	"landing":              {Kind: "choice", Choices: []string{"agents", "human"}, Description: "agents: verified branches merge into the default branch by themselves; human: they wait for you (ADR-024)"},
}

// ImplementationJobKinds are the job kinds whose run changes code and so
// runs with worker_model: the owner's tasks, the board's voted improvements
// and the loop's experiments. Every other run - the lead, answers, ballots,
// reflects, onboarding, the sealed held-out call, the login check - is light
// work and runs with light_model when the owner set one.
var ImplementationJobKinds = []string{"task", "improve", "experiment"}

// ModelSettings split the runner's model by the weight of the work: the
// legacy worker_model stays the model of the runs that change code, and
// light_model is the model of everything lighter, so the owner can put a
// cheaper model on the routine and keep the strong one for implementation.
// Like ComputeSettings they are not written at Create; Setting falls back
// here, and an empty light_model means "the same as worker_model".
var ModelSettings = map[string]string{
	"light_model": "",
}

// ModelRules validate ModelSettings.
var ModelRules = map[string]SettingRule{
	"light_model": {Kind: "text", Description: "the model of the light runs - lead, answer, ballot, reflect, onboard and clean jobs, the sealed held-out call and the login check; empty means worker_model"},
}

// RunModel is the model a run of the given job kind gets: worker_model for
// the kinds that change code (ImplementationJobKinds), else LightModel. ""
// means the runner's own default model.
func (s *Store) RunModel(kind string) string {
	for _, k := range ImplementationJobKinds {
		if k == kind {
			return strings.TrimSpace(s.Setting("worker_model", ""))
		}
	}
	return s.LightModel()
}

// LightModel is the model of the light work - the lead, answers, ballots,
// reflects, onboarding, the sealed held-out call and the login check:
// light_model, falling back to worker_model while light_model is empty.
func (s *Store) LightModel() string {
	if light := strings.TrimSpace(s.Setting("light_model", "")); light != "" {
		return light
	}
	return strings.TrimSpace(s.Setting("worker_model", ""))
}

// ComputeProviders are the compute providers the runner builds in, in the
// order the console lists them; compute.provider takes one of them. Each
// authenticates through its own CLI's login on the hub machine (az login, aws
// configure / aws sso login, gcloud auth login) or, for multipass, needs none:
// the store never holds a cloud credential.
var ComputeProviders = []string{"azure", "aws", "gcp", "multipass"}

// ComputeSizeSettings names, per provider, the setting that holds the machine
// size a compute request gets when it names none.
var ComputeSizeSettings = map[string]string{
	"azure":     "compute.azure.vm_size",
	"aws":       "compute.aws.instance_type",
	"gcp":       "compute.gcp.machine_type",
	"multipass": "compute.multipass.size",
}

// ComputeSizeSetting is the default-size setting of a provider; an unknown
// provider falls back to azure's so old stores and messages keep working.
func ComputeSizeSetting(provider string) string {
	if name, ok := ComputeSizeSettings[strings.TrimSpace(provider)]; ok {
		return name
	}
	return ComputeSizeSettings["azure"]
}

// ComputeSettings are the owner's compute caps and provider configuration
// (D-D). They are not written at Create (legacy stores stay byte-identical);
// Setting falls back to these defaults. Secret-shaped values are refused:
// cloud authentication is the CLI login on the hub machine, never a stored key.
var ComputeSettings = map[string]string{
	"compute.provider":             "azure",
	"compute.azure.subscription":   "",
	"compute.azure.resource_group": "",
	"compute.azure.location":       "",
	"compute.azure.vm_size":        "Standard_B2ms",
	"compute.azure.image":          "Ubuntu2404",
	"compute.aws.profile":          "",
	"compute.aws.region":           "",
	"compute.aws.instance_type":    "t3.medium",
	"compute.aws.image":            "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id",
	"compute.aws.subnet":           "",
	"compute.aws.security_group":   "",
	"compute.gcp.project":          "",
	"compute.gcp.zone":             "",
	"compute.gcp.machine_type":     "e2-medium",
	"compute.gcp.image":            "ubuntu-2404-lts-amd64",
	"compute.gcp.network":          "",
	"compute.multipass.size":       "cpus=2,mem=4G,disk=32G",
	"compute.multipass.image":      "24.04",
	"compute.max_vms":              "0",
	"compute.max_hours_per_day":    "0",
	"compute.auto_approve":         "0",
	"compute.budget_credits":       "0",
	"compute.self_heal":            "1",
	"compute.self_retire":          "1",
	"compute.retire_grace_hours":   "24",
}

// ComputeRules validate ComputeSettings; kind "identifier" is free text that
// may be empty and must not look like a credential (SecretShaped).
var ComputeRules = map[string]SettingRule{
	"compute.provider":             {Kind: "choice", Choices: ComputeProviders, Description: "where agents get more machines from: azure (az login), aws (aws configure / aws sso login), gcp (gcloud auth login) or multipass (local VMs on the hub machine); each provider reads its own compute.<provider>.* settings"},
	"compute.azure.subscription":   {Kind: "identifier", Description: "Azure subscription id or name the runner provisions in (auth is az login on this machine)"},
	"compute.azure.resource_group": {Kind: "identifier", Description: "Azure resource group that holds the provisioned hands"},
	"compute.azure.location":       {Kind: "identifier", Description: "Azure region, e.g. westeurope"},
	"compute.azure.vm_size":        {Kind: "identifier", Description: "VM size for a provisioned hand, e.g. Standard_B2ms"},
	"compute.azure.image":          {Kind: "identifier", Description: "VM image alias or URN, e.g. Ubuntu2404"},
	"compute.aws.profile":          {Kind: "identifier", Description: "AWS CLI profile the runner provisions with; empty means the CLI's default (auth is aws configure or aws sso login on this machine)"},
	"compute.aws.region":           {Kind: "identifier", Description: "AWS region the instances are created in, e.g. eu-west-1"},
	"compute.aws.instance_type":    {Kind: "identifier", Description: "EC2 instance type for a provisioned hand, e.g. t3.medium"},
	"compute.aws.image":            {Kind: "identifier", Description: "AMI id (ami-...) or an SSM public parameter path resolved at launch; the default is Canonical's current Ubuntu 24.04 amd64 image"},
	"compute.aws.subnet":           {Kind: "identifier", Description: "subnet id the instances join; empty means the default VPC's default subnet in the region"},
	"compute.aws.security_group":   {Kind: "identifier", Description: "security group id the instances get; empty means a group named pulse-hands that allows ssh in, created when missing"},
	"compute.gcp.project":          {Kind: "identifier", Description: "Google Cloud project the instances are created in; empty means gcloud's current project (auth is gcloud auth login on this machine)"},
	"compute.gcp.zone":             {Kind: "identifier", Description: "Compute Engine zone, e.g. europe-west4-a"},
	"compute.gcp.machine_type":     {Kind: "identifier", Description: "machine type for a provisioned hand, e.g. e2-medium"},
	"compute.gcp.image":            {Kind: "identifier", Description: "Ubuntu image family in ubuntu-os-cloud, e.g. ubuntu-2404-lts-amd64, or a full image path projects/P/global/images/I"},
	"compute.gcp.network":          {Kind: "identifier", Description: "VPC network the instances join; empty means default. A firewall rule pulse-hands-ssh (tcp 22 to the tag pulse-hands) is created on it when missing"},
	"compute.multipass.size":       {Kind: "identifier", Description: "size of a local multipass hand as cpus=N,mem=SIZE,disk=SIZE, e.g. cpus=2,mem=4G,disk=32G"},
	"compute.multipass.image":      {Kind: "identifier", Description: "multipass image a local hand launches from, e.g. 24.04"},
	"compute.max_vms":              {Kind: "count", Min: 0, HasMin: true, Description: "provisioned machines that may exist at once; 0 means agents cannot get compute"},
	"compute.max_hours_per_day":    {Kind: "count", Min: 0, HasMin: true, Description: "machine-hours agents may consume per UTC day across provisioned hands; 0 means none"},
	"compute.auto_approve":         {Kind: "switch", Choices: []string{"0", "1"}, Description: "1: requests under the caps are approved without the owner; 0: every request becomes a proposal"},
	"compute.budget_credits":       {Kind: "count", Min: 0, HasMin: true, Description: "AI credits the provisioned hands may spend per UTC day together; 0 means no extra budget"},
	"compute.self_heal":            {Kind: "switch", Choices: []string{"0", "1"}, Description: "1: the daemon places the executor token (pulse executor token) on a drained executor whose login is refused, once per drain; 0: it leaves the login to you (ADR-034)"},
	"compute.self_retire":          {Kind: "switch", Choices: []string{"0", "1"}, Description: "1: the daemon deallocates a VM it created whose login stays refused and retires its executor row, then deletes the VM after compute.retire_grace_hours; 0: it only reports (ADR-034)"},
	"compute.retire_grace_hours":   {Kind: "count", Min: 1, HasMin: true, Description: "hours a retired VM stays stopped before the daemon deletes it; starting it again at the provider (the finding names the command) and `pulse executor activate NAME` within them keep it"},
}

// EvolveSettings configure the self-improvement loop (D-M). Like
// ComputeSettings they are not written at Create; Setting falls back here.
var EvolveSettings = map[string]string{
	"evolve.loop":        "0",
	"evolve.limits":      "none",
	"evolve.tolerance":   "0.02",
	"evolve.heldout_dir": "",
	"evolve.heldout":     "auto",
}

// EvolveRules validate EvolveSettings.
var EvolveRules = map[string]SettingRule{
	"evolve.loop":        {Kind: "switch", Choices: []string{"0", "1"}, Description: "1: pulse evolve cycle may run experiments; 0: the loop is off (STOP is the stop setting)"},
	"evolve.limits":      {Kind: "text", Description: "none, or a budget such as cycles=20,hours=48 after which the loop stops by itself"},
	"evolve.tolerance":   {Kind: "decimal", Max: 1, HasMax: true, Description: "score improvement a variant needs over the baseline to be kept, a share between 0 and 1 such as 0.02"},
	"evolve.heldout_dir": {Kind: "identifier", Description: "owner-private directory outside every repository holding held-out fixtures; never quoted to agents"},
	"evolve.heldout":     {Kind: "choice", Choices: []string{"auto", "owner"}, Description: "auto: the daemon writes the private held-out set itself, sealed and labelled generated; owner: you write it from the console (ADR-033)"},
}

// DaemonSettings govern the daemon's care of its own binary (ADR-035), of
// the jobs that failed (ADR-036: recovery), of what it does after a
// landing (ADR-037: landing.push) and of what its runs and landings leave
// on this machine (daemon.sweep). Like ComputeSettings they are not written
// at Create; Setting falls back here.
var DaemonSettings = map[string]string{
	"daemon.self_update":               "on",
	"daemon.self_update_retry_minutes": "60",
	"daemon.recover":                   "on",
	"daemon.recover_limit":             "3",
	"daemon.recover_days":              "7",
	"daemon.sweep":                     "on",
	"daemon.sweep_grace_minutes":       "30",
	"daemon.sweep_branch_days":         "7",
	"daemon.sweep_keep_days":           "30",
	"landing.push":                     "off",
}

// DaemonRules validate DaemonSettings.
var DaemonRules = map[string]SettingRule{
	"daemon.self_update":               {Kind: "choice", Choices: []string{"on", "off"}, Description: "on: after a landing in the brain the daemon rebuilds pulse from the clean checkout, smoke-tests it, installs it and restarts after its runs in flight; off: builds are yours (ADR-035; the launcher turns it off after a crash-loop)"},
	"daemon.self_update_retry_minutes": {Kind: "count", Min: 1, HasMin: true, Description: "minutes the daemon waits before building again after a failed build or smoke test"},
	"daemon.recover":                   {Kind: "choice", Choices: []string{"on", "off"}, Description: "on: the daemon recovers failed jobs by itself - a transient failure runs again after a growing pause, a failure in the work becomes one corrected job carrying the failure, a structural one waits for its own fix - and writes the repeated causes down as memory records that later briefs carry; off: a failed job stays failed until `pulse job retry` (ADR-036)"},
	"daemon.recover_limit":             {Kind: "count", Min: 0, HasMin: true, Description: "recoveries one failed job may receive (the same job run again or a corrected job queued for it) before the daemon holds it for the lead and the owner; 0 holds every failure at once"},
	"daemon.recover_days":              {Kind: "count", Min: 1, HasMin: true, Description: "days a failed job stays eligible for recovery and counts toward a repeated-failure lesson"},
	"daemon.sweep":                     {Kind: "choice", Choices: []string{"on", "off"}, Description: "on: every ten minutes the daemon removes what its runs and landings left on this machine and nothing holds any more - clones, keys and bundles of finished jobs under PULSE_HOME/work, landing worktrees whose process is gone, holding refs, run logs and patches past daemon.sweep_keep_days, and pulse/* or evolve/* branches no job will land (saved as a patch first); a run in flight, a branch waiting to land and the owner's own files are never touched. off: `pulse sweep` by hand"},
	"daemon.sweep_grace_minutes":       {Kind: "count", Min: 0, HasMin: true, Description: "minutes a finished job's clone, key or bundle and a dead landing's worktree must sit untouched before the sweep removes them; a lost run's leftovers go at once"},
	"daemon.sweep_branch_days":         {Kind: "count", Min: 0, HasMin: true, Description: "days since its last commit before a pulse/* or evolve/* branch that no job will land is removed; its commits are saved under PULSE_HOME/patches first unless they are already on the default branch"},
	"daemon.sweep_keep_days":           {Kind: "count", Min: 1, HasMin: true, Description: "days run logs, saved patches and work files of jobs this store does not know are kept before the sweep removes them"},
	"landing.push":                     {Kind: "choice", Choices: []string{"off", "main"}, Description: "main: after a landing the daemon pushes the project's default branch to its upstream, fast-forward only, with the git credentials this machine already has, and pushes what landed earlier and was not pushed yet; a failed push shows in the landing line of pulse status and the hub. off: you push (ADR-037)"},
}

// LeadSettings govern the lead's standing brief (D-C): what the lead does
// when the owner is quiet and no work is queued for a project.
var LeadSettings = map[string]string{
	"lead.idle_hours":        "6",
	"lead.idle_max_jobs":     "3",
	"lead.stuck_thread_jobs": "3",
}

// LeadRules validate LeadSettings.
var LeadRules = map[string]SettingRule{
	"lead.idle_hours":        {Kind: "count", Min: 0, HasMin: true, Description: "hours a project may sit with no job queued, leased or created before its lead is asked to find the next work by itself and tell the owner; 0 turns the standing brief off"},
	"lead.idle_max_jobs":     {Kind: "count", Min: 1, HasMin: true, Description: "worker jobs one idle lead run may queue"},
	"lead.stuck_thread_jobs": {Kind: "count", Min: 0, HasMin: true, Description: "finished worker jobs one board thread may seed in a row (from its suggestions, handoffs and questions) that commit nothing - read-only triage, claims, questions - before the planner stops seeding that thread and asks the lead to decide it; a job from the thread that commits resets the count; 0 turns the cap off"},
}

const (
	OwnerPresent = "present"
	OwnerAway    = "away"
)

// OwnerSettings say whether the owner is there to answer: when away, each project's lead decides the board's
// proposals on the owner's behalf; hard stops, money, Tier 2 and protected sections still wait. Only the owner's
// own write counts (see OwnerAway): the setting hands authority down, so an agent cannot grant it to itself.
var OwnerSettings = map[string]string{"owner.presence": OwnerPresent}

// OwnerPaths are the files only the owner changes: the constitution, the evaluators, the kill switch, the
// decisions and the workflows. Worker commits, worker-card signals and lead decisions may not target them.
var OwnerPaths = []string{"docs/evolution/constitution.md", "docs/evolution/evaluators.md", "docs/evolution/STOP", "docs/decisions/", ".github/"}

// HumanActor reports whether an actor is the owner: `human` (the CLI without PULSE_AGENT) or `human:<surface>`
// such as the console's `human:dashboard`.
func HumanActor(actor string) bool {
	return actor == HumanAgent || strings.HasPrefix(actor, HumanAgent+":")
}

// OwnerRules validate OwnerSettings.
var OwnerRules = map[string]SettingRule{
	"owner.presence": {Kind: "choice", Choices: []string{OwnerPresent, OwnerAway}, Description: "present: proposals, compute requests and owner questions wait for you; away: each project's lead decides the board's proposals on your behalf with its best judgement, recorded as the lead's decision and never as yours - hard stops, money (compute), Tier 2 decisions (ADRs), the constitution's protected sections and the kill switch still wait for you"},
}

var secretValueRes = []*regexp.Regexp{
	regexp.MustCompile(`-----BEGIN`),
	regexp.MustCompile(`://[^/\s:@]+:[^@\s]+@`),
	regexp.MustCompile(`\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}`),
	regexp.MustCompile(`\bAKIA[0-9A-Z]{12,}`),
	regexp.MustCompile(`\b(?:ghp_|github_pat_|sk-)[A-Za-z0-9_-]{8,}`),
}
var longRunRe = regexp.MustCompile(`[A-Za-z0-9+/_=-]{40,}`)
var pathLikeRe = regexp.MustCompile(`^(/|\./|\.\./|~|\$|[A-Za-z][A-Za-z0-9+.-]*://[^@\s]*$)`)

// SecretShaped mirrors legacy/pulse.py secret_shaped(): a PEM block, user:pass
// in a URL, a JWT, a known key prefix, or a long opaque run that is not a
// path or URL.
func SecretShaped(value string) bool {
	for _, rx := range secretValueRes {
		if rx.MatchString(value) {
			return true
		}
	}
	return longRunRe.MatchString(value) && !pathLikeRe.MatchString(strings.TrimSpace(value))
}

func knownSetting(name string) (SettingRule, bool) {
	if _, ok := DefaultSettings[name]; ok {
		rule, has := SettingRules[name]
		if !has {
			rule = SettingRule{Kind: "text"}
		}
		return rule, true
	}
	if rule, ok := ComputeRules[name]; ok {
		return rule, true
	}
	if rule, ok := ModelRules[name]; ok {
		return rule, true
	}
	if rule, ok := EvolveRules[name]; ok {
		return rule, true
	}
	if rule, ok := DaemonRules[name]; ok {
		return rule, true
	}
	if rule, ok := LeadRules[name]; ok {
		return rule, true
	}
	if rule, ok := OwnerRules[name]; ok {
		return rule, true
	}
	return SettingRule{}, false
}

func defaultSetting(name string) string {
	if v, ok := DefaultSettings[name]; ok {
		return v
	}
	if v, ok := ComputeSettings[name]; ok {
		return v
	}
	if v, ok := ModelSettings[name]; ok {
		return v
	}
	if v, ok := EvolveSettings[name]; ok {
		return v
	}
	if v, ok := DaemonSettings[name]; ok {
		return v
	}
	if v, ok := LeadSettings[name]; ok {
		return v
	}
	return OwnerSettings[name]
}

// NoneWords are the spellings of "no cap" for count-or-none settings.
var NoneWords = []string{"none", "off", "unlimited"}

// SettingNames lists the known settings sorted by name.
func SettingNames() []string {
	names := make([]string, 0, len(DefaultSettings))
	for name := range DefaultSettings {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

// ComputeSettingNames lists the compute.* settings sorted by name.
func ComputeSettingNames() []string { return sortedKeys(ComputeSettings) }

// ModelSettingNames lists the model settings that are not written at Create (light_model).
func ModelSettingNames() []string { return sortedKeys(ModelSettings) }

// EvolveSettingNames lists the evolve.* settings sorted by name.
func EvolveSettingNames() []string { return sortedKeys(EvolveSettings) }

// DaemonSettingNames lists the daemon.* settings sorted by name.
func DaemonSettingNames() []string { return sortedKeys(DaemonSettings) }

// LeadSettingNames lists the lead.* settings sorted by name.
func LeadSettingNames() []string { return sortedKeys(LeadSettings) }

// OwnerSettingNames lists the owner.* settings sorted by name.
func OwnerSettingNames() []string { return sortedKeys(OwnerSettings) }

// ExtraSettingNames lists every setting that is not written at Create
// (light_model, then compute.*, evolve.*, daemon.*, lead.*, then owner.*);
// inspect and the hub list these after the legacy names with their
// effective values.
func ExtraSettingNames() []string {
	names := append(ModelSettingNames(), ComputeSettingNames()...)
	return append(append(append(append(names, EvolveSettingNames()...), DaemonSettingNames()...), LeadSettingNames()...), OwnerSettingNames()...)
}

func sortedKeys(m map[string]string) []string {
	names := make([]string, 0, len(m))
	for name := range m {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

var fractionRe = regexp.MustCompile(`^\s*(\d+)\s*/\s*(\d+)\s*$`)

// Majority parses "n/m" like legacy/pulse.py majority().
func Majority(value string) (int, int, error) {
	m := fractionRe.FindStringSubmatch(value)
	if m == nil {
		return 0, 0, inputErr("A majority must be a fraction such as 2/3, not %s", pyRepr(value))
	}
	n, _ := strconv.Atoi(m[1])
	d, _ := strconv.Atoi(m[2])
	if d == 0 {
		return 0, 0, inputErr("A majority must be a fraction such as 2/3, not %s", pyRepr(value))
	}
	return n, d, nil
}

// pyRepr formats a string like Python's repr() for error-message parity.
func pyRepr(s string) string {
	if strings.Contains(s, "'") && !strings.Contains(s, `"`) {
		return `"` + s + `"`
	}
	return "'" + strings.ReplaceAll(strings.ReplaceAll(s, `\`, `\\`), "'", `\'`) + "'"
}

// ValidateSetting returns the normalized value for a setting, or an
// InputError saying what the setting takes (legacy/pulse.py validate_setting).
func ValidateSetting(name, value string) (string, error) {
	rule, ok := knownSetting(name)
	if !ok {
		if strings.HasPrefix(name, "compute.") {
			return "", inputErr("Unknown setting %s. Compute settings: %s", pyRepr(name), strings.Join(ComputeSettingNames(), ", "))
		}
		if strings.HasPrefix(name, "evolve.") {
			return "", inputErr("Unknown setting %s. Evolve settings: %s", pyRepr(name), strings.Join(EvolveSettingNames(), ", "))
		}
		if strings.HasPrefix(name, "owner.") {
			return "", inputErr("Unknown setting %s. Owner settings: %s", pyRepr(name), strings.Join(OwnerSettingNames(), ", "))
		}
		return "", inputErr("Unknown setting %s. Known: %s", pyRepr(name), strings.Join(SettingNames(), ", "))
	}
	text := strings.TrimSpace(value)
	switch rule.Kind {
	case "count", "count-or-none", "credits":
		lower := strings.ToLower(text)
		if rule.Kind == "count-or-none" {
			for _, w := range NoneWords {
				if lower == w {
					return lower, nil
				}
			}
		}
		number, err := strconv.Atoi(text)
		if err != nil {
			extra := ""
			if rule.Kind == "count-or-none" {
				extra = ", or one of " + strings.Join(NoneWords, "/")
			}
			return "", inputErr("%s takes a whole number%s, not %s", name, extra, pyRepr(text))
		}
		if rule.HasMin && number < rule.Min {
			return "", inputErr("%s must be at least %d, not %d", name, rule.Min, number)
		}
		return strconv.Itoa(number), nil
	case "fraction":
		n, d, err := Majority(text)
		if err != nil {
			return "", err
		}
		if n > d {
			return "", inputErr("%s is a share of the votes: the numerator cannot exceed the denominator (%s)", name, text)
		}
		return strconv.Itoa(n) + "/" + strconv.Itoa(d), nil
	case "decimal":
		f, err := strconv.ParseFloat(text, 64)
		if err != nil || math.IsNaN(f) || math.IsInf(f, 0) {
			return "", inputErr("%s takes a decimal number such as 0.02, not %s", name, pyRepr(text))
		}
		if f < 0 {
			return "", inputErr("%s cannot be negative, not %s", name, text)
		}
		if rule.HasMax && f > rule.Max {
			return "", inputErr("%s is a share: it cannot exceed %s, not %s", name, strconv.FormatFloat(rule.Max, 'f', -1, 64), text)
		}
		return strconv.FormatFloat(f, 'f', -1, 64), nil
	case "switch", "choice":
		lower := strings.ToLower(text)
		for _, c := range rule.Choices {
			if lower == c {
				return lower, nil
			}
		}
		return "", inputErr("%s takes one of %s, not %s", name, strings.Join(rule.Choices, ", "), pyRepr(text))
	case "command":
		if text == "" {
			return "", inputErr("%s cannot be empty", name)
		}
	case "identifier":
		if SecretShaped(text) {
			return "", inputErr("%s looks like a credential; PULSE never stores secrets - authenticate on this machine (az login, aws configure, gcloud auth login) and keep the setting to an identifier", name)
		}
	}
	return text, nil
}

// Setting returns the stored value, else def, else the built-in default
// (legacy/pulse.py setting()).
func (s *Store) Setting(key, def string) string {
	var value string
	err := s.DB.QueryRow("SELECT value FROM settings WHERE name = ?", key).Scan(&value)
	if err == nil {
		return value
	}
	if def != "" {
		return def
	}
	return defaultSetting(key)
}

// SettingInt is Setting parsed as an integer, falling back to the default.
func (s *Store) SettingInt(key string) int {
	if n, err := strconv.Atoi(s.Setting(key, "")); err == nil {
		return n
	}
	n, _ := strconv.Atoi(defaultSetting(key))
	return n
}

// OwnerAway reports whether the owner has delegated board proposals to project leads: owner.presence is `away`
// and the owner wrote it. Agents run `pulse` with PULSE_AGENT set and may reach the settings table; a value an
// agent wrote does not count, so no agent can hand itself the owner's decisions.
func (s *Store) OwnerAway() bool {
	var value, by string
	if err := s.DB.QueryRow("SELECT value, updated_by FROM settings WHERE name = 'owner.presence'").Scan(&value, &by); err != nil {
		return false
	}
	return value == OwnerAway && HumanActor(by)
}

// SettingRow is one row of the settings table.
type SettingRow struct {
	Name      string `json:"name"`
	Value     string `json:"value"`
	UpdatedAt string `json:"updated_at"`
	UpdatedBy string `json:"updated_by"`
}

// Settings returns every stored setting by name.
func (s *Store) Settings() (map[string]string, error) {
	rows, err := s.SettingRows()
	if err != nil {
		return nil, err
	}
	out := make(map[string]string, len(rows))
	for _, r := range rows {
		out[r.Name] = r.Value
	}
	return out, nil
}

// SettingRows returns the settings table ordered by name.
func (s *Store) SettingRows() ([]SettingRow, error) {
	rows, err := s.DB.Query("SELECT name, value, updated_at, updated_by FROM settings ORDER BY name")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []SettingRow
	for rows.Next() {
		var r SettingRow
		if err := rows.Scan(&r.Name, &r.Value, &r.UpdatedAt, &r.UpdatedBy); err != nil {
			return nil, err
		}
		out = append(out, r)
	}
	return out, rows.Err()
}

// SetSetting validates, upserts and logs a `setting` event exactly like
// legacy/pulse.py set_setting(). The returned error is an *InputError when
// the value is refused.
func (s *Store) SetSetting(actor, key, value string) error {
	normalized, err := ValidateSetting(key, value)
	if err != nil {
		return err
	}
	now := s.Now()
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, ?) "+
		"ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, "+
		"updated_by = excluded.updated_by", key, normalized, now, actor); err != nil {
		return err
	}
	if err := event(tx, now, actor, "setting", key, normalized); err != nil {
		return err
	}
	return tx.Commit()
}

// Meta returns one meta value ("" and false when absent).
func (s *Store) Meta(key string) (string, bool, error) {
	var value string
	err := s.DB.QueryRow("SELECT value FROM meta WHERE key = ?", key).Scan(&value)
	if errors.Is(err, sql.ErrNoRows) {
		return "", false, nil
	}
	if err != nil {
		return "", false, err
	}
	return value, true, nil
}
