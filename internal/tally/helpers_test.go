package tally

import (
	"bytes"
	"flag"
	"io"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// runCLI runs a registered command in-process the way cmd/pulse does: global
// flags (--project, --agent, --session, --now, --json) are picked out of args,
// the command's own flags are parsed, the rest are positionals.
func runCLI(t *testing.T, st *store.Store, name string, args ...string) (string, error) {
	t.Helper()
	cmd, ok := cli.Lookup(name)
	if !ok {
		t.Fatalf("no command %q", name)
	}
	var out bytes.Buffer
	c := &cli.Context{Store: st, Home: st.Home, Out: &out, Err: io.Discard}
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	if cmd.Flags != nil {
		cmd.Flags(fs)
	}
	def := func(n string, p *string) {
		if fs.Lookup(n) == nil {
			fs.StringVar(p, n, "", "")
		}
	}
	def("project", &c.Project)
	def("agent", &c.Agent)
	def("session", &c.Session)
	def("now", &c.Now)
	if fs.Lookup("json") == nil {
		fs.BoolVar(&c.JSON, "json", false, "")
	}
	var positional []string
	rest := args
	for len(rest) > 0 {
		if !strings.HasPrefix(rest[0], "-") {
			positional = append(positional, rest[0])
			rest = rest[1:]
			continue
		}
		if err := fs.Parse(rest); err != nil {
			return "", err
		}
		if len(fs.Args()) == len(rest) { // "-" alone is a positional
			positional = append(positional, rest[0])
			rest = rest[1:]
			continue
		}
		rest = fs.Args()
	}
	c.Args = positional
	err := cmd.Run(c, fs, positional)
	return out.String(), err
}

// openSnapshot opens a read-only store at home (a snapshot or a fixture).
func openReadOnly(t *testing.T, home string) *store.Store {
	t.Helper()
	st, err := store.Open(store.Options{Home: home, ReadOnly: true, SkipRecords: true})
	if err != nil {
		t.Fatalf("open %s: %v", home, err)
	}
	t.Cleanup(func() { st.Close() })
	return st
}
