package tasks

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func init() {
	cli.Register(cli.Command{
		Name:    "task",
		Group:   "agents",
		Summary: "the owner's tasks for a project: new, list, show, amend, cancel, status, reply",
		Usage: "pulse task new --project P --title T [--brief-file F | --brief TEXT | stdin] [--priority N]\n" +
			"pulse task list [--project P] [--status S] [--json]\n" +
			"pulse task show TASK-NNN [--json]\n" +
			"pulse task amend TASK-NNN [--title T] [--brief-file F | --brief TEXT | stdin] [--note WHY]\n" +
			"pulse task cancel TASK-NNN [--reason WHY]\n" +
			"pulse task status TASK-NNN <open|planned|in_progress|review|done|cancelled>\n" +
			"pulse task reply TASK-NNN <text> | --file F",
		Flags: func(fs *flag.FlagSet) {
			fs.String("title", "", "task title (new, amend)")
			fs.String("brief", "", "brief text (new, amend); or --brief-file, or stdin")
			fs.String("brief-file", "", "file holding the brief; - for stdin")
			fs.Int("priority", 0, "priority, higher first (new)")
			fs.String("status", "", "filter by status (list)")
			fs.String("note", "", "why this revision (amend)")
			fs.String("reason", "", "why (cancel)")
			fs.String("file", "", "file holding the reply text (reply)")
		},
		Run: runTask,
	})
	cli.Register(cli.Command{
		Name:    "msg",
		Group:   "agents",
		Summary: "the conversation between the owner and a project's lead: send, list, read",
		Usage: "pulse msg send --project P [--task TASK-NNN] <text> | --file F\n" +
			"pulse msg list [--project P] [--task TASK-NNN] [--since MSG-ID] [--unread] [--json]\n" +
			"pulse msg read MSG-ID... (mark read for --agent)",
		Flags: func(fs *flag.FlagSet) {
			fs.String("task", "", "the task the message is about")
			fs.String("since", "", "only messages after this id (list)")
			fs.Bool("unread", false, "only messages the acting agent has not read (list)")
			fs.String("file", "", "file holding the message text (send)")
		},
		Run: runMsg,
	})
	cli.Register(cli.Command{
		Name:    "compute",
		Group:   "agents",
		Summary: "more machines for agents: request (agents), list, approve|deny (the owner)",
		Usage: "pulse compute request --project P [--size S] --hours N --reason WHY\n" +
			"pulse compute list [--project P] [--status S] [--json]\n" +
			"pulse compute approve CR-NNN [--words WHY]   (owner)\n" +
			"pulse compute deny CR-NNN [--words WHY]      (owner)\n" +
			"pulse compute providers [--json]             (the providers this binary can provision with)",
		Flags: func(fs *flag.FlagSet) {
			fs.String("size", "", "machine size (default: the provider's size setting, e.g. compute.azure.vm_size)")
			fs.Int("hours", 0, "hours the machine is needed")
			fs.String("reason", "", "why the work needs it")
			fs.String("status", "", "filter by status (list)")
			fs.String("words", "", "the owner's words (approve, deny)")
		},
		Run: runCompute,
	})
	cli.Register(cli.Command{
		Name:     "watch",
		Group:    "agents",
		Summary:  "stream store events as they happen (Ctrl-C exits 0)",
		Usage:    "pulse watch [--project P] [--area A] [--thread T] [--action GLOB] [--since ID] [--detail] [--interval 1s] [--json]",
		ReadOnly: true,
		Flags: func(fs *flag.FlagSet) {
			fs.String("area", "", "only posts in this area")
			fs.String("thread", "", "only posts in this thread (area/thread or thread)")
			fs.String("action", "", "only actions matching this glob, e.g. task.* (repeatable with commas)")
			fs.Int64("since", -1, "start after this event id (default: now)")
			fs.Bool("detail", false, "append the detail column")
			fs.Duration("interval", time.Second, "poll interval")
		},
		Run: runWatch,
	})
}

func actor(c *cli.Context) string {
	if c.Agent != "" {
		return c.Agent
	}
	return store.HumanAgent
}

