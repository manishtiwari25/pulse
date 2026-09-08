package runner

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime/debug"
	"strings"
	"testing"
	"time"
)

func TestPairDryRunHandsDiagnostics(t *testing.T) {
	template := []byte("#!/bin/sh\n# pulse-hands v2.0.0\nexit 0\n")
	templateHash := sha256Bytes(template)
	modifiedHash := sha256Bytes([]byte("#!/bin/sh\n# pulse-hands v2.0.0\nexit 1\n"))
	for _, tc := range []struct {
		name, version, hash, want string
	}{
		{"same", "v2.0.0", templateHash, "present"},
		{"same-version-edited", "v2.0.0", modifiedHash, fmt.Sprintf("differs (installed v2.0.0 %s, template v2.0.0 %s)", modifiedHash[:7], templateHash[:7])},
		{"unrecognized-old-version", "v1.0.0", modifiedHash, fmt.Sprintf("differs (installed v1.0.0 %s, template v2.0.0 %s)", modifiedHash[:7], templateHash[:7])},
		{"shipped-legacy", "", "c75aba6a43bc7db6f640fc145358e5f41304ca2975946e86f0ada9bb79da5118", fmt.Sprintf("older (installed legacy-ccb0790 c75aba6, template v2.0.0 %s)", templateHash[:7])},
		{"edited-legacy", "legacy-ccb0790", "c75aba6a43bc7db6f640fc145358e5f41304ca2975946e86f0ada9bb79da5119", fmt.Sprintf("differs (installed legacy-ccb0790 c75aba6, template v2.0.0 %s)", templateHash[:7])},
		{"unversioned", "", modifiedHash, fmt.Sprintf("differs (installed unknown %s, template v2.0.0 %s)", modifiedHash[:7], templateHash[:7])},
		{"missing-hash", "v1.0.0", "", fmt.Sprintf("differs (installed v1.0.0 unknown, template v2.0.0 %s)", templateHash[:7])},
		{"short-hash", "v1.0.0", "c75aba6", fmt.Sprintf("differs (installed v1.0.0 unknown, template v2.0.0 %s)", templateHash[:7])},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := newStore(t)
			hub := t.TempDir()
			events := count(t, s, "SELECT COUNT(*) AS n FROM events")
			oldSSH, oldBinary := runSSH, handsBinary
			t.Cleanup(func() { runSSH, handsBinary = oldSSH, oldBinary })
			calls, builds := 0, 0
			runSSH = func(_ context.Context, argv []string, stdin []byte, _ time.Duration) (int, string, string, error) {
				calls++
				if !strings.Contains(string(stdin), "printf 'uname=%s\\n'") {
					t.Fatal("dry run sent a non-inspection script")
				}
				return 0, "uname=Linux\narch=x86_64\nsudo=yes\nhands=differs\ndropin_dir=yes\nremotes=0\n" +
					"hands_version=" + tc.version + "\nhands_sha256=" + tc.hash + "\n", "", nil
			}
			handsBinary = func(arch string) ([]byte, string, error) {
				builds++
				if arch != "amd64" {
					t.Fatalf("architecture = %q", arch)
				}
				return template, "fixture", nil
			}
			rep, err := pairExecutor(s, "admin@example.invalid", "hands-two", testNow,
				pairOpts{Workdir: "/srv/pulse", Workers: 1, Projects: "*", DryRun: true, HubHome: hub})
			if err != nil {
				t.Fatal(err)
			}
			var handsLine string
			for _, line := range rep.Lines {
				if strings.HasPrefix(line, "  hands=") {
					handsLine = line
					break
				}
			}
			if handsLine != "  hands="+tc.want {
				t.Errorf("inspection = %q, want %q", handsLine, "  hands="+tc.want)
			}
			if !rep.OK || calls != 1 || builds != 1 {
				t.Errorf("OK=%t, SSH calls=%d, template loads=%d; want true, 1, 1", rep.OK, calls, builds)
			}
			if executorRow(s, "hands-two") != nil || count(t, s, "SELECT COUNT(*) AS n FROM events") != events {
				t.Error("inspection changed the store")
			}
			entries, err := os.ReadDir(hub)
			if err != nil || len(entries) != 0 {
				t.Fatalf("inspection changed the hub home: %v, %v", entries, err)
			}
		})
	}
}

