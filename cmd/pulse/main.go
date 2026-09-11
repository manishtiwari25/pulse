// Command pulse is the PULSE store, board, records and runner in one binary.
//
//	pulse [global flags] <command> [command flags] [args]
package main

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"

	_ "github.com/manishtiwari25/pulse/internal/board"
	_ "github.com/manishtiwari25/pulse/internal/evolve"
	_ "github.com/manishtiwari25/pulse/internal/harness"
	_ "github.com/manishtiwari25/pulse/internal/hooks"
	_ "github.com/manishtiwari25/pulse/internal/hub"
	_ "github.com/manishtiwari25/pulse/internal/inspect"
	_ "github.com/manishtiwari25/pulse/internal/provider/copilot"
	_ "github.com/manishtiwari25/pulse/internal/records"
	_ "github.com/manishtiwari25/pulse/internal/runner"
	_ "github.com/manishtiwari25/pulse/internal/tally"
	_ "github.com/manishtiwari25/pulse/internal/tasks"
)

func init() {
	cli.Register(cli.Command{
		Name:    "version",
		Group:   "inspect",
		Summary: "print the pulse version",
		Usage:   "pulse version [--json]",
		NoStore: true,
		Run: func(c *cli.Context, _ *flag.FlagSet, _ []string) error {
			if c.JSON {
				return cli.WriteJSON(c.Out, map[string]string{"version": cli.Version()})
			}
			fmt.Fprintln(c.Out, cli.Version())
			return nil
		},
	})
}

type globals struct {
	home, project, agent, session, now string
	json, quiet, noColor               bool
}

func (g *globals) bind(fs *flag.FlagSet) {
	fs.StringVar(&g.home, "home", os.Getenv("PULSE_HOME"), "PULSE home (default $PULSE_HOME or ~/.pulse)")
	fs.StringVar(&g.project, "project", os.Getenv("PULSE_PROJECT"), "project slug (default $PULSE_PROJECT, else the checkout you are in)")
	fs.StringVar(&g.agent, "agent", os.Getenv("PULSE_AGENT"), "the agent you act as (default $PULSE_AGENT)")
	fs.StringVar(&g.session, "session", os.Getenv("PULSE_SESSION"), "the session you act in (default $PULSE_SESSION)")
	fs.StringVar(&g.now, "now", os.Getenv("PULSE_NOW"), "fixed UTC clock like 2026-09-06T10:00:00Z (tests, parity)")
	fs.BoolVar(&g.json, "json", false, "machine-readable output")
	fs.BoolVar(&g.quiet, "quiet", false, "print only what a script needs")
	fs.BoolVar(&g.noColor, "no-color", false, "no colour even on a terminal")
}

func main() {
	os.Exit(run(os.Args[1:], os.Stdout, os.Stderr))
}

func run(argv []string, stdout, stderr io.Writer) int {
	g := &globals{}
	gfs := flag.NewFlagSet("pulse", flag.ContinueOnError)
	gfs.SetOutput(io.Discard)
	g.bind(gfs)
	if err := gfs.Parse(argv); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			cli.Help(stdout, cli.ColorEnabled(stdout, g.noColor))
			return 0
		}
		fmt.Fprintf(stderr, "pulse: %v\n  fix: pulse help\n", err)
		return 2
	}
	color := cli.ColorEnabled(stdout, g.noColor)
	rest := gfs.Args()
	if len(rest) == 0 {
		cli.Help(stdout, color)
		return 0
	}
	name, rest := rest[0], rest[1:]
	switch name {
	case "help", "-h", "--help":
		return help(rest, stdout, stderr, color)
	}
	cmd, ok := cli.Lookup(name)
	if !ok {
		return unknown(name, stderr)
	}

	home, err := store.ResolveHome(g.home)
	if err != nil {
		return report(name, err, stderr)
	}
	ctx := &cli.Context{
		Home: home, Out: stdout, Err: stderr,
		JSON: g.json, Quiet: g.quiet, Color: color,
		Agent: g.agent, Project: g.project, Session: g.session, Now: g.now,
	}

	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	if cmd.Flags != nil {
		cmd.Flags(fs)
	}
	var args []string
	if cmd.Raw {
		args = rest
	} else {
		// the global flags are accepted after the command word too, so the
		// Python spellings (`pulse post --project X ...`) keep working
		late := &globals{}
		lateBind(fs, late)
		args, err = parseInterleaved(fs, rest)
		if err != nil {
			if errors.Is(err, flag.ErrHelp) {
				cli.HelpCommand(stdout, color, cmd)
				return 0
			}
			fmt.Fprintf(stderr, "pulse %s: %v\n  fix: pulse help %s\n", name, err, name)
			return 2
		}
		applyLate(fs, late, ctx)
	}
	ctx.Args = args

	if !cmd.NoStore {
		st, err := store.Open(store.Options{
			Home:        g.home,
			Create:      name == "init",
			ReadOnly:    cmd.ReadOnly,
			SkipRecords: cmd.ReadOnly,
			Now:         ctx.Now,
		})
		if err != nil {
			return report(name, err, stderr)
		}
		defer st.Close()
		ctx.Store = st
	}
	if err := cmd.Run(ctx, fs, args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			cli.HelpCommand(stdout, color, cmd)
			return 0
		}
		return report(name, err, stderr)
	}
	return 0
}