func str(fs *flag.FlagSet, name string) string { return fs.Lookup(name).Value.String() }
func num(fs *flag.FlagSet, name string) int {
	n, _ := strconv.Atoi(fs.Lookup(name).Value.String())
	return n
}
func boolean(fs *flag.FlagSet, name string) bool { return fs.Lookup(name).Value.String() == "true" }

// stdin is what the commands read a brief from; tests replace it.
var stdin io.Reader = os.Stdin

// readBody returns text from --brief/--file style flags, a file, or stdin when
// it is not a terminal. Empty when nothing was given.
func readBody(inline, file string) (string, error) {
	if inline != "" {
		return inline, nil
	}
	if file != "" && file != "-" {
		b, err := os.ReadFile(file)
		if err != nil {
			return "", cli.Errorf(2, "Cannot read "+file, err.Error(), "check the path")
		}
		return string(b), nil
	}
	if file == "-" || (stdin != os.Stdin) || !cli.IsTTY(os.Stdin) {
		b, err := io.ReadAll(stdin)
		if err != nil {
			return "", err
		}
		return string(b), nil
	}
	return "", nil
}

func usage(c *cli.Context, cmd, sub string) error {
	what := fmt.Sprintf("pulse %s: unknown subcommand %q", cmd, sub)
	if sub == "" {
		what = fmt.Sprintf("pulse %s needs a subcommand", cmd)
	}
	return cli.Errorf(2, what, "see the usage", "pulse help "+cmd)
}

func need(args []string, n int, fix string) error {
	if len(args) < n {
		return cli.Errorf(2, "Missing argument", "the command needs "+fix, "pulse help task")
	}
	return nil
}

func printTask(c *cli.Context, t *Task) {
	st := c.Style()
	c.Printf("%s  %s  %s\n", st.Bold(t.ID), st.Cyan(t.Status), t.Title)
	c.Printf("  project %s  priority %d  rev %d  by %s  created %s  updated %s\n", t.Project, t.Priority, t.CurrentRev, t.CreatedBy, t.CreatedAt, t.UpdatedAt)
	if t.Lead != "" || t.JobID != "" {
		c.Printf("  lead %s  job %s\n", t.Lead, t.JobID)
	}
}

