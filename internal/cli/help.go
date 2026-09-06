package cli

import (
	"flag"
	"fmt"
	"io"
	"sort"
	"strings"
)

// Title is the first line of `pulse help`.
const Title = "pulse - the PULSE store, board, records and runner in one command"

// GlobalUsage is the usage line printed by `pulse help`.
const GlobalUsage = "usage: pulse [--home DIR] [--project SLUG] [--agent NAME] [--session ID] [--json] [--quiet] [--no-color] [--now TIME] <command> [flags] [args]"

// Help prints the grouped command list.
func Help(w io.Writer, color bool) {
	style := Style{On: color}
	fmt.Fprintln(w, style.Bold(Title))
	fmt.Fprintln(w)
	fmt.Fprintln(w, GlobalUsage)
	fmt.Fprintln(w, "       pulse help [command]     flags and usage of one command")
	byGroup := map[string][]Command{}
	for _, cmd := range All() {
		byGroup[cmd.Group] = append(byGroup[cmd.Group], cmd)
	}
	for _, g := range Groups {
		cmds := byGroup[g]
		if len(cmds) == 0 {
			continue
		}
		fmt.Fprintln(w)
		fmt.Fprintln(w, style.Cyan(GroupTitles[g]))
		rows := make([][]string, 0, len(cmds))
		for _, cmd := range cmds {
			name := cmd.Name
			if len(cmd.Aliases) > 0 {
				name += " (" + strings.Join(cmd.Aliases, ", ") + ")"
			}
			rows = append(rows, []string{"  " + name, cmd.Summary})
		}
		Table(w, false, nil, rows)
	}
	fmt.Fprintln(w)
	fmt.Fprintln(w, "Global flags default to $PULSE_HOME, $PULSE_PROJECT, $PULSE_AGENT, $PULSE_SESSION and $PULSE_NOW.")
}

// HelpCommand prints the usage and flags of one command.
func HelpCommand(w io.Writer, color bool, cmd Command) {
	style := Style{On: color}
	fmt.Fprintf(w, "%s  %s\n", style.Bold("pulse "+cmd.Name), cmd.Summary)
	if len(cmd.Aliases) > 0 {
		fmt.Fprintf(w, "aliases: %s\n", strings.Join(cmd.Aliases, ", "))
	}
	fmt.Fprintln(w)
	usage := cmd.Usage
	if usage == "" {
		usage = "pulse " + cmd.Name
	}
	fmt.Fprintln(w, "Usage: "+usage)
	if cmd.Flags == nil {
		return
	}
	fs := flag.NewFlagSet(cmd.Name, flag.ContinueOnError)
	cmd.Flags(fs)
	n := 0
	fs.VisitAll(func(*flag.Flag) { n++ })
	if n == 0 {
		return
	}
	fmt.Fprintln(w)
	fmt.Fprintln(w, "Flags:")
	fs.SetOutput(w)
	fs.PrintDefaults()
}

// Suggest returns up to n registered names closest to name by edit distance.
func Suggest(name string, n int) []string {
	type cand struct {
		name string
		d    int
	}
	var cands []cand
	for _, other := range Names() {
		cands = append(cands, cand{other, editDistance(name, other)})
	}
	sort.Slice(cands, func(i, j int) bool {
		if cands[i].d != cands[j].d {
			return cands[i].d < cands[j].d
		}
		return cands[i].name < cands[j].name
	})
	var out []string
	for _, c := range cands {
		if len(out) == n {
			break
		}
		out = append(out, c.name)
	}
	return out
}

func editDistance(a, b string) int {
	ra, rb := []rune(a), []rune(b)
	prev := make([]int, len(rb)+1)
	cur := make([]int, len(rb)+1)
	for j := range prev {
		prev[j] = j
	}
	for i := 1; i <= len(ra); i++ {
		cur[0] = i
		for j := 1; j <= len(rb); j++ {
			cost := 1
			if ra[i-1] == rb[j-1] {
				cost = 0
			}
			cur[j] = min(prev[j]+1, cur[j-1]+1, prev[j-1]+cost)
		}
		prev, cur = cur, prev
	}
	return prev[len(rb)]
}
