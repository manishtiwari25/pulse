package main

import (
	"bytes"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestHelpAndVersion(t *testing.T) {
	var out, errb bytes.Buffer
	if code := run([]string{"help"}, &out, &errb); code != 0 || !strings.HasPrefix(out.String(), cli.Title) {
		t.Fatalf("help: %d %q", code, out.String())
	}
	out.Reset()
	if code := run(nil, &out, &errb); code != 0 || !strings.Contains(out.String(), "Runner - jobs, workers and landing") {
		t.Fatalf("bare: %d %q", code, out.String())
	}
	out.Reset()
	if code := run([]string{"-h"}, &out, &errb); code != 0 || !strings.HasPrefix(out.String(), cli.Title) {
		t.Fatalf("-h: %d", code)
	}
	out.Reset()
	if code := run([]string{"help", "work"}, &out, &errb); code != 0 || !strings.Contains(out.String(), "Usage: pulse work") {
		t.Fatalf("help work: %d %q", code, out.String())
	}
	out.Reset()
	if code := run([]string{"version"}, &out, &errb); code != 0 || strings.TrimSpace(out.String()) == "" {
		t.Fatalf("version: %d %q", code, out.String())
	}
	out.Reset()
	if code := run([]string{"--json", "version"}, &out, &errb); code != 0 || !strings.Contains(out.String(), `"version":`) {
		t.Fatalf("version --json: %d %q", code, out.String())
	}
}

func TestUnknownCommand(t *testing.T) {
	var out, errb bytes.Buffer
	code := run([]string{"--home", t.TempDir(), "statuz"}, &out, &errb)
	if code != 2 || !strings.Contains(errb.String(), `unknown command "statuz"`) || !strings.Contains(errb.String(), "did you mean:") {
		t.Fatalf("code %d stderr %q", code, errb.String())
	}
}

func TestNoStoreMapping(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := t.TempDir()
	cli.Register(cli.Command{Name: "zz-needs-store", Group: "inspect", Summary: "test", Run: func(c *cli.Context, _ *flag.FlagSet, _ []string) error {
		return nil
	}})
	var out, errb bytes.Buffer
	code := run([]string{"--home", home, "zz-needs-store"}, &out, &errb)
	want, _ := store.StorePath(home)
	if code != 2 || !strings.Contains(errb.String(), "No store at "+want) || !strings.Contains(errb.String(), "fix: pulse init <project-path>") {
		t.Fatalf("code %d stderr %q", code, errb.String())
	}
	if _, err := os.Stat(want); err == nil {
		t.Fatal("store created by a non-init command")
	}
}

func TestInterleavedFlagsAndErrors(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	t.Setenv("NO_COLOR", "1")
	home := t.TempDir()
	if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
		t.Fatal(err)
	}
	var seen *cli.Context
	var seenArgs []string
	var seenKind string
	cli.Register(cli.Command{Name: "zz-post", Group: "agents", Summary: "test", Run: func(c *cli.Context, fs *flag.FlagSet, args []string) error {
		seen, seenArgs = c, args
		seenKind = fs.Lookup("kind").Value.String()
		if len(args) > 0 && args[0] == "fail" {
			return cli.Errorf(3, "boom", "because", "pulse zz-post ok")
		}
		if len(args) > 0 && args[0] == "input" {
			return c.Store.SetSetting("human", "quorum", "x")
		}
		return nil
	}, Flags: func(fs *flag.FlagSet) { fs.String("kind", "", "kind") }})

	var out, errb bytes.Buffer
	code := run([]string{"--home", home, "--agent", "early", "zz-post", "one", "--kind", "finding", "two", "--project", "pulse", "--json", "--now", "2026-09-06T10:00:00Z", "--", "-three"}, &out, &errb)
	if code != 0 {
		t.Fatalf("code %d stderr %q", code, errb.String())
	}
	if strings.Join(seenArgs, ",") != "one,two,-three" || seenKind != "finding" {
		t.Fatalf("args %v kind %q", seenArgs, seenKind)
	}
	if seen.Agent != "early" || seen.Project != "pulse" || !seen.JSON || seen.Now != "2026-09-06T10:00:00Z" || seen.Store == nil || seen.Store.Now() != "2026-09-06T10:00:00Z" {
		t.Fatalf("context %+v", seen)
	}
	if !strings.HasSuffix(seen.Home, filepath.Base(home)) {
		t.Fatalf("home %q", seen.Home)
	}

	errb.Reset()
	if code := run([]string{"--home", home, "zz-post", "fail"}, &out, &errb); code != 3 || errb.String() != "boom\n  why: because\n  fix: pulse zz-post ok\n" {
		t.Fatalf("cli.Error: %d %q", code, errb.String())
	}
	errb.Reset()
	if code := run([]string{"--home", home, "zz-post", "input"}, &out, &errb); code != 2 || errb.String() != "pulse zz-post: quorum takes a whole number, not 'x'\n" {
		t.Fatalf("InputError: %d %q", code, errb.String())
	}
	errb.Reset()
	if code := run([]string{"--home", home, "zz-post", "--bogus"}, &out, &errb); code != 2 || !strings.Contains(errb.String(), "flag provided but not defined: -bogus") {
		t.Fatalf("bad flag: %d %q", code, errb.String())
	}
	out.Reset()
	if code := run([]string{"--home", home, "zz-post", "-h"}, &out, &errb); code != 0 || !strings.Contains(out.String(), "-kind string") {
		t.Fatalf("command -h: %d %q", code, out.String())
	}
}
