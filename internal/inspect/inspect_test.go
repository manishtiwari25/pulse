package inspect

import (
	"bytes"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/parity"
	"github.com/manishtiwari25/pulse/internal/store"
)

const parityNow = "2026-09-06T19:00:00Z"

type commandResult struct {
	out, err string
	code     int
}

func invoke(t *testing.T, s *store.Store, asJSON bool, project, command string, args ...string) commandResult {
	t.Helper()
	cmd, ok := cli.Lookup(command)
	if !ok {
		t.Fatalf("command %q not registered", command)
	}
	fs := flag.NewFlagSet(command, flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	if cmd.Flags != nil {
		cmd.Flags(fs)
	}
	if err := fs.Parse(args); err != nil {
		t.Fatal(err)
	}
	var stdout, stderr bytes.Buffer
	c := &cli.Context{Store: s, Out: &stdout, Err: &stderr, Project: project, JSON: asJSON, Now: parityNow}
	if s != nil {
		c.Home = s.Home
	}
	err := cmd.Run(c, fs, fs.Args())
	result := commandResult{out: stdout.String(), err: stderr.String()}
	if err != nil {
		var ce *cli.Error
		var input *store.InputError
		switch {
		case errors.As(err, &ce):
			result.code = ce.ExitCode()
			if ce.Error() != "" {
				result.err += ce.Error() + "\n"
			}
		case errors.As(err, &input):
			result.code = 2
			result.err += "pulse: " + input.Error() + "\n"
		default:
			result.code = 1
			result.err += "pulse: " + err.Error() + "\n"
		}
	}
	return result
}

func temporaryStore(t *testing.T) *store.Store {
	t.Helper()
	home := t.TempDir()
	t.Setenv("PULSE_HOME", home)
	t.Setenv("PULSE_JOB", "")
	t.Setenv("PULSE_AGENT", "")
	t.Setenv("PULSE_PROJECT", "")
	t.Setenv("PULSE_NOW", parityNow)
	s, err := store.Open(store.Options{Home: home, Create: true, Now: parityNow})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := s.Close(); err != nil {
			t.Error(err)
		}
	})
	return s
}

func snapshotStore(t *testing.T) *store.Store {
	t.Helper()
	home := "/tmp/pulse-parity"
	if _, err := os.Stat(filepath.Join(home, store.StoreName)); os.IsNotExist(err) {
		t.Skip("optional parity snapshot is absent")
	} else if err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(store.Options{Home: home, ReadOnly: true, SkipRecords: true, Now: parityNow})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

// The shared parity package was not yet available when this port landed.
// A read-only connection is essential: Python status otherwise prunes meta.unfit.
// Freezing utc_now also covers status, which has no Python --now flag.
func oracle(t *testing.T, home string, readOnly bool, args ...string) commandResult {
	t.Helper()
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Skip("optional Python parity oracle is unavailable")
	}
	dir := os.Getenv("PULSE_ORACLE_DIR")
	if dir == "" {
		dir = "/tmp/pulse-oracle"
	}
	script := filepath.Join(dir, "pulse.py")
	if _, err := os.Stat(script); os.IsNotExist(err) {
		t.Skip("optional pulse.py parity oracle is unavailable")
	} else if err != nil {
		t.Fatal(err)
	}
	const harness = `import importlib.util, os, sys
spec = importlib.util.spec_from_file_location("inspect_oracle", sys.argv[1])
p = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = p
spec.loader.exec_module(p)
p.utc_now = lambda: os.environ["PULSE_NOW"]
if sys.argv[2] == "true":
    connect = p.connect
    p.connect = lambda home=None, create=False, readonly=False: connect(home, readonly=True)
raise SystemExit(p.main(["--home", os.environ["PULSE_HOME"]] + sys.argv[3:]))
`
	argv := append([]string{"-c", harness, script, strconv.FormatBool(readOnly)}, args...)
	cmd := exec.Command(python, argv...)
	cmd.Env = append(os.Environ(), "PULSE_HOME="+home, "PULSE_NOW="+parityNow,
		"PULSE_AGENT=", "PULSE_PROJECT=", "PULSE_JOB=inspect-parity", "PYTHONDONTWRITEBYTECODE=1")
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	runErr := cmd.Run()
	result := commandResult{out: stdout.String(), err: stderr.String()}
	if runErr != nil {
		var exit *exec.ExitError
		if !errors.As(runErr, &exit) {
			t.Fatal(runErr)
		}
		result.code = exit.ExitCode()
	}
	return result
}