func runTask(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return usage(c, "task", "")
	}
	sub, rest := args[0], args[1:]
	switch sub {
	case "new":
		brief, err := readBody(str(fs, "brief"), str(fs, "brief-file"))
		if err != nil {
			return err
		}
		t, err := New(c.Store, c.Project, str(fs, "title"), brief, num(fs, "priority"), actor(c))
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"task": t})
		}
		c.Printf("%s created in %s: %s\n", t.ID, t.Project, t.Title)
		return nil
	case "list":
		list, err := List(c.Store, c.Project, str(fs, "status"))
		if err != nil {
			return err
		}
		if c.JSON {
			if list == nil {
				list = []Task{}
			}
			return cli.WriteJSON(c.Out, map[string]any{"tasks": list})
		}
		if len(list) == 0 {
			c.Println("No tasks.")
			return nil
		}
		rows := make([][]string, 0, len(list))
		for _, t := range list {
			rows = append(rows, []string{t.ID, t.Project, t.Status, strconv.Itoa(t.Priority), t.Lead, t.UpdatedAt, t.Title})
		}
		cli.Table(c.Out, c.Color, []string{"ID", "PROJECT", "STATUS", "PRI", "LEAD", "UPDATED", "TITLE"}, rows)
		return nil
	case "show":
		if err := need(rest, 1, "a task id"); err != nil {
			return err
		}
		t, revs, err := Get(c.Store, rest[0])
		if err != nil {
			return err
		}
		msgs, err := Messages(c.Store, "", t.ID, "", "")
		if err != nil {
			return err
		}
		jobs, err := Jobs(c.Store, t.ID)
		if err != nil {
			return err
		}
		if msgs == nil {
			msgs = []Message{}
		}
		if jobs == nil {
			jobs = []Job{}
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"task": t, "revisions": revs, "messages": msgs, "jobs": jobs})
		}
		printTask(c, t)
		c.Println()
		c.Println(strings.TrimRight(t.Brief, "\n"))
		c.Println()
		rows := [][]string{}
		for _, r := range revs {
			rows = append(rows, []string{strconv.Itoa(r.Rev), r.At, r.Author, r.Note, r.Title})
		}
		c.Println(c.Style().Bold("REVISIONS"))
		cli.Table(c.Out, c.Color, []string{"REV", "AT", "AUTHOR", "NOTE", "TITLE"}, rows)
		c.Println()
		c.Println(c.Style().Bold("MESSAGES"))
		if len(msgs) == 0 {
			c.Println("  none")
		}
		for _, m := range msgs {
			c.Printf("  %s %s %s: %s\n", m.At, m.ID, m.Author, m.Body)
		}
		c.Println()
		c.Println(c.Style().Bold("JOBS"))
		if len(jobs) == 0 {
			c.Println("  none")
		} else {
			rows = rows[:0]
			for _, j := range jobs {
				rows = append(rows, []string{j.ID, j.Kind, j.Status, j.Agent, j.Branch, j.Title})
			}
			cli.Table(c.Out, c.Color, []string{"ID", "KIND", "STATUS", "AGENT", "BRANCH", "TITLE"}, rows)
		}
		return nil
	case "amend":
		if err := need(rest, 1, "a task id"); err != nil {
			return err
		}
		brief, err := readBody(str(fs, "brief"), str(fs, "brief-file"))
		if err != nil {
			return err
		}
		t, err := Amend(c.Store, rest[0], str(fs, "title"), brief, str(fs, "note"), actor(c))
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"task": t})
		}
		c.Printf("%s amended: rev %d\n", t.ID, t.CurrentRev)
		return nil
	case "cancel":
		if err := need(rest, 1, "a task id"); err != nil {
			return err
		}
		t, err := Cancel(c.Store, rest[0], str(fs, "reason"), actor(c))
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"task": t})
		}
		c.Printf("%s cancelled\n", t.ID)
		return nil
	case "status":
		if err := need(rest, 2, "a task id and a status"); err != nil {
			return err
		}
		t, err := SetStatus(c.Store, rest[0], rest[1], actor(c))
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"task": t})
		}
		c.Printf("%s is %s\n", t.ID, t.Status)
		return nil
	case "reply":
		if err := need(rest, 1, "a task id"); err != nil {
			return err
		}
		body, err := readBody(strings.Join(rest[1:], " "), str(fs, "file"))
		if err != nil {
			return err
		}
		m, err := Reply(c.Store, rest[0], actor(c), body)
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"message": m})
		}
		c.Printf("%s sent on %s\n", m.ID, m.TaskID)
		return nil
	}
	return usage(c, "task", sub)
}

func runMsg(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return usage(c, "msg", "")
	}
	sub, rest := args[0], args[1:]
	switch sub {
	case "send":
		body, err := readBody(strings.Join(rest, " "), str(fs, "file"))
		if err != nil {
			return err
		}
		m, err := SendMessage(c.Store, c.Project, str(fs, "task"), actor(c), body)
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"message": m})
		}
		c.Printf("%s sent in %s\n", m.ID, m.Project)
		return nil
	case "list":
		unread := ""
		if boolean(fs, "unread") {
			unread = actor(c)
		}
		list, err := Messages(c.Store, c.Project, str(fs, "task"), str(fs, "since"), unread)
		if err != nil {
			return err
		}
		if list == nil {
			list = []Message{}
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"messages": list})
		}
		if len(list) == 0 {
			c.Println("No messages.")
			return nil
		}
		for _, m := range list {
			task := m.TaskID
			if task == "" {
				task = "-"
			}
			c.Printf("%s %s %s %s %s: %s\n", m.At, m.ID, m.Project, task, c.Style().Bold(m.Author), m.Body)
		}
		return nil
	case "read":
		if err := need(rest, 1, "one or more message ids"); err != nil {
			return err
		}
		n, err := MarkRead(c.Store, rest, actor(c))
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"marked": n})
		}
		c.Printf("%d marked read for %s\n", n, actor(c))
		return nil
	}
	return usage(c, "msg", sub)
}