func TestPairHandsBuildVersion(t *testing.T) {
	revision := strings.Repeat("abcdef01", 5)
	for _, tc := range []struct {
		name, moduleVersion, flags, want string
	}{
		{"release", "v2.3.4", "", "v2.3.4"},
		{"development", "(devel)", "", "dev-abcdef01abcd-dirty"},
		{"linked", "(devel)", "-s -w -X " + pairHandsModule + "/internal/cli.version=v2.4.0", "v2.4.0"},
		{"linked-equals", "(devel)", "-X=" + pairHandsModule + "/internal/cli.version=v2.4.1", "v2.4.1"},
		{"linked-quoted", "(devel)", "-X '" + pairHandsModule + "/internal/cli.version=v2.4.2'", "v2.4.2"},
		{"linked-precedence", "v2.3.4", "-X " + pairHandsModule + "/internal/cli.version=v2.4.3", "v2.4.3"},
		{"linked-empty", "v2.3.4", "-X " + pairHandsModule + "/internal/cli.version=", "v2.3.4"},
		{"unrelated-override", "v2.3.4", "-X another/package.version=v9.0.0", "v2.3.4"},
		{"unsafe-version", "(devel)", "-X '" + pairHandsModule + "/internal/cli.version=v2.4.3\nhands=present'", "unknown"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			info := &debug.BuildInfo{
				Main: debug.Module{Path: pairHandsModule, Version: tc.moduleVersion},
				Settings: []debug.BuildSetting{
					{Key: "vcs.revision", Value: revision},
					{Key: "vcs.modified", Value: "true"},
					{Key: "-ldflags", Value: tc.flags},
				},
			}
			if got := pairHandsBuildVersion(info); got != tc.want {
				t.Errorf("build version = %q, want %q", got, tc.want)
			}
			encoded := base64.StdEncoding.EncodeToString([]byte(info.String()))
			if got := pairInstalledHandsVersion(map[string]string{"hands_buildinfo": encoded}); got != tc.want {
				t.Errorf("installed version = %q, want %q", got, tc.want)
			}
		})
	}
	for _, encoded := range []string{"not base64", base64Std("mod\tnot-pulse\tv9.0.0\n"),
		base64Std("build\tbroken\n"), strings.Repeat("A", 12000)} {
		if got := pairInstalledHandsVersion(map[string]string{"hands_buildinfo": encoded}); got != "unknown" {
			t.Errorf("invalid metadata version = %q", got)
		}
	}
	for _, tc := range []struct{ text, want string }{
		{"# pulse-hands bad/version\n", "unknown"},
		{"#!/bin/sh\n# pulse-hands v1\n", "v1"},
		{"#!/bin/sh\r\n# pulse-hands v1\r\n", "v1"},
		{"# pulse-hands " + strings.Repeat("v", 81) + "\n", "unknown"},
	} {
		if got := pairHandsVersion([]byte(tc.text)); got != tc.want {
			t.Errorf("marker version = %q, want %q", got, tc.want)
		}
	}
}