func trimTrailingSpaces(text string) string {
	lines := strings.Split(text, "\n")
	for i := range lines {
		lines[i] = strings.TrimRight(lines[i], " \t")
	}
	return strings.Join(lines, "\n")
}

func assertParity(t *testing.T, got, want commandResult) {
	t.Helper()
	want.out, want.err = parity.Expected(want.out), parity.Expected(want.err)
	if got.code != want.code || trimTrailingSpaces(got.out) != trimTrailingSpaces(want.out) || trimTrailingSpaces(got.err) != trimTrailingSpaces(want.err) {
		t.Fatalf("Go code=%d stdout:\n%s\nstderr:\n%s\nPython code=%d stdout:\n%s\nstderr:\n%s",
			got.code, got.out, got.err, want.code, want.out, want.err)
	}
}

func TestSnapshotParity(t *testing.T) {
	s := snapshotStore(t)
	t.Setenv("PULSE_AGENT", "")
	t.Setenv("PULSE_PROJECT", "")
	projects, err := s.Projects(true)
	if err != nil {
		t.Fatal(err)
	}
	if len(projects) != 3 {
		t.Fatalf("expected the three-project snapshot; got %d", len(projects))
	}
	cases := []struct {
		name, project string
		args          []string
	}{
		{name: "status"},
		{name: "projects"},
		{name: "jobs"},
		{name: "jobs", args: []string{"--all"}},
		{name: "agents"},
		{name: "agents", args: []string{"--all"}},
		{name: "events"},
		{name: "events", args: []string{"--limit", "0"}},
		{name: "events", args: []string{"--limit", "3"}},
		{name: "settings"},
		{name: "settings", args: []string{"list"}},
		{name: "settings", args: []string{"get"}},
		{name: "settings", args: []string{"get", "jobs_per_day"}},
		{name: "settings", args: []string{"get", "not-a-setting"}},
		{name: "schema"},
		{name: "signers"},
	}
	for _, p := range projects {
		cases = append(cases,
			struct {
				name, project string
				args          []string
			}{"jobs", p.Slug, []string{"--all"}},
			struct {
				name, project string
				args          []string
			}{"signers", p.Slug, nil},
		)
	}
	for _, tc := range cases {
		t.Run(strings.Join(append([]string{tc.name, tc.project}, tc.args...), " "), func(t *testing.T) {
			args := append([]string{tc.name}, tc.args...)
			if tc.project != "" {
				args = append(args, "--project", tc.project)
			}
			got := invoke(t, s, false, tc.project, tc.name, tc.args...)
			want := oracle(t, s.Home, true, args...)
			if tc.name == "settings" && len(tc.args) <= 1 {
				// The original stored rows are unchanged; compute/evolve defaults
				// are intentionally appended after the Python-compatible prefix.
				lines := strings.SplitAfter(got.out, "\n")
				legacyLines := strings.Count(want.out, "\n")
				if len(lines) <= legacyLines {
					t.Fatal("settings did not append the extra namespaces")
				}
				got.out = strings.Join(lines[:legacyLines], "")
			}
			assertParity(t, got, want)
		})
	}
}

func TestStatusUsesActualStorePath(t *testing.T) {
	s := temporaryStore(t)
	t.Setenv("PULSE_HOME", filepath.Join(t.TempDir(), "not-the-open-store"))
	got := invoke(t, s, false, "", "status")
	if got.code != 0 || !strings.HasPrefix(got.out, "store      "+s.Path+" (schema 3)\n") {
		t.Fatalf("status did not identify the store actually opened: %+v", got)
	}
	// Intentional difference: Python status_text calls store_path() with the
	// default home even when --home selected a different database.
}