func runCompute(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return usage(c, "compute", "")
	}
	sub, rest := args[0], args[1:]
	switch sub {
	case "request":
		r, err := RequestCompute(c.Store, c.Project, actor(c), str(fs, "size"), num(fs, "hours"), str(fs, "reason"))
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"request": r})
		}
		switch r.Status {
		case "approved":
			c.Printf("%s approved: %s for %dh (auto, under the caps)\n", r.ID, r.Size, r.Hours)
		default:
			c.Printf("%s pending: %s for %dh - proposal %s waits for the owner\n", r.ID, r.Size, r.Hours, r.ProposalPost)
		}
		return nil
	case "list":
		list, err := ListCompute(c.Store, c.Project, str(fs, "status"))
		if err != nil {
			return err
		}
		caps, err := GetCaps(c.Store)
		if err != nil {
			return err
		}
		if list == nil {
			list = []ComputeRequest{}
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"requests": list, "caps": caps})
		}
		c.Printf("caps: provider %s  max_vms %d (in use %d)  max_hours_per_day %d (today %d)  auto_approve %v  budget_credits %d\n",
			caps.Provider, caps.MaxVMs, caps.VMsNow, caps.MaxHoursPerDay, caps.HoursToday, caps.AutoApprove, caps.BudgetCredits)
		if len(list) == 0 {
			c.Println("No compute requests.")
			return nil
		}
		rows := make([][]string, 0, len(list))
		for _, r := range list {
			rows = append(rows, []string{r.ID, r.Project, r.Status, r.RequestedBy, r.Size, strconv.Itoa(r.Hours), r.Executor, r.At, r.Reason})
		}
		cli.Table(c.Out, c.Color, []string{"ID", "PROJECT", "STATUS", "BY", "SIZE", "HOURS", "EXECUTOR", "AT", "REASON"}, rows)
		return nil
	case "approve", "deny":
		if err := need(rest, 1, "a request id"); err != nil {
			return err
		}
		who := actor(c)
		var r *ComputeRequest
		var err error
		if sub == "approve" {
			r, err = Approve(c.Store, rest[0], str(fs, "words"), who)
		} else {
			r, err = Deny(c.Store, rest[0], str(fs, "words"), who)
		}
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"request": r})
		}
		c.Printf("%s %s by %s\n", r.ID, r.Status, r.DecidedBy)
		return nil
	case "providers":
		if ComputeProviders == nil {
			return cli.Errorf(2, "No compute provider is built into this binary", "the runner registers them", "use a full pulse build")
		}
		return ComputeProviders(c)
	}
	return usage(c, "compute", sub)
}

// ComputeProviders prints the compute providers the runner registered
// (`pulse compute providers`); the runner package assigns it at init because
// this package must not import the runner.
var ComputeProviders func(c *cli.Context) error

func runWatch(c *cli.Context, fs *flag.FlagSet, _ []string) error {
	filter := bus.Filter{Project: c.Project, Area: str(fs, "area"), Thread: str(fs, "thread")}
	if a := str(fs, "action"); a != "" {
		filter.Actions = strings.Split(a, ",")
	}
	since, _ := strconv.ParseInt(str(fs, "since"), 10, 64)
	interval, _ := time.ParseDuration(str(fs, "interval"))
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	return Watch(ctx, c, since, filter, interval, boolean(fs, "detail"))
}

// Watch prints events from bus.Tail until ctx ends; it is what `pulse watch`
// runs and what a test drives with its own context.
func Watch(ctx context.Context, c *cli.Context, since int64, filter bus.Filter, interval time.Duration, detail bool) error {
	w := bufio.NewWriter(c.Out)
	defer w.Flush()
	redact := bus.Redactor(c.Store) // owner-private setting values never reach agents
	for e := range bus.Tail(ctx, c.Store, since, filter, interval) {
		e = redact(e)
		if c.JSON {
			b, err := jsonLine(e)
			if err != nil {
				return err
			}
			w.Write(b)
			w.WriteByte('\n')
		} else {
			fmt.Fprintf(w, "%s %s %s %s", e.At, e.Actor, e.Action, e.Subject)
			if detail && e.Detail != "" {
				fmt.Fprintf(w, " %s", c.Style().Dim(e.Detail))
			}
			w.WriteByte('\n')
		}
		if err := w.Flush(); err != nil {
			if errors.Is(err, syscall.EPIPE) {
				return nil
			}
			return err
		}
	}
	return nil
}

func jsonLine(e bus.Event) ([]byte, error) { return json.Marshal(e) }