// lateBind defines the global flags on a command's FlagSet unless the command
// defined a flag of that name itself.
func lateBind(fs *flag.FlagSet, late *globals) {
	def := func(name string, f func()) {
		if fs.Lookup(name) == nil {
			f()
		}
	}
	def("project", func() { fs.StringVar(&late.project, "project", "", "project slug") })
	def("agent", func() { fs.StringVar(&late.agent, "agent", "", "the agent you act as") })
	def("session", func() { fs.StringVar(&late.session, "session", "", "the session you act in") })
	def("now", func() { fs.StringVar(&late.now, "now", "", "fixed UTC clock") })
	def("json", func() { fs.BoolVar(&late.json, "json", false, "machine-readable output") })
	def("quiet", func() { fs.BoolVar(&late.quiet, "quiet", false, "print only what a script needs") })
	def("no-color", func() { fs.BoolVar(&late.noColor, "no-color", false, "no colour") })
}

func applyLate(fs *flag.FlagSet, late *globals, ctx *cli.Context) {
	set := map[string]bool{}
	fs.Visit(func(f *flag.Flag) { set[f.Name] = true })
	if set["project"] && late.project != "" {
		ctx.Project = late.project
	}
	if set["agent"] && late.agent != "" {
		ctx.Agent = late.agent
	}
	if set["session"] && late.session != "" {
		ctx.Session = late.session
	}
	if set["now"] && late.now != "" {
		ctx.Now = late.now
	}
	if late.json {
		ctx.JSON = true
	}
	if late.quiet {
		ctx.Quiet = true
	}
	if late.noColor {
		ctx.Color = false
	}
}

// parseInterleaved parses flags that appear before, between or after the
// positional arguments (argparse style) and returns the positionals.
func parseInterleaved(fs *flag.FlagSet, args []string) ([]string, error) {
	var positional []string
	for len(args) > 0 {
		a := args[0]
		if a == "--" {
			return append(positional, args[1:]...), nil
		}
		if !strings.HasPrefix(a, "-") || a == "-" {
			positional = append(positional, a)
			args = args[1:]
			continue
		}
		if err := fs.Parse(args); err != nil {
			return nil, err
		}
		rest := fs.Args()
		consumed := args[:len(args)-len(rest)]
		if len(consumed) > 0 && consumed[len(consumed)-1] == "--" {
			return append(positional, rest...), nil
		}
		args = rest
	}
	return positional, nil
}

func help(args []string, stdout, stderr io.Writer, color bool) int {
	if len(args) == 0 {
		cli.Help(stdout, color)
		return 0
	}
	cmd, ok := cli.Lookup(args[0])
	if !ok {
		return unknown(args[0], stderr)
	}
	cli.HelpCommand(stdout, color, cmd)
	return 0
}

func unknown(name string, stderr io.Writer) int {
	fmt.Fprintf(stderr, "pulse: unknown command %q\n", name)
	if s := cli.Suggest(name, 3); len(s) > 0 {
		fmt.Fprintf(stderr, "  did you mean: %s\n", strings.Join(s, ", "))
	}
	fmt.Fprintln(stderr, "  fix: pulse help")
	return 2
}

// report prints an error the "what / why / fix" way and returns the exit code.
func report(name string, err error, stderr io.Writer) int {
	var ce *cli.Error
	var nf *store.NotFoundError
	var in *store.InputError
	switch {
	case errors.As(err, &ce):
		if ce.What != "" {
			fmt.Fprintln(stderr, ce.Error())
		}
		return ce.ExitCode()
	case errors.As(err, &nf):
		fmt.Fprintln(stderr, cli.Errorf(2, "No store at "+nf.Path, "this machine has no PULSE store yet", "pulse init <project-path>").Error())
		return 2
	case errors.As(err, &in):
		fmt.Fprintf(stderr, "pulse %s: %s\n", name, in.Msg)
		return 2
	}
	fmt.Fprintf(stderr, "pulse %s: %v\n", name, err)
	return 1
}