func TestSnapshotJSONStable(t *testing.T) {
	s := snapshotStore(t)
	for _, name := range []string{"status", "projects", "jobs", "agents", "events", "settings", "signers"} {
		t.Run(name, func(t *testing.T) {
			var args []string
			if name == "jobs" || name == "agents" {
				args = []string{"--all"}
			}
			got := invoke(t, s, true, "", name, args...)
			again := invoke(t, s, true, "", name, args...)
			if got.code != 0 || got != again {
				t.Fatalf("unstable or unsuccessful JSON: %d %s", got.code, got.err)
			}
			var value any
			if err := json.Unmarshal([]byte(got.out), &value); err != nil {
				t.Fatal(err)
			}
			assertSnakeCase(t, value)
			if name != "status" {
				if _, ok := value.([]any); !ok {
					t.Fatalf("%s should return an array of objects", name)
				}
			}
		})
	}
}

func TestSchemaWithoutStore(t *testing.T) {
	cmd, _ := cli.Lookup("schema")
	if !cmd.NoStore {
		t.Fatal("schema must not open a store")
	}
	got := invoke(t, nil, false, "", "schema")
	if got.code != 0 || got.out != store.DDL {
		t.Fatalf("schema differs from store.DDL: %+v", got)
	}
}

func TestRegistrationAndJSON(t *testing.T) {
	s := temporaryStore(t)
	for _, name := range []string{"status", "projects", "jobs", "agents", "events", "schema", "signers"} {
		t.Run(name, func(t *testing.T) {
			cmd, _ := cli.Lookup(name)
			if cmd.Group != "inspect" || (!cmd.ReadOnly && !cmd.NoStore) {
				t.Fatalf("wrong registry metadata: %+v", cmd)
			}
			first := invoke(t, s, true, "", name)
			second := invoke(t, s, true, "", name)
			if first.code != 0 || first != second {
				t.Fatalf("JSON is not stable: first %+v second %+v", first, second)
			}
			var value any
			if err := json.Unmarshal([]byte(first.out), &value); err != nil {
				t.Fatal(err)
			}
			if slices.Contains([]string{"projects", "jobs", "agents", "events", "signers"}, name) {
				if _, ok := value.([]any); !ok {
					t.Fatalf("list must be an array, got %T: %s", value, first.out)
				}
			}
			assertSnakeCase(t, value)
		})
	}
	for _, name := range []string{"settings", "start", "stop"} {
		cmd, _ := cli.Lookup(name)
		if cmd.Group != "human" || cmd.ReadOnly || cmd.NoStore {
			t.Fatalf("wrong human command registry metadata: %+v", cmd)
		}
	}
}

var snakeCase = regexp.MustCompile(`^[a-z][a-z0-9]*(?:_[a-z0-9]+)*$`)

func assertSnakeCase(t *testing.T, value any) {
	t.Helper()
	switch v := value.(type) {
	case map[string]any:
		for key, child := range v {
			if !snakeCase.MatchString(key) {
				t.Errorf("JSON key is not snake_case: %q", key)
			}
			assertSnakeCase(t, child)
		}
	case []any:
		for _, child := range v {
			assertSnakeCase(t, child)
		}
	}
}

func TestSettingsValidationParity(t *testing.T) {
	s := temporaryStore(t)
	cases := [][]string{
		{"set"}, {"set", "workers"},
		{"set", "not-a-setting", "1"},
		{"set", "workers", "many"},
		{"set", "workers", "-1"},
		{"set", "parallel_per_project", "0"},
		{"set", "jobs_per_day", "two"},
		{"set", "jobs_per_day", "-3"},
		{"set", "max_ai_credits", "29"},
		{"set", "amend_majority", "no"},
		{"set", "amend_majority", "1/0"},
		{"set", "amend_majority", "3/2"},
		{"set", "stop", "yes"},
		{"set", "landing", "automatic"},
		{"set", "runner_cmd", "  "},
	}
	for _, args := range cases {
		t.Run(strings.Join(args, " "), func(t *testing.T) {
			eventsBefore := eventCount(t, s)
			got := invoke(t, s, false, "", "settings", args...)
			if got.code != 2 {
				t.Fatalf("invalid setting returned %d: %+v", got.code, got)
			}
			if eventCount(t, s) != eventsBefore {
				t.Fatal("refused setting emitted an event")
			}
			want := oracle(t, s.Home, false, append([]string{"settings"}, args...)...)
			assertParity(t, got, want)
			if eventCount(t, s) != eventsBefore {
				t.Fatal("refused setting emitted an event")
			}
		})
	}
}

