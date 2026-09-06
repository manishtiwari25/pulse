// Package cli holds what every pulse command shares: the Context a command
// runs in, the command registry and its help groups, aligned tables, colour,
// JSON output and the "what / why / fix" error type.
package cli

import (
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Context is what a command receives.
type Context struct {
	Store *store.Store // nil for NoStore commands
	Home  string       // the resolved PULSE home
	Out   io.Writer
	Err   io.Writer
	JSON  bool
	Quiet bool
	Color bool

	Agent   string // --agent / $PULSE_AGENT
	Project string // --project / $PULSE_PROJECT
	Session string // --session / $PULSE_SESSION
	Now     string // --now / $PULSE_NOW ("" = wall clock)

	Args []string // positional arguments after the command and its flags
}

// Style returns the colour style for this context's output.
func (c *Context) Style() Style { return Style{On: c.Color} }

// Printf writes to Out unless --quiet.
func (c *Context) Printf(format string, a ...any) {
	if c.Quiet {
		return
	}
	fmt.Fprintf(c.Out, format, a...)
}

// Println writes a line to Out unless --quiet.
func (c *Context) Println(a ...any) {
	if c.Quiet {
		return
	}
	fmt.Fprintln(c.Out, a...)
}

// Command is one registered pulse command.
type Command struct {
	Name    string
	Group   string // one of Groups
	Summary string // one line for `pulse help`
	Usage   string // "pulse post <kind> --summary S ..." for `pulse help <name>`
	Aliases []string

	NoStore  bool // do not open the store (help, version, schema, commands that open it themselves)
	ReadOnly bool // open the store read-only (no EnsureRecords)
	Raw      bool // pass the arguments verbatim, without flag parsing (subcommand dispatchers)

	Run   func(c *Context, fs *flag.FlagSet, args []string) error
	Flags func(fs *flag.FlagSet) // optional: define the command's flags
}

// Groups lists the help groups in display order.
var Groups = []string{"agents", "records", "human", "runner", "hub", "inspect"}

// GroupTitles are the headings printed by `pulse help`.
var GroupTitles = map[string]string{
	"agents":  "Agents - the board",
	"records": "Records - decisions, plans, guides and memory in the store",
	"human":   "Human - the maintainer's calls",
	"runner":  "Runner - jobs, workers and landing",
	"hub":     "Hub - the web view",
	"inspect": "Inspect - read the store",
}

var (
	commands = map[string]Command{}
	aliases  = map[string]string{}
)

// Register adds a command; it panics on a duplicate name or alias so a
// collision between agents' packages fails at start-up, not at run time.
func Register(cmd Command) {
	if cmd.Name == "" {
		panic("cli.Register: command without a name")
	}
	if cmd.Run == nil {
		panic("cli.Register: command " + cmd.Name + " without Run")
	}
	if !validGroup(cmd.Group) {
		panic("cli.Register: command " + cmd.Name + " has unknown group " + cmd.Group)
	}
	if _, dup := commands[cmd.Name]; dup {
		panic("cli.Register: duplicate command " + cmd.Name)
	}
	if target, dup := aliases[cmd.Name]; dup {
		panic("cli.Register: command " + cmd.Name + " collides with an alias of " + target)
	}
	for _, a := range cmd.Aliases {
		if _, dup := commands[a]; dup {
			panic("cli.Register: alias " + a + " of " + cmd.Name + " collides with a command")
		}
		if target, dup := aliases[a]; dup {
			panic("cli.Register: alias " + a + " of " + cmd.Name + " collides with an alias of " + target)
		}
	}
	commands[cmd.Name] = cmd
	for _, a := range cmd.Aliases {
		aliases[a] = cmd.Name
	}
}

func validGroup(g string) bool {
	for _, x := range Groups {
		if x == g {
			return true
		}
	}
	return false
}

// Lookup finds a command by name or alias.
func Lookup(name string) (Command, bool) {
	if cmd, ok := commands[name]; ok {
		return cmd, true
	}
	if target, ok := aliases[name]; ok {
		return commands[target], true
	}
	return Command{}, false
}

// All returns every command sorted by group (display order) then name.
func All() []Command {
	out := make([]Command, 0, len(commands))
	for _, cmd := range commands {
		out = append(out, cmd)
	}
	sort.Slice(out, func(i, j int) bool {
		gi, gj := groupIndex(out[i].Group), groupIndex(out[j].Group)
		if gi != gj {
			return gi < gj
		}
		return out[i].Name < out[j].Name
	})
	return out
}

// Names returns every command name and alias, for suggestions.
func Names() []string {
	out := make([]string, 0, len(commands)+len(aliases))
	for n := range commands {
		out = append(out, n)
	}
	for a := range aliases {
		out = append(out, a)
	}
	sort.Strings(out)
	return out
}

func groupIndex(g string) int {
	for i, x := range Groups {
		if x == g {
			return i
		}
	}
	return len(Groups)
}

// WriteJSON writes v as indented JSON with a trailing newline.
func WriteJSON(w io.Writer, v any) error {
	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	enc.SetEscapeHTML(false)
	return enc.Encode(v)
}

// Error is a command failure told as what failed, why, and the one command
// that fixes it. Code is the exit status (default 1). An Error with an empty
// What is silent: the process just exits with Code.
type Error struct {
	What string
	Why  string
	Fix  string
	Code int
}

func (e *Error) Error() string {
	var b strings.Builder
	b.WriteString(e.What)
	if e.Why != "" {
		b.WriteString("\n  why: " + e.Why)
	}
	if e.Fix != "" {
		b.WriteString("\n  fix: " + e.Fix)
	}
	return b.String()
}

// ExitCode is Code, or 1 when unset.
func (e *Error) ExitCode() int {
	if e.Code == 0 {
		return 1
	}
	return e.Code
}

// Errorf builds an Error.
func Errorf(code int, what, why, fix string) *Error {
	return &Error{What: what, Why: why, Fix: fix, Code: code}
}

// IsTTY reports whether f is a terminal.
func IsTTY(f *os.File) bool {
	if f == nil {
		return false
	}
	info, err := f.Stat()
	if err != nil {
		return false
	}
	return info.Mode()&os.ModeCharDevice != 0
}

// ColorEnabled applies the colour rule: on only when out is a TTY, NO_COLOR is
// unset and --no-color was not given.
func ColorEnabled(out io.Writer, noColor bool) bool {
	if noColor {
		return false
	}
	if _, set := os.LookupEnv("NO_COLOR"); set {
		return false
	}
	f, ok := out.(*os.File)
	return ok && IsTTY(f)
}