func TestPairInspectHandsScript(t *testing.T) {
	buildInfo := "mod\t" + pairHandsModule + "\t(devel)\t\n" +
		"build\t-ldflags=\"-X " + pairHandsModule + "/internal/cli.version=v3.2.1\"\n" +
		"build\tvcs.revision=" + strings.Repeat("ab", 20) + "\n" +
		"build\tvcs.modified=false\n"
	for _, tc := range []struct {
		name, content, wantVersion              string
		legacy, same, noHash, hashFails, absent bool
	}{
		{name: "script", content: "#!/bin/sh\n# pulse-hands v1.2.3\n", wantVersion: "v1.2.3"},
		{name: "crlf", content: "#!/bin/sh\r\n# pulse-hands v1.2.3\r\n", wantVersion: "v1.2.3"},
		{name: "legacy-path", content: "#!/bin/sh\n# pulse-hands v1.2.3\n", wantVersion: "v1.2.3", legacy: true},
		{name: "matching", content: "#!/bin/sh\n# pulse-hands v1.2.3\n", wantVersion: "v1.2.3", same: true},
		{name: "go-build-info", content: "\x7fELF\x00\n" + buildInfo + "\x00", wantVersion: "v3.2.1"},
		{name: "unversioned", content: "#!/bin/sh\n", wantVersion: "unknown"},
		{name: "missing-sha256sum", content: "#!/bin/sh\n# pulse-hands v1.2.3\n", wantVersion: "v1.2.3", noHash: true},
		{name: "hash-error", content: "#!/bin/sh\n", hashFails: true},
		{name: "absent", absent: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			dir := t.TempDir()
			path := filepath.Join(dir, "hands ' file")
			installedPath := path
			if tc.legacy {
				installedPath += ".sh"
			}
			marker := filepath.Join(dir, "executed")
			content := tc.content + ": > " + shellQuote(marker) + "\n"
			if !tc.absent {
				if err := os.WriteFile(installedPath, []byte(content), 0o755); err != nil {
					t.Fatal(err)
				}
				if !tc.legacy {
					if err := os.WriteFile(path+".sh", []byte("# pulse-hands wrong\n"), 0o755); err != nil {
						t.Fatal(err)
					}
				}
			}
			bin := filepath.Join(dir, "bin")
			if err := os.Mkdir(bin, 0o755); err != nil {
				t.Fatal(err)
			}
			for _, name := range []string{"head", "sed", "grep", "base64", "tr"} {
				tool, err := exec.LookPath(name)
				if err != nil {
					t.Fatal(err)
				}
				if err := os.Symlink(tool, filepath.Join(bin, name)); err != nil {
					t.Fatal(err)
				}
			}
			hash := sha256Bytes([]byte(content))
			if !tc.noHash {
				hasher := "#!/bin/sh\n[ \"$1\" = " + shellQuote(installedPath) + " ] || exit 2\nprintf '%s  %s\\n' " + shellQuote(hash) + " \"$1\"\n"
				if tc.hashFails {
					hasher = "#!/bin/sh\necho 'hash read failed' >&2\nexit 1\n"
				}
				if err := os.WriteFile(filepath.Join(bin, "sha256sum"), []byte(hasher), 0o755); err != nil {
					t.Fatal(err)
				}
			}
			wantState, expectedHash := "differs", ""
			if tc.same {
				wantState, expectedHash = "present", hash
			}
			cmd := exec.Command("sh", "-s")
			cmd.Env = envWith(os.Environ(), map[string]string{"PATH": bin})
			cmd.Stdin = strings.NewReader("set -u\n" + pairInspectHandsScript(path, expectedHash))
			out, err := cmd.CombinedOutput()
			if tc.hashFails {
				if err == nil || !strings.Contains(string(out), "hash read failed") {
					t.Fatalf("hash error not surfaced: %v, %s", err, out)
				}
				return
			}
			if err != nil {
				t.Fatalf("inspect: %v\n%s", err, out)
			}
			insp, _ := pairParseInspection(string(out))
			if tc.absent {
				if len(insp) != 1 || insp["hands"] != "absent" {
					t.Fatalf("absent: %v", insp)
				}
				return
			}
			if tc.noHash {
				hash = "unknown"
			}
			if insp["hands"] != wantState || insp["hands_sha256"] != hash || pairInstalledHandsVersion(insp) != tc.wantVersion {
				t.Fatalf("inspection %v, version %q", insp, pairInstalledHandsVersion(insp))
			}
			if _, err := os.Stat(marker); !os.IsNotExist(err) {
				t.Fatalf("installed artifact was executed: %v", err)
			}
		})
	}
}