func TestSettingsSuccessfulWrites(t *testing.T) {
	s := temporaryStore(t)
	for _, tc := range []struct{ name, input, normalized string }{
		{"workers", " 0002 ", "2"},
		{"jobs_per_day", "OFF", "off"},
		{"amend_majority", " 2 / 3 ", "2/3"},
		{"landing", "HUMAN", "human"},
		{"worker_model", "", ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			before := eventCount(t, s)
			got := invoke(t, s, false, "", "settings", "set", tc.name, tc.input)
			if got.code != 0 || got.out != tc.name+"="+tc.input+"\n" {
				t.Fatalf("unexpected set output: %+v", got)
			}
			if value := s.Setting(tc.name, ""); value != tc.normalized {
				t.Fatalf("stored %q, want %q", value, tc.normalized)
			}
			checkSettingEvent(t, s, tc.name, tc.normalized, "human")
			if eventCount(t, s) != before+1 {
				t.Fatal("set must log exactly one setting event")
			}
			want := oracle(t, s.Home, false, "settings", "set", tc.name, tc.input)
			assertParity(t, got, want)
		})
	}
}

func TestSettingsReport(t *testing.T) {
	s := temporaryStore(t)
	before := eventCount(t, s)
	report, err := SettingsReport(s)
	if err != nil {
		t.Fatal(err)
	}
	wantNames := append(store.SettingNames(), store.ExtraSettingNames()...)
	gotNames := make([]string, len(report))
	for i, row := range report {
		gotNames[i] = row.Name
		if row.Kind == "" || row.Description == "" {
			t.Errorf("missing metadata: %+v", row)
		}
	}
	if !reflect.DeepEqual(gotNames, wantNames) {
		t.Fatalf("report names %v, want %v", gotNames, wantNames)
	}
	got := invoke(t, s, true, "", "settings")
	var decoded []SettingReport
	if err := json.Unmarshal([]byte(got.out), &decoded); err != nil || !reflect.DeepEqual(decoded, report) {
		t.Fatalf("JSON report differs: %v: %+v", err, got)
	}
	if eventCount(t, s) != before {
		t.Fatal("report must not materialize default settings")
	}
	get := invoke(t, s, false, "", "settings", "get", "compute.provider")
	if get.out != "azure\n" {
		t.Fatalf("compute default not accessible: %+v", get)
	}
}

func TestSettingsListAppendsEffectiveExtras(t *testing.T) {
	s := temporaryStore(t)
	legacyRows, err := s.SettingRows()
	if err != nil {
		t.Fatal(err)
	}
	if len(legacyRows) != 16 {
		t.Fatalf("fresh store has %d legacy settings, want 16", len(legacyRows))
	}
	var prefix strings.Builder
	for _, row := range legacyRows {
		fmt.Fprintf(&prefix, "%-18s %-40s %s %s\n", row.Name, row.Value, row.UpdatedAt, row.UpdatedBy)
	}
	before := eventCount(t, s)
	for _, args := range [][]string{nil, {"list"}, {"get"}} {
		got := invoke(t, s, false, "", "settings", args...)
		var want strings.Builder
		want.WriteString(prefix.String())
		for _, name := range store.ExtraSettingNames() {
			fmt.Fprintf(&want, "%-18s %-40s %s %s\n", name, s.Setting(name, ""), "", "default")
		}
		if got.code != 0 || got.out != want.String() {
			t.Fatalf("wrong settings list:\n%s\nwant:\n%s", got.out, want.String())
		}
	}
	rows, err := s.SettingRows()
	if err != nil || len(rows) != len(legacyRows) || eventCount(t, s) != before {
		t.Fatal("reading defaults must not materialize settings or emit events")
	}
	if err := s.SetSetting("owner", "compute.max_vms", "2"); err != nil {
		t.Fatal(err)
	}
	got := invoke(t, s, false, "", "settings")
	if !strings.HasPrefix(got.out, prefix.String()) ||
		!strings.Contains(got.out, fmt.Sprintf("%-18s %-40s %s %s\n", "compute.max_vms", "2", parityNow, "owner")) {
		t.Fatalf("stored extra setting lost its order or provenance:\n%s", got.out)
	}
	if strings.Count(got.out, "compute.max_vms") != 1 {
		t.Fatal("stored extra setting was listed twice")
	}
}

