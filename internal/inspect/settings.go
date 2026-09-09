package inspect

import (
	"errors"
	"flag"
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func settingsUsage() string {
	return fmt.Sprintf(`pulse settings [get NAME | set NAME VALUE | list | help] [--json]

Runner and evolution caps (built-in defaults):
  minutes_per_job  default %s minutes
    Worker wall-clock cap; also the deadline for each measurement harness and
    scorer. Measurement requires a positive value.
  max_ai_credits   default %s credits (minimum %d)
    Passed to Copilot workers; not automatically applied to measurement harnesses.
  evolve.limits   default %s
    Optional loop budget: cycles=N,hours=H. none does not remove runner caps.

evolve_minutes and evolve_credits are not supported by this Go CLI; they are
not aliases for the settings above.

Use pulse settings get NAME for an effective value, or pulse settings list --json
for values, defaults, validation rules and descriptions. Only the owner changes caps.`,
		store.DefaultSettings["minutes_per_job"],
		store.DefaultSettings["max_ai_credits"], store.SettingRules["max_ai_credits"].Min,
		store.EvolveSettings["evolve.limits"])
}

type SettingReport struct {
	Name        string   `json:"name"`
	Value       string   `json:"value"`
	Default     string   `json:"default"`
	Kind        string   `json:"kind"`
	Min         *int     `json:"min"`
	Choices     []string `json:"choices"`
	Description string   `json:"description"`
	UpdatedAt   string   `json:"updated_at"`
	UpdatedBy   string   `json:"updated_by"`
}

// SettingsReport is the Python settings_report whitelist, extended with the
// compute and evolution settings whose defaults are not written into old stores.
func SettingsReport(s *store.Store) ([]SettingReport, error) {
	rows, err := s.SettingRows()
	if err != nil {
		return nil, err
	}
	stored := make(map[string]store.SettingRow, len(rows))
	for _, r := range rows {
		stored[r.Name] = r
	}
	names := append(store.SettingNames(), store.ExtraSettingNames()...)
	report := make([]SettingReport, 0, len(names))
	for _, name := range names {
		def, rule := store.DefaultSettings[name], store.SettingRules[name]
		if d, ok := store.ComputeSettings[name]; ok {
			def, rule = d, store.ComputeRules[name]
		}
		if d, ok := store.EvolveSettings[name]; ok {
			def, rule = d, store.EvolveRules[name]
		}
		if d, ok := store.DaemonSettings[name]; ok {
			def, rule = d, store.DaemonRules[name]
		}
		if d, ok := store.LeadSettings[name]; ok {
			def, rule = d, store.LeadRules[name]
		}
		if d, ok := store.OwnerSettings[name]; ok {
			def, rule = d, store.OwnerRules[name]
		}
		r := SettingReport{Name: name, Value: def, Default: def, Kind: rule.Kind, Choices: rule.Choices, Description: rule.Description}
		if rule.HasMin {
			minimum := rule.Min
			r.Min = &minimum
		}
		if row, ok := stored[name]; ok {
			r.Value, r.UpdatedAt, r.UpdatedBy = row.Value, row.UpdatedAt, row.UpdatedBy
		} else if _, legacy := store.DefaultSettings[name]; !legacy {
			r.UpdatedBy = "default"
		}
		report = append(report, r)
	}
	return report, nil
}

func listedSettings(s *store.Store) ([]store.SettingRow, error) {
	stored, err := s.SettingRows()
	if err != nil {
		return nil, err
	}
	extraNames := store.ExtraSettingNames()
	extras := make(map[string]store.SettingRow, len(extraNames))
	for _, name := range extraNames {
		extras[name] = store.SettingRow{Name: name, Value: s.Setting(name, ""), UpdatedBy: "default"}
	}
	rows := make([]store.SettingRow, 0, len(stored)+len(extras))
	for _, row := range stored {
		if _, extra := extras[row.Name]; extra {
			extras[row.Name] = row
		} else {
			rows = append(rows, row)
		}
	}
	for _, name := range extraNames {
		rows = append(rows, extras[name])
	}
	return rows, nil
}

// The owner may configure this path, but agent-facing output must not reveal it.
func agentSettingValue(name, value string) string {
	if bus.PrivateSettings[name] && value != "" {
		return bus.PrivateValue
	}
	return value
}

func runSettings(c *cli.Context, _ *flag.FlagSet, args []string) (retErr error) {
	action := "list"
	if len(args) > 0 {
		action = args[0]
	}
	if action == "help" {
		if err := noArgs(args[1:]); err != nil {
			return err
		}
		return flag.ErrHelp
	}
	if len(args) > 3 {
		return noArgs(args[3:])
	}
	if c.Store == nil {
		s, err := store.Open(store.Options{Home: c.Home, Now: c.Now})
		if err != nil {
			return err
		}
		defer func() { retErr = errors.Join(retErr, s.Close()) }()
		copy := *c
		copy.Store = s
		c = &copy
	}
	switch action {
	case "list", "get":
		if action == "get" && len(args) > 1 {
			values, err := c.Store.Settings()
			if err != nil {
				return err
			}
			name := args[1]
			value, exists := values[name]
			if !exists {
				value, exists = store.DefaultSettings[name]
			}
			if !exists {
				value, exists = store.ComputeSettings[name]
			}
			if !exists {
				value, exists = store.EvolveSettings[name]
			}
			if !exists {
				value, exists = store.DaemonSettings[name]
			}
			if !exists {
				value, exists = store.LeadSettings[name]
			}
			if !exists {
				value, exists = store.OwnerSettings[name]
			}
			value = agentSettingValue(name, value)
			if c.JSON {
				var result any
				if exists {
					result = value
				}
				return jsonOutput(c, struct {
					Name  string `json:"name"`
					Value any    `json:"value"`
				}{name, result})
			}
			if !exists {
				value = "None"
			}
			c.Println(value)
			return nil
		}
		if c.JSON {
			report, err := SettingsReport(c.Store)
			if err != nil {
				return err
			}
			for i := range report {
				report[i].Value = agentSettingValue(report[i].Name, report[i].Value)
			}
			return jsonOutput(c, report)
		}
		// Preserve the Python rows, then show the new namespaces without
		// materializing their defaults in the store.
		rows, err := listedSettings(c.Store)
		if err != nil {
			return err
		}
		for _, r := range rows {
			c.Printf("%-18s %-40s %s %s\n", r.Name, agentSettingValue(r.Name, r.Value), r.UpdatedAt, r.UpdatedBy)
		}
		return nil
	case "set":
		if len(args) < 3 {
			return cli.Errorf(2, "pulse: settings set NAME VALUE", "", "")
		}
		if strings.HasPrefix(args[1], "owner.") && c.Agent != "" && c.Agent != store.HumanAgent {
			return cli.Errorf(2, "pulse: "+args[1]+" is the owner's setting", "it hands the owner's decisions to the leads, so only the owner sets it - in the console or here without PULSE_AGENT",
				"pulse settings get "+args[1])
		}
		if err := c.Store.SetSetting(actor(c), args[1], args[2]); err != nil {
			return inputError(err)
		}
		if c.JSON {
			return jsonOutput(c, struct {
				Name  string `json:"name"`
				Value string `json:"value"`
			}{args[1], agentSettingValue(args[1], c.Store.Setting(args[1], ""))})
		}
		c.Printf("%s=%s\n", args[1], agentSettingValue(args[1], args[2]))
		return nil
	default:
		return cli.Errorf(2, "pulse: settings action must be get, set, list, or help", "", "")
	}
}
