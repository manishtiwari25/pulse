package inspect

import (
	"flag"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

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

func runSettings(c *cli.Context, _ *flag.FlagSet, args []string) error {
	action := "list"
	if len(args) > 0 {
		action = args[0]
	}
	if len(args) > 3 {
		return noArgs(args[3:])
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
		return cli.Errorf(2, "pulse: settings action must be get, set, or list", "", "")
	}
}