func TestHeldoutPathNeverAppearsInAgentViews(t *testing.T) {
	s := temporaryStore(t)
	privatePath := filepath.Join(t.TempDir(), "owner-fixtures")
	for _, asJSON := range []bool{false, true} {
		set := invoke(t, s, asJSON, "", "settings", "set", "evolve.heldout_dir", privatePath)
		if set.code != 0 || strings.Contains(set.out+set.err, privatePath) {
			t.Fatalf("settings set exposed the held-out path: %+v", set)
		}
		for _, tc := range []struct {
			name string
			args []string
		}{
			{"settings", nil},
			{"settings", []string{"get", "evolve.heldout_dir"}},
			{"status", nil},
			{"events", nil},
		} {
			got := invoke(t, s, asJSON, "", tc.name, tc.args...)
			if got.code != 0 || strings.Contains(got.out+got.err, privatePath) || !strings.Contains(got.out, "[owner-private]") {
				t.Fatalf("%s JSON=%t did not keep the path private: %+v", tc.name, asJSON, got)
			}
		}
	}
	if got := s.Setting("evolve.heldout_dir", ""); got != privatePath {
		t.Fatal("redaction must not overwrite the owner's stored path")
	}
}

func TestEventsRedactQuotedAndHistoricalPrivatePaths(t *testing.T) {
	s := temporaryStore(t)
	oldPath := filepath.Join(t.TempDir(), "priv\u00e9-\U0001f512", `"fixtures"\<heldout>&`)
	newPath := filepath.Join(t.TempDir(), "replacement-fixtures")
	if err := s.SetSetting("human", "evolve.heldout_dir", oldPath); err != nil {
		t.Fatal(err)
	}
	payload := map[string]any{"path": oldPath, "message": "scoring in " + oldPath}
	pythonJSON, err := store.PyJSON(payload)
	if err != nil {
		t.Fatal(err)
	}
	htmlJSON, err := json.Marshal(payload)
	if err != nil {
		t.Fatal(err)
	}
	var plainJSON bytes.Buffer
	encoder := json.NewEncoder(&plainJSON)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(payload); err != nil {
		t.Fatal(err)
	}
	doubleJSON, err := json.Marshal(pythonJSON)
	if err != nil {
		t.Fatal(err)
	}
	for _, detail := range []string{pythonJSON, string(htmlJSON), strings.TrimSpace(plainJSON.String()), string(doubleJSON)} {
		if err := s.Event("human", "fixture.quoted", oldPath+"/result", detail); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.SetSetting("human", "evolve.heldout_dir", newPath); err != nil {
		t.Fatal(err)
	}
	if err := s.Event("human", "fixture.current", newPath+"/result", "scoring in "+newPath); err != nil {
		t.Fatal(err)
	}
	for _, cleared := range []bool{false, true} {
		if cleared {
			if err := s.SetSetting("human", "evolve.heldout_dir", ""); err != nil {
				t.Fatal(err)
			}
		}
		before, err := Events(s, "", "", -1)
		if err != nil {
			t.Fatal(err)
		}
		for _, asJSON := range []bool{false, true} {
			got := invoke(t, s, asJSON, "", "events", "--limit", "-1")
			if got.code != 0 || strings.Contains(got.out+got.err, oldPath) || strings.Contains(got.out+got.err, newPath) {
				t.Fatalf("events JSON=%t cleared=%t exposed a private path: %+v", asJSON, cleared, got)
			}
			if !strings.Contains(got.out, "[owner-private]/result") {
				t.Fatalf("ordinary event subjects were not redacted: %+v", got)
			}
			if asJSON {
				var events []Event
				if err := json.Unmarshal([]byte(got.out), &events); err != nil {
					t.Fatal(err)
				}
				for _, event := range events {
					if event.Action != "fixture.quoted" {
						continue
					}
					var decoded any
					if err := json.Unmarshal([]byte(event.Detail), &decoded); err != nil {
						t.Fatalf("redaction corrupted detail JSON: %s: %v", event.Detail, err)
					}
					if inner, ok := decoded.(string); ok {
						if err := json.Unmarshal([]byte(inner), &decoded); err != nil {
							t.Fatalf("redaction corrupted double-encoded JSON: %s: %v", inner, err)
						}
					}
					text := fmt.Sprint(decoded)
					if strings.Contains(text, oldPath) || strings.Contains(text, newPath) || !strings.Contains(text, "[owner-private]") {
						t.Fatalf("decoded event detail exposed a path: %s", text)
					}
				}
			}
		}
		after, err := Events(s, "", "", -1)
		if err != nil || !reflect.DeepEqual(before, after) {
			t.Fatal("redaction changed canonical event rows")
		}
	}
}

func eventCount(t *testing.T, s *store.Store) int {
	t.Helper()
	var n int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events").Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

func checkSettingEvent(t *testing.T, s *store.Store, name, value, actor string) {
	t.Helper()
	var at, gotActor, action, subject, detail string
	if err := s.DB.QueryRow("SELECT at, actor, action, subject, detail FROM events ORDER BY seq DESC LIMIT 1").
		Scan(&at, &gotActor, &action, &subject, &detail); err != nil {
		t.Fatal(err)
	}
	if at != parityNow || gotActor != actor || action != "setting" || subject != name || detail != value {
		t.Fatalf("unexpected setting event: %q %q %q %q %q", at, gotActor, action, subject, detail)
	}
}

func TestStopStartRoundTrip(t *testing.T) {
	s := temporaryStore(t)
	if _, err := s.DB.Exec("INSERT INTO meta(key,value) VALUES ('daemon','{\"pid\":123,\"note\":\"exited\",\"at\":\"2026-09-06T18:00:00Z\"}')"); err != nil {
		t.Fatal(err)
	}
	daemonBefore, _, err := s.Meta("daemon")
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ name, value, output string }{
		{"stop", "1", "runner stopped\n"}, {"start", "0", "runner allowed\n"},
	} {
		before := eventCount(t, s)
		got := invoke(t, s, false, "", tc.name)
		if got.code != 0 || got.out != tc.output || s.Setting("stop", "") != tc.value {
			t.Fatalf("kill switch failed: %+v", got)
		}
		checkSettingEvent(t, s, "stop", tc.value, "human")
		if eventCount(t, s) != before+1 {
			t.Fatal("kill switch must log exactly one setting event")
		}
		reason, err := RunnerStopped(s)
		if err != nil || (reason != "") != (tc.value == "1") {
			t.Fatalf("runner switch mismatch: %q %v", reason, err)
		}
		t.Run(tc.name+" parity", func(t *testing.T) {
			want := oracle(t, s.Home, false, tc.name)
			assertParity(t, got, want)
		})
	}
	daemonAfter, _, err := s.Meta("daemon")
	if err != nil || daemonAfter != daemonBefore {
		t.Fatal("start/stop must not rewrite daemon metadata")
	}
	t.Setenv("PULSE_AGENT", "inspect-agent")
	if got := invoke(t, s, false, "", "stop"); got.code != 0 {
		t.Fatalf("agent switch failed: %+v", got)
	}
	checkSettingEvent(t, s, "stop", "1", "inspect-agent")
}

func TestEventsFilters(t *testing.T) {
	s := temporaryStore(t)
	execSQL(t, s, `INSERT INTO jobs(id,project,kind,title,created_at) VALUES ('job-one','one','task','test',?)`, parityNow)
	execSQL(t, s, `INSERT INTO posts(id,project,area,thread,kind,agent,date,created_at) VALUES ('post-one','one','a','t','finding','human',?,?)`, parityNow, parityNow)
	for _, e := range []struct{ subject, detail string }{
		{"one", "direct"}, {"job-one", "job"}, {"post-one", "post"},
		{"local", `{"project":"one"}`}, {"two", `{"project":"two"}`},
	} {
		if err := s.Event("human", "test", e.subject, e.detail); err != nil {
			t.Fatal(err)
		}
	}
	events, err := Events(s, "one", "", -1)
	if err != nil || len(events) != 4 {
		t.Fatalf("project filter: %d events, %v", len(events), err)
	}
	latest, err := Events(s, "one", "", 1)
	if err != nil || len(latest) != 1 || latest[0].Subject != "local" {
		t.Fatalf("limit/order: %+v %v", latest, err)
	}
	after, err := Events(s, "one", strconv.FormatInt(events[2].Seq, 10), 40)
	if err != nil || len(after) != 2 {
		t.Fatalf("sequence cursor: %+v %v", after, err)
	}
	at, err := Events(s, "one", parityNow, 40)
	if err != nil || len(at) != 0 {
		t.Fatalf("timestamp must be exclusive: %+v %v", at, err)
	}
	earlier, err := Events(s, "one", "2026-09-06T18:59:59Z", 40)
	if err != nil || len(earlier) != 4 {
		t.Fatalf("timestamp filter: %+v %v", earlier, err)
	}
	for _, since := range []string{"-1", "yesterday", "2026-99-99T19:00:00Z"} {
		if _, err := Events(s, "", since, 40); err == nil {
			t.Fatalf("invalid cursor %q accepted", since)
		}
	}
}

func execSQL(t *testing.T, s *store.Store, q string, args ...any) {
	t.Helper()
	if _, err := s.DB.Exec(q, args...); err != nil {
		t.Fatal(err)
	}
}

func TestScorecardsPruningAndCounts(t *testing.T) {
	s := temporaryStore(t)
	for _, a := range []struct{ name, status, active string }{
		{"old", "pruned", "2026-01-01T00:00:00Z"},
		{"b", "active", parityNow}, {"a", "active", parityNow},
	} {
		execSQL(t, s, `INSERT INTO agents(name,status,created_at,last_active_at,posts,jobs_done,jobs_failed)
			VALUES (?,?,?, ?, 99,2,3)`, a.name, a.status, parityNow, a.active)
	}
	for i, kind := range []string{"finding", "suggestion", "vote", "vote", "answer", "result"} {
		execSQL(t, s, `INSERT INTO posts(id,project,area,thread,kind,agent,date,created_at) VALUES (?,'p','a','t',?,'old',?,?)`,
			fmt.Sprintf("post-%d", i), kind, parityNow, parityNow)
	}
	all, err := AgentScorecards(s, true)
	if err != nil || len(all) != 4 {
		t.Fatalf("all scorecards: %+v %v", all, err)
	}
	old := all[len(all)-1]
	if old.Name != "old" || old.Posts != 99 || old.N != 6 || old.Findings != 1 || old.Suggestions != 1 || old.Votes != 2 ||
		old.Answers != 1 || old.Results != 1 || old.LastPost != parityNow {
		t.Fatalf("scorecard must use actual posts, including pruned authors: %+v", old)
	}
	if all[0].Name != "a" || all[1].Name != "b" {
		t.Fatalf("wrong scorecard order: %+v", all)
	}
	active, err := AgentScorecards(s, false)
	if err != nil || len(active) != 3 {
		t.Fatalf("pruned filter: %+v %v", active, err)
	}
	for _, args := range [][]string{nil, {"--all"}} {
		assertParity(t, invoke(t, s, false, "", "agents", args...), oracle(t, s.Home, true, append([]string{"agents"}, args...)...))
	}
}

func TestTruncUnicode(t *testing.T) {
	text := strings.Repeat("\u754c", 10)
	if got := trunc(text, 5); got != strings.Repeat("\u754c", 4)+"~" {
		t.Fatalf("truncation must count Unicode code points like Python: %q", got)
	}
}
