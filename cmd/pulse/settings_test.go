package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func TestSettingsHelpWithoutStore(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := filepath.Join(t.TempDir(), "missing")
	t.Setenv("PULSE_HOME", home)
	var first string
	for _, args := range [][]string{
		{"settings", "help"},
		{"settings", "--help"},
		{"settings", "-h"},
		{"help", "settings"},
	} {
		t.Run(strings.Join(args, " "), func(t *testing.T) {
			var out, errb bytes.Buffer
			if code := run(args, &out, &errb); code != 0 || errb.Len() != 0 {
				t.Fatalf("help: code %d, stdout %q, stderr %q", code, out.String(), errb.String())
			}
			for _, want := range []string{
				"Usage: pulse settings [get NAME | set NAME VALUE | list | help]",
				"minutes_per_job",
				fmt.Sprintf("default %s minutes", store.DefaultSettings["minutes_per_job"]),
				"max_ai_credits",
				fmt.Sprintf("default %s credits", store.DefaultSettings["max_ai_credits"]),
				"evolve.limits",
				fmt.Sprintf("default %s", store.EvolveSettings["evolve.limits"]),
				"evolve_minutes and evolve_credits are not supported",
				"not aliases",
				"pulse settings get NAME",
				"pulse settings list --json",
			} {
				if !strings.Contains(out.String(), want) {
					t.Errorf("help missing %q:\n%s", want, out.String())
				}
			}
			if first == "" {
				first = out.String()
			} else if out.String() != first {
				t.Errorf("help routes disagree:\n%s\nwant:\n%s", out.String(), first)
			}
			if _, err := os.Stat(home); !os.IsNotExist(err) {
				t.Fatalf("help touched the store home: %v", err)
			}
		})
	}
}

func TestSettingsHelpRejectsArguments(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := filepath.Join(t.TempDir(), "missing")
	t.Setenv("PULSE_HOME", home)
	var out, errb bytes.Buffer
	code := run([]string{"settings", "help", "extra"}, &out, &errb)
	if code != 2 || out.Len() != 0 || !strings.Contains(errb.String(), "unrecognized arguments: extra") {
		t.Fatalf("help arguments: code %d, stdout %q, stderr %q", code, out.String(), errb.String())
	}
	if _, err := os.Stat(home); !os.IsNotExist(err) {
		t.Fatalf("invalid help touched the store home: %v", err)
	}
}

func TestSettingsActionsNeedStore(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := filepath.Join(t.TempDir(), "missing")
	t.Setenv("PULSE_HOME", home)
	for _, args := range [][]string{
		{"settings"},
		{"settings", "list"},
		{"settings", "get", "minutes_per_job"},
		{"settings", "set", "minutes_per_job", "11"},
	} {
		var out, errb bytes.Buffer
		code := run(args, &out, &errb)
		if code != 2 || out.Len() != 0 || !strings.Contains(errb.String(), "No store at "+filepath.Join(home, store.StoreName)) {
			t.Fatalf("%v: code %d, stdout %q, stderr %q", args, code, out.String(), errb.String())
		}
		if _, err := os.Stat(home); !os.IsNotExist(err) {
			t.Fatalf("%v created a store home: %v", args, err)
		}
	}
}

func TestSettingsStoreActions(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	t.Setenv("PULSE_AGENT", "")
	home := t.TempDir()
	t.Setenv("PULSE_HOME", home)
	if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		args []string
		want string
	}{
		{[]string{"get", "minutes_per_job"}, store.DefaultSettings["minutes_per_job"] + "\n"},
		{[]string{"set", "minutes_per_job", "11"}, "minutes_per_job=11\n"},
		{[]string{"get", "minutes_per_job"}, "11\n"},
		{[]string{"get", "evolve.limits"}, store.EvolveSettings["evolve.limits"] + "\n"},
	} {
		var out, errb bytes.Buffer
		code := run(append([]string{"settings"}, tc.args...), &out, &errb)
		if code != 0 || out.String() != tc.want || errb.Len() != 0 {
			t.Fatalf("%v: code %d, stdout %q, stderr %q", tc.args, code, out.String(), errb.String())
		}
	}
	for _, name := range []string{"evolve_minutes", "evolve_credits"} {
		var out, errb bytes.Buffer
		code := run([]string{"settings", "set", name, "60"}, &out, &errb)
		if code != 2 || out.Len() != 0 || !strings.Contains(errb.String(), "Unknown setting '"+name+"'") {
			t.Fatalf("unsupported %s: code %d, stdout %q, stderr %q", name, code, out.String(), errb.String())
		}
	}
	var out, errb bytes.Buffer
	if code := run([]string{"settings", "list", "--json"}, &out, &errb); code != 0 || errb.Len() != 0 {
		t.Fatalf("list: code %d, stdout %q, stderr %q", code, out.String(), errb.String())
	}
	var rows []struct {
		Name    string `json:"name"`
		Value   string `json:"value"`
		Default string `json:"default"`
	}
	if err := json.Unmarshal(out.Bytes(), &rows); err != nil {
		t.Fatal(err)
	}
	found := false
	for _, row := range rows {
		if row.Name == "evolve_minutes" || row.Name == "evolve_credits" {
			t.Errorf("unsupported setting advertised as configurable: %+v", row)
		}
		if row.Name == "minutes_per_job" {
			found = true
			if row.Value != "11" || row.Default != store.DefaultSettings[row.Name] {
				t.Errorf("effective value and default lost: %+v", row)
			}
		}
	}
	if !found {
		t.Fatal("settings list omitted minutes_per_job")
	}
}
