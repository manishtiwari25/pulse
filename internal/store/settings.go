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
	"worker_model":         {Kind: "text", Description: "the model the worker runs with; empty means the runner's default"},
	"stop":                 {Kind: "switch", Choices: []string{"0", "1"}, Description: "the kill switch: 1 stops the runner after the runs in flight finish (pulse stop / pulse start)"},
	"landing":              {Kind: "choice", Choices: []string{"agents", "human"}, Description: "agents: verified branches merge into the default branch by themselves; human: they wait for you (ADR-024)"},
}

// ComputeSettings are the owner's compute caps and provider configuration
// (D-D). They are not written at Create (legacy stores stay byte-identical);
// Setting falls back to these defaults. Secret-shaped values are refused:
// Azure authentication is `az login` on the hub machine, never a stored key.
var ComputeSettings = map[string]string{
	"compute.provider":             "azure",
	"compute.azure.subscription":   "",
	"compute.azure.resource_group": "",
	"compute.azure.location":       "",
	"compute.azure.vm_size":        "Standard_B2ms",
	"compute.azure.image":          "Ubuntu2404",
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
	"compute.provider":             {Kind: "choice", Choices: []string{"azure"}, Description: "where agents get more machines from; azure today, more providers later"},
	"compute.azure.subscription":   {Kind: "identifier", Description: "Azure subscription id or name the runner provisions in (auth is az login on this machine)"},
	"compute.azure.resource_group": {Kind: "identifier", Description: "Azure resource group that holds the provisioned hands"},
	"compute.azure.location":       {Kind: "identifier", Description: "Azure region, e.g. westeurope"},
	"compute.azure.vm_size":        {Kind: "identifier", Description: "VM size for a provisioned hand, e.g. Standard_B2ms"},
	"compute.azure.image":          {Kind: "identifier", Description: "VM image alias or URN, e.g. Ubuntu2404"},
	"compute.max_vms":              {Kind: "count", Min: 0, HasMin: true, Description: "provisioned machines that may exist at once; 0 means agents cannot get compute"},
	"compute.max_hours_per_day":    {Kind: "count", Min: 0, HasMin: true, Description: "machine-hours agents may consume per UTC day across provisioned hands; 0 means none"},
	"compute.auto_approve":         {Kind: "switch", Choices: []string{"0", "1"}, Description: "1: requests under the caps are approved without the owner; 0: every request becomes a proposal"},
	"compute.budget_credits":       {Kind: "count", Min: 0, HasMin: true, Description: "AI credits the provisioned hands may spend per UTC day together; 0 means no extra budget"},
	"compute.self_heal":            {Kind: "switch", Choices: []string{"0", "1"}, Description: "1: the daemon places the executor token (pulse executor token) on a drained executor whose login is refused, once per drain; 0: it leaves the login to you (ADR-034)"},
	"compute.self_retire":          {Kind: "switch", Choices: []string{"0", "1"}, Description: "1: the daemon deallocates a VM it created whose login stays refused and retires its executor row, then deletes the VM after compute.retire_grace_hours; 0: it only reports (ADR-034)"},
	"compute.retire_grace_hours":   {Kind: "count", Min: 1, HasMin: true, Description: "hours a retired VM stays deallocated before the daemon deletes it; `az vm start` and `pulse executor activate NAME` within them keep it"},
}

// EvolveSettings configure the self-improvement loop (D-M). Like
// ComputeSettings they are not written at Create; Setting falls back here.
var EvolveSettings = map[string]string{
	"evolve.loop":        "0",
	"evolve.limits":      "none",
	"evolve.tolerance":   "0.02",
	"evolve.heldout_dir": "",
}

// EvolveRules validate EvolveSettings.
var EvolveRules = map[string]SettingRule{
	"evolve.loop":        {Kind: "switch", Choices: []string{"0", "1"}, Description: "1: pulse evolve cycle may run experiments; 0: the loop is off (STOP is the stop setting)"},
	"evolve.limits":      {Kind: "text", Description: "none, or a budget such as cycles=20,hours=48 after which the loop stops by itself"},
	"evolve.tolerance":   {Kind: "decimal", Max: 1, HasMax: true, Description: "score improvement a variant needs over the baseline to be kept, a share between 0 and 1 such as 0.02"},
	"evolve.heldout_dir": {Kind: "identifier", Description: "owner-private directory outside every repository holding held-out fixtures; never quoted to agents"},
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
	if rule, ok := EvolveRules[name]; ok {
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
	return EvolveSettings[name]
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

// EvolveSettingNames lists the evolve.* settings sorted by name.
func EvolveSettingNames() []string { return sortedKeys(EvolveSettings) }

// ExtraSettingNames lists every setting that is not written at Create
// (compute.* then evolve.*); inspect and the hub list these after the legacy
// names with their effective values.
func ExtraSettingNames() []string {
	return append(ComputeSettingNames(), EvolveSettingNames()...)
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
			return "", inputErr("%s looks like a credential; PULSE never stores secrets - authenticate on this machine (az login) and keep the setting to an identifier", name)
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