func TestPairInspectAlias(t *testing.T) {
	s := newStore(t)
	t.Setenv("HOME", t.TempDir())
	oldSSH, oldBinary := runSSH, handsBinary
	t.Cleanup(func() { runSSH, handsBinary = oldSSH, oldBinary })
	calls := 0
	runSSH = func(_ context.Context, _ []string, stdin []byte, _ time.Duration) (int, string, string, error) {
		calls++
		if !strings.Contains(string(stdin), "hands_buildinfo=") {
			t.Fatal("missing metadata in the read-only inspection script")
		}
		return 0, "uname=Linux\narch=x86_64\nsudo=yes\nhands=differs\ndropin_dir=yes\nremotes=0\nhands_version=v1\n", "", nil
	}
	handsBinary = func(string) ([]byte, string, error) {
		return []byte("#!/bin/sh\n# pulse-hands v2\n"), "fixture", nil
	}
	events := count(t, s, "SELECT COUNT(*) AS n FROM events")
	out, errText, code := invoke(t, s, "pair", "", "admin@example.invalid", "--name", "hands-two", "--inspect")
	if code != 0 || !strings.Contains(out, "hands=differs (installed v1 unknown, template v2 ") || calls != 1 {
		t.Fatalf("inspect alias: code=%d, calls=%d, stderr=%s\n%s", code, calls, errText, out)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM events") != events || executorRow(s, "hands-two") != nil {
		t.Fatal("inspect alias changed the store")
	}
	handsBinary = func(string) ([]byte, string, error) {
		return nil, "", errors.New("template unavailable")
	}
	_, err := pairExecutor(s, "admin@example.invalid", "hands-two", testNow,
		pairOpts{Workdir: "/srv/pulse", Workers: 1, DryRun: true})
	if err == nil || !strings.Contains(err.Error(), "template unavailable") || calls != 2 {
		t.Fatalf("template failure: %v, calls=%d", err, calls)
	}
	_, err = pairExecutor(s, "admin@example.invalid", "hands-two", testNow,
		pairOpts{Workdir: "/srv/pulse", Workers: 1, DryRun: true, Actor: "worker"})
	if err == nil || calls != 2 {
		t.Fatalf("inspection bypassed the agent gate: %v, calls=%d", err, calls)
	}
}

func TestPairHandsGoBinaryMetadata(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "internal", "cli"), 0o755); err != nil {
		t.Fatal(err)
	}
	versionSource, err := os.ReadFile("../cli/version.go")
	if err != nil {
		t.Fatal(err)
	}
	files := map[string][]byte{
		"go.mod":                  []byte("module " + pairHandsModule + "\n\ngo 1.26\n"),
		"internal/cli/version.go": versionSource,
		"main.go": []byte("package main\nimport (\n\"fmt\"\n\"" + pairHandsModule +
			"/internal/cli\"\n)\nfunc main() { fmt.Println(cli.Version()) }\n"),
	}
	for path, data := range files {
		if err := os.WriteFile(filepath.Join(dir, path), data, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	path := filepath.Join(dir, "pulse-hands")
	cmd := exec.Command("go", "build", "-o", path, "-ldflags", "-X "+pairHandsModule+"/internal/cli.version=v8.7.6", ".")
	cmd.Dir = dir
	cmd.Env = envWith(os.Environ(), map[string]string{
		"GOOS": "linux", "GOARCH": "amd64", "CGO_ENABLED": "0", "GOWORK": "off",
		"GOPROXY": "off", "GOSUMDB": "off", "GOTOOLCHAIN": "local", "GOFLAGS": "",
	})
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("build fixture: %v\n%s", err, out)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if got := pairHandsVersion(data); got != "v8.7.6" {
		t.Fatalf("template binary version = %q", got)
	}
	cmd = exec.Command("sh", "-s")
	cmd.Stdin = strings.NewReader("set -u\n" + pairInspectHandsScript(path, ""))
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("inspect Go binary: %v\n%s", err, out)
	}
	insp, _ := pairParseInspection(string(out))
	if got := pairInstalledHandsVersion(insp); got != "v8.7.6" {
		t.Fatalf("installed binary version = %q, metadata = %v", got, insp)
	}
}

func TestPairHandsLegacyOrder(t *testing.T) {
	newer, older := []byte("# pulse-hands new\n"), []byte("# pulse-hands old\n")
	old := pairLegacyHands
	t.Cleanup(func() { pairLegacyHands = old })
	pairLegacyHands = []struct{ revision, hash string }{
		{"new", sha256Bytes(newer)}, {"old", sha256Bytes(older)},
	}
	for _, tc := range []struct {
		installed, template []byte
		want                string
	}{
		{older, newer, "older (installed legacy-old "},
		{newer, older, "differs (installed legacy-new "},
		{newer, newer, "present"},
		{older, nil, "differs (installed legacy-old "},
	} {
		insp := map[string]string{"hands": "differs", "hands_sha256": sha256Bytes(tc.installed)}
		if got := pairHandsSummary(insp, tc.template); !strings.HasPrefix(got, tc.want) {
			t.Errorf("legacy comparison = %q, want prefix %q", got, tc.want)
		}
	}
}
