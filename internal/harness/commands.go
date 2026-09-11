package harness

import (
	"bufio"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/provider"
	"github.com/manishtiwari25/pulse/internal/provider/fake"
	"github.com/manishtiwari25/pulse/internal/store"
)

const (
	harnessUsage = "pulse harness run -p TEXT [-C DIR] [--model M] [--steps-file PATH] ... | login [--with-token] | status | models | logout | sessions | show ID"
	loginFix     = "sign in with GitHub in the console (Settings \u2192 Harness) or run pulse harness login"
)

// notLoggedInLine is the one stderr line the runner recognises as a login
// fault (it starts with "not logged in:"), carrying the provider's reason when
// it gave one, such as a GitHub account without a Copilot seat.
func notLoggedInLine(err error) string {
	reason := ""
	if err != nil {
		reason = strings.TrimSpace(strings.TrimPrefix(err.Error(), provider.ErrNotLoggedIn.Error()))
		reason = strings.TrimSpace(strings.TrimPrefix(reason, ":"))
	}
	if reason == "" {
		return "not logged in: " + loginFix
	}
	return "not logged in: " + reason + " - " + loginFix
}

func init() {
	cli.Register(cli.Command{
		Name:    "harness",
		Group:   "runner",
		Summary: "the built-in agent harness: run a task through the model provider, sign in, list models and sessions",
		Usage:   harnessUsage,
		NoStore: true,
		Raw:     true,
		Run:     runHarnessCommand,
	})
}

func runHarnessCommand(c *cli.Context, _ *flag.FlagSet, args []string) error {
	if os.Getenv("PULSE_HARNESS_FAKE") == "1" {
		fake.Register()
	}
	if len(args) == 0 {
		return unknownHarnessVerb("")
	}
	verb, rest := args[0], args[1:]
	switch verb {
	case "run":
		return harnessRun(c, rest)
	case "login":
		return harnessLogin(c, rest)
	case "status":
		return harnessStatus(c, rest)
	case "models":
		return harnessModels(c, rest)
	case "logout":
		return harnessLogout(c, rest)
	case "sessions":
		return harnessSessions(c, rest)
	case "show":
		return harnessShow(c, rest)
	default:
		return unknownHarnessVerb(verb)
	}
}

func unknownHarnessVerb(verb string) error {
	what := "pulse harness: missing verb"
	if verb != "" {
		what = fmt.Sprintf("pulse harness: unknown verb %q", verb)
	}
	return cli.Errorf(2, what, "", "pulse harness run|login|status|models|logout|sessions|show")
}

func harnessRun(c *cli.Context, args []string) error {
	fs := flag.NewFlagSet("harness run", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var taskText, taskFile, cwd, modelName, providerName, sessionID, trajectory, stepsFile string
	var maxSteps, stepTimeout, outputLimit, minutes, keepObservations int
	var maxCost float64
	var noStore bool
	fs.StringVar(&taskText, "p", "", "task text")
	fs.StringVar(&taskText, "task", "", "task text")
	fs.StringVar(&taskFile, "task-file", "", "read task from file, or - for stdin")
	fs.StringVar(&cwd, "C", ".", "working directory")
	fs.StringVar(&modelName, "model", "", "model id")
	fs.StringVar(&providerName, "provider", "", "provider name")
	fs.IntVar(&maxSteps, "max-steps", 0, "maximum model calls")
	fs.Float64Var(&maxCost, "max-cost", 0, "maximum provider-reported cost")
	fs.IntVar(&stepTimeout, "step-timeout", 0, "per-command timeout in seconds")
	fs.IntVar(&outputLimit, "output-limit", 0, "characters of command output kept")
	fs.IntVar(&keepObservations, "keep-observations", 0, "observations kept verbatim in context")
	fs.IntVar(&minutes, "minutes", 0, "wall-clock cap in minutes")
	fs.StringVar(&sessionID, "session", "", "harness session id")
	fs.StringVar(&trajectory, "trajectory", "", "trajectory path, or none")
	fs.StringVar(&stepsFile, "steps-file", "", "append newline-delimited harness events to PATH")
	fs.BoolVar(&noStore, "no-store", false, "do not open the PULSE store")
	bindRawGlobals(c, fs)
	if err := fs.Parse(args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return cli.Errorf(2, "pulse harness run: unrecognized arguments: "+strings.Join(fs.Args(), " "), "", "")
	}
	task, err := readTask(taskText, taskFile, os.Stdin)
	if err != nil {
		return err
	}
	absCwd, err := existingDir(cwd)
	if err != nil {
		return err
	}
	if sessionID == "" {
		sessionID = os.Getenv("PULSE_SESSION")
	}
	if sessionID == "" {
		sessionID = newHarnessSessionID()
	}
	if stepsFile == "" {
		stepsFile = os.Getenv("PULSE_HARNESS_STEPS")
	}
	var stepLog *stepsFileSink
	if stepsFile != "" {
		stepLog = &stepsFileSink{c: c, path: stepsFile, session: sessionID}
	}
	st, closeStore, err := openHarnessStore(c, noStore)
	if err != nil {
		return err
	}
	if closeStore != nil {
		defer closeStore()
	}
	if st == nil && !noStore && !c.Quiet {
		fmt.Fprintf(c.Err, "pulse harness: no store at %s; running without store defaults or session history\n", filepath.Join(c.Home, store.StoreName))
	}
	if providerName == "" {
		if st != nil {
			providerName = st.Setting("provider", provider.Default)
		} else {
			providerName = provider.Default
		}
	}
	p, err := provider.Open(providerName, provider.Options{Home: c.Home})
	if err != nil {
		return cli.Errorf(2, "pulse harness run: "+err.Error(), "", "pass --provider P")
	}
	if modelName == "" {
		modelName, err = defaultRunModel(context.Background(), st, p)
		if errors.Is(err, provider.ErrNotLoggedIn) {
			result := Result{ExitStatus: ExitNotLoggedIn, Error: err.Error()}
			fmt.Fprintln(c.Err, notLoggedInLine(err))
			if stepLog != nil {
				stepLog.begin(providerName, "", absCwd)
				stepLog.finish(result)
			}
			if st != nil {
				// Record the attempt so the console shows why nothing ran.
				rec := &storeSink{c: c, s: st, id: sessionID}
				if berr := rec.begin(providerName, "", absCwd, ""); berr != nil {
					fmt.Fprintf(c.Err, "pulse harness: store warning: %v\n", berr)
				} else if ferr := rec.finish(result, ""); ferr != nil {
					fmt.Fprintf(c.Err, "pulse harness: store warning: %v\n", ferr)
				}
			}
			if c.JSON {
				_ = cli.WriteJSON(c.Out, runJSON(sessionID, result, ""))
			} else {
				writeRunText(c.Out, nil, result)
			}
			return cli.Errorf(4, "", "", "")
		}
		if err != nil {
			return err
		}
	}
	if maxSteps == 0 {
		maxSteps = DefaultMaxSteps
		if st != nil {
			maxSteps = st.SettingInt("harness.max_steps")
		}
	}
	if maxCost == 0 && st != nil {
		maxCost = st.SettingFloat("harness.max_cost")
	}
	if stepTimeout == 0 {
		stepTimeout = int(DefaultStepTimeout.Seconds())
		if st != nil {
			stepTimeout = st.SettingInt("harness.step_timeout")
		}
	}
	if outputLimit == 0 {
		outputLimit = DefaultOutputLimit
		if st != nil {
			outputLimit = st.SettingInt("harness.output_limit")
		}
	}
	if keepObservations == 0 {
		keepObservations = DefaultKeepObservations
		if st != nil {
			keepObservations = st.SettingInt("harness.keep_observations")
		}
	}
	contextWindow := modelContextWindow(context.Background(), p, modelName)
	trajectoryPath := ""
	if trajectory != "none" {
		if trajectory == "" {
			trajectory = filepath.Join(c.Home, "harness", sessionID+".json")
		}
		if err := os.MkdirAll(filepath.Dir(trajectory), 0o700); err != nil {
			return err
		}
		trajectoryPath = trajectory
	}
	progressStart := time.Now()
	var sinks []Sink
	var rec *storeSink
	if !c.Quiet {
		sinks = append(sinks, progressSink(c.Err, progressStart))
	}
	if stepLog != nil {
		sinks = append(sinks, stepLog)
		stepLog.begin(providerName, modelName, absCwd)
	}
	if st != nil {
		rec = &storeSink{c: c, s: st, id: sessionID}
		sinks = append(sinks, rec)
		if err := rec.begin(providerName, modelName, absCwd, trajectoryPath); err != nil {
			fmt.Fprintf(c.Err, "pulse harness: store warning: %v\n", err)
		}
	}
	var trajectoryLog *trajectorySink
	if trajectoryPath != "" {
		trajectoryLog = &trajectorySink{c: c, path: trajectoryPath}
		sinks = append(sinks, trajectoryLog)
	}
	env := append(os.Environ(), "PULSE_HARNESS=1", "PULSE_HARNESS_SESSION="+sessionID, "GIT_TERMINAL_PROMPT=0")
	cfg := Config{
		Model: modelName, Task: task, Cwd: absCwd, Env: env, Provider: providerName,
		MaxSteps: maxSteps, StepTimeout: time.Duration(stepTimeout) * time.Second,
		MaxCost: maxCost, OutputLimit: outputLimit, KeepObservations: keepObservations, ContextWindow: contextWindow,
	}
	if minutes > 0 {
		cfg.WallTime = time.Duration(minutes) * time.Minute
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	agent := New(p, NewLocalEnv(absCwd, env), cfg, sinks...)
	if trajectoryLog != nil {
		trajectoryLog.agent = agent
	}
	result, runErr := agent.Run(ctx)
	if runErr != nil {
		return runErr
	}
	if trajectoryPath != "" {
		if err := WriteTrajectory(trajectoryPath, agent.Trajectory()); err != nil {
			fmt.Fprintf(c.Err, "pulse harness: trajectory warning: %v\n", err)
		}
	}
	if rec != nil {
		if err := rec.finish(result, trajectoryPath); err != nil {
			fmt.Fprintf(c.Err, "pulse harness: store warning: %v\n", err)
		}
	}
	if stepLog != nil {
		stepLog.finish(result)
	}
	if result.ExitStatus == ExitNotLoggedIn {
		fmt.Fprintln(c.Err, notLoggedInLine(errors.New(result.Error)))
	}
	if c.JSON {
		if !c.Quiet {
			return cli.WriteJSON(c.Out, runJSON(sessionID, result, trajectoryPath))
		}
		if result.ExitStatus == ExitSubmitted {
			return nil
		}
		return cli.Errorf(exitCodeFor(result.ExitStatus), "", "", "")
	}
	writeRunText(c.Out, agent.Trajectory().Steps, result)
	if result.ExitStatus == ExitSubmitted {
		return nil
	}
	return cli.Errorf(exitCodeFor(result.ExitStatus), "", "", "")
}

func harnessLogin(c *cli.Context, args []string) error {
	fs := flag.NewFlagSet("harness login", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var providerName string
	var withToken bool
	fs.StringVar(&providerName, "provider", provider.Default, "provider name")
	fs.BoolVar(&withToken, "with-token", false, "read token from stdin")
	bindRawGlobals(c, fs)
	if err := fs.Parse(args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return cli.Errorf(2, "pulse harness login: unrecognized arguments: "+strings.Join(fs.Args(), " "), "", "")
	}
	p, err := provider.Open(providerName, provider.Options{Home: c.Home})
	if err != nil {
		return cli.Errorf(2, "pulse harness login: "+err.Error(), "", "pass --provider P")
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	var login provider.Login
	if withToken {
		tl, ok := p.(provider.TokenLoginer)
		if !ok {
			return cli.Errorf(2, "pulse harness login: provider "+p.Label()+" does not support token login", "", "")
		}
		data, err := io.ReadAll(os.Stdin)
		if err != nil {
			return err
		}
		token := strings.TrimSpace(string(data))
		if token == "" {
			return cli.Errorf(2, "pulse harness login: empty token", "", "pipe a GitHub token to pulse harness login --with-token")
		}
		login, err = tl.LoginWithToken(ctx, token)
		if err != nil {
			return err
		}
	} else {
		dl, ok := p.(provider.DeviceLoginer)
		if !ok {
			return cli.Errorf(2, "pulse harness login: provider "+p.Label()+" does not support device login", "", "")
		}
		flow, err := dl.BeginDeviceLogin(ctx)
		if err != nil {
			return err
		}
		code := c.Style().Bold(flow.UserCode)
		fmt.Fprintf(c.Err, "Open %s and enter the code %s\n", flow.VerificationURI, code)
		interval := flow.Interval
		if interval <= 0 {
			interval = 5
		}
		deadline := time.Now().Add(time.Duration(flow.ExpiresIn) * time.Second)
		for {
			if flow.ExpiresIn > 0 && time.Now().After(deadline) {
				return cli.Errorf(2, "pulse harness login: device code expired", "", "run pulse harness login again")
			}
			timer := time.NewTimer(time.Duration(interval) * time.Second)
			select {
			case <-timer.C:
			case <-ctx.Done():
				timer.Stop()
				return ctx.Err()
			}
			state, got, err := dl.PollDeviceLogin(ctx, flow.DeviceCode)
			if err != nil {
				return err
			}
			switch state {
			case provider.DeviceComplete:
				login = got
				goto done
			case provider.DevicePending:
			case provider.DeviceSlowDown:
				interval += 5
			case provider.DeviceExpired:
				return cli.Errorf(2, "pulse harness login: device code expired", "", "run pulse harness login again")
			case provider.DeviceDenied:
				return cli.Errorf(2, "pulse harness login: device login denied", "", "")
			default:
				return cli.Errorf(2, "pulse harness login: unexpected device login state "+state, "", "")
			}
		}
	}
done:
	if c.JSON {
		return cli.WriteJSON(c.Out, login)
	}
	fmt.Fprintf(c.Out, "Signed in as %s (%s)\n", login.User, p.Label())
	return nil
}

func harnessStatus(c *cli.Context, args []string) error {
	fs := flag.NewFlagSet("harness status", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var providerName string
	fs.StringVar(&providerName, "provider", "", "provider name")
	bindRawGlobals(c, fs)
	if err := fs.Parse(args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return cli.Errorf(2, "pulse harness status: unrecognized arguments: "+strings.Join(fs.Args(), " "), "", "")
	}
	st, closeStore, err := openHarnessStore(c, false)
	if err != nil {
		return err
	}
	if closeStore != nil {
		defer closeStore()
	}
	if providerName == "" {
		providerName = provider.Default
		if st != nil {
			providerName = st.Setting("provider", provider.Default)
		}
	}
	p, err := provider.Open(providerName, provider.Options{Home: c.Home})
	if err != nil {
		return cli.Errorf(2, "pulse harness status: "+err.Error(), "", "pass --provider P")
	}
	login, loginErr := p.Status(context.Background())
	if loginErr != nil {
		login = provider.Login{Detail: oneLine(loginErr.Error(), 200)}
	}
	status := statusReport(st, p, login, loginErr)
	if c.JSON {
		return cli.WriteJSON(c.Out, status)
	}
	writeHarnessStatus(c.Out, status)
	return nil
}

func harnessModels(c *cli.Context, args []string) error {
	fs := flag.NewFlagSet("harness models", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var providerName string
	fs.StringVar(&providerName, "provider", provider.Default, "provider name")
	bindRawGlobals(c, fs)
	if err := fs.Parse(args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return cli.Errorf(2, "pulse harness models: unrecognized arguments: "+strings.Join(fs.Args(), " "), "", "")
	}
	p, err := provider.Open(providerName, provider.Options{Home: c.Home})
	if err != nil {
		return cli.Errorf(2, "pulse harness models: "+err.Error(), "", "pass --provider P")
	}
	models, err := p.Models(context.Background())
	if errors.Is(err, provider.ErrNotLoggedIn) {
		fmt.Fprintln(c.Err, notLoggedInLine(err))
		return cli.Errorf(4, "", "", "")
	}
	if err != nil {
		return err
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, models)
	}
	rows := make([][]string, 0, len(models))
	for _, m := range models {
		id := m.ID
		if m.Default {
			id += " *"
		}
		premium := "-"
		if m.Premium > 0 {
			premium = "x" + strconv.FormatFloat(m.Premium, 'f', -1, 64)
		}
		rows = append(rows, []string{id, m.Name, m.Vendor, yesNo(m.ToolCalls), premium, intText(m.ContextWindow), intText(m.MaxOutput)})
	}
	cli.Table(c.Out, c.Color, []string{"ID", "NAME", "VENDOR", "TOOLS", "PREMIUM", "CONTEXT", "OUTPUT"}, rows)
	return nil
}

func harnessLogout(c *cli.Context, args []string) error {
	fs := flag.NewFlagSet("harness logout", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var providerName string
	fs.StringVar(&providerName, "provider", provider.Default, "provider name")
	bindRawGlobals(c, fs)
	if err := fs.Parse(args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return cli.Errorf(2, "pulse harness logout: unrecognized arguments: "+strings.Join(fs.Args(), " "), "", "")
	}
	p, err := provider.Open(providerName, provider.Options{Home: c.Home})
	if err != nil {
		return cli.Errorf(2, "pulse harness logout: "+err.Error(), "", "pass --provider P")
	}
	l, ok := p.(provider.Logouter)
	if !ok {
		return cli.Errorf(2, "pulse harness logout: provider "+p.Label()+" does not support logout", "", "")
	}
	if err := l.Logout(); err != nil {
		return err
	}
	fmt.Fprintf(c.Out, "removed stored %s login\n", p.Label())
	login, err := p.Status(context.Background())
	if err == nil && login.LoggedIn {
		fmt.Fprintf(c.Out, "still signed in via %s", login.Source)
		if login.Detail != "" {
			fmt.Fprintf(c.Out, ": %s", login.Detail)
		}
		fmt.Fprintln(c.Out)
	}
	return nil
}

func harnessSessions(c *cli.Context, args []string) error {
	fs := flag.NewFlagSet("harness sessions", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	var project, status string
	var limit int
	fs.StringVar(&project, "project", "", "project slug")
	fs.StringVar(&status, "status", "", "session status")
	fs.IntVar(&limit, "limit", 50, "maximum sessions")
	bindRawGlobals(c, fs)
	if err := fs.Parse(args); err != nil {
		return err
	}
	if fs.NArg() != 0 {
		return cli.Errorf(2, "pulse harness sessions: unrecognized arguments: "+strings.Join(fs.Args(), " "), "", "")
	}
	st, closeStore, err := requireHarnessStore(c)
	if err != nil {
		return err
	}
	defer closeStore()
	sessions, err := st.HarnessSessions(store.HarnessSessionFilter{Project: project, Status: status, Limit: limit})
	if err != nil {
		return err
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, sessions)
	}
	writeSessionsTable(c.Out, c.Color, sessions)
	return nil
}

func harnessShow(c *cli.Context, args []string) error {
	fs := flag.NewFlagSet("harness show", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	bindRawGlobals(c, fs)
	if err := fs.Parse(args); err != nil {
		return err
	}
	if fs.NArg() != 1 {
		return cli.Errorf(2, "pulse harness show: expected SESSION_ID", "", "pulse harness show SESSION_ID")
	}
	st, closeStore, err := requireHarnessStore(c)
	if err != nil {
		return err
	}
	defer closeStore()
	session, steps, err := st.HarnessSession(fs.Arg(0))
	if err != nil {
		return err
	}
	if session == nil {
		return cli.Errorf(2, "pulse harness show: no harness session "+fs.Arg(0), "", "pulse harness sessions")
	}
	if c.JSON {
		return cli.WriteJSON(c.Out, struct {
			Session *store.HarnessSession `json:"session"`
			Steps   []store.HarnessStep   `json:"steps"`
		}{session, steps})
	}
	writeHarnessTranscript(c.Out, *session, steps)
	return nil
}

func bindRawGlobals(c *cli.Context, fs *flag.FlagSet) {
	fs.BoolFunc("json", "machine-readable output", func(string) error { c.JSON = true; return nil })
	fs.BoolFunc("quiet", "print only what a script needs", func(string) error { c.Quiet = true; return nil })
	fs.BoolFunc("no-color", "no colour", func(string) error { c.Color = false; return nil })
}

func readTask(text, file string, stdin io.Reader) (string, error) {
	if strings.TrimSpace(text) == "" && file == "" {
		return "", cli.Errorf(2, "pulse harness run: exactly one of -p/--task or --task-file is required", "", "pass -p TEXT or --task-file F")
	}
	if strings.TrimSpace(text) != "" && file != "" {
		return "", cli.Errorf(2, "pulse harness run: exactly one of -p/--task or --task-file is required", "", "pass only one task source")
	}
	if file == "" {
		return text, nil
	}
	var data []byte
	var err error
	if file == "-" {
		data, err = io.ReadAll(stdin)
	} else {
		data, err = os.ReadFile(file)
	}
	if err != nil {
		return "", err
	}
	if strings.TrimSpace(string(data)) == "" {
		return "", cli.Errorf(2, "pulse harness run: task is empty", "", "")
	}
	return string(data), nil
}

func existingDir(path string) (string, error) {
	abs, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	info, err := os.Stat(abs)
	if err != nil {
		return "", cli.Errorf(2, "pulse harness run: working directory does not exist: "+abs, "", "")
	}
	if !info.IsDir() {
		return "", cli.Errorf(2, "pulse harness run: working directory is not a directory: "+abs, "", "")
	}
	return abs, nil
}

func openHarnessStore(c *cli.Context, noStore bool) (*store.Store, func(), error) {
	if noStore {
		return nil, nil, nil
	}
	if c.Store != nil {
		return c.Store, func() {}, nil
	}
	path := filepath.Join(c.Home, store.StoreName)
	if _, err := os.Stat(path); errors.Is(err, os.ErrNotExist) {
		return nil, nil, nil
	} else if err != nil {
		return nil, nil, err
	}
	st, err := store.Open(store.Options{Home: c.Home, Now: c.Now})
	if err != nil {
		return nil, nil, err
	}
	return st, func() { _ = st.Close() }, nil
}

func requireHarnessStore(c *cli.Context) (*store.Store, func(), error) {
	st, closeStore, err := openHarnessStore(c, false)
	if err != nil {
		return nil, nil, err
	}
	if st == nil {
		return nil, nil, cli.Errorf(2, "pulse harness: no store at "+filepath.Join(c.Home, store.StoreName), "", "pulse init <project-path>")
	}
	return st, closeStore, nil
}

func defaultRunModel(ctx context.Context, st *store.Store, p provider.Provider) (string, error) {
	if st != nil {
		if model := strings.TrimSpace(st.RunModel("task")); model != "" {
			return model, nil
		}
	}
	models, err := p.Models(ctx)
	if errors.Is(err, provider.ErrNotLoggedIn) {
		return "", err
	}
	if err != nil {
		return "", err
	}
	for _, m := range models {
		if m.Default && m.Chat {
			return m.ID, nil
		}
	}
	for _, m := range models {
		if m.Chat {
			return m.ID, nil
		}
	}
	return "", cli.Errorf(2, "pulse harness run: no default chat model available", "", "pass --model M")
}

func modelContextWindow(ctx context.Context, p provider.Provider, model string) int {
	models, err := p.Models(ctx)
	if err != nil {
		return 0
	}
	for _, m := range models {
		if m.ID == model {
			return m.ContextWindow
		}
	}
	return 0
}

func newHarnessSessionID() string {
	var b [3]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "hs-" + time.Now().UTC().Format("20060102T150405") + "-000000"
	}
	return "hs-" + time.Now().UTC().Format("20060102T150405") + "-" + hex.EncodeToString(b[:])
}

func progressSink(w io.Writer, start time.Time) Sink {
	return SinkFunc(func(s Step) {
		elapsed := time.Since(start).Truncate(time.Second)
		prefix := fmt.Sprintf("%02d:%02d [%d] ", int(elapsed.Minutes()), int(elapsed.Seconds())%60, s.N)
		switch s.Kind {
		case StepAssistant:
			fmt.Fprintf(w, "%sassistant: %s\n", prefix, oneLine(s.Content, 160))
		case StepCommand:
			fmt.Fprintf(w, "%s$ %s\n", prefix, oneLine(s.Command, 160))
		case StepObservation:
			code := 0
			if s.ExitCode != nil {
				code = *s.ExitCode
			}
			fmt.Fprintf(w, "%sexit %d in %dms (%d chars)\n", prefix, code, s.DurationMS, len(s.Content))
		case StepNote:
			fmt.Fprintf(w, "%snote: %s\n", prefix, oneLine(s.Content, 160))
		case StepError:
			fmt.Fprintf(w, "%serror: %s\n", prefix, oneLine(s.Content, 160))
		case StepExit:
			fmt.Fprintf(w, "%sexit: %s\n", prefix, s.Content)
		}
	})
}

type storeSink struct {
	c        *cli.Context
	s        *store.Store
	id       string
	provider string
	model    string
	warned   bool
}

func (s *storeSink) begin(providerName, model, cwd, trajectory string) error {
	s.provider, s.model = providerName, model
	if err := s.s.BeginHarnessSession(store.HarnessSession{
		ID: s.id, RunID: os.Getenv("PULSE_SESSION"), JobID: os.Getenv("PULSE_JOB"), Agent: os.Getenv("PULSE_AGENT"),
		Project: os.Getenv("PULSE_PROJECT"), Executor: os.Getenv("PULSE_EXECUTOR"), Provider: providerName, Model: model,
		Cwd: cwd, Status: "running", Trajectory: trajectory,
	}); err != nil {
		return err
	}
	_, err := bus.Emit(s.s, actorOf(s.c), "harness.start", s.id, map[string]string{
		"project": os.Getenv("PULSE_PROJECT"), "agent": os.Getenv("PULSE_AGENT"), "job": os.Getenv("PULSE_JOB"),
		"model": model, "provider": providerName,
	})
	return err
}

func (s *storeSink) Step(step Step) {
	err := s.s.AppendHarnessStep(store.HarnessStep{
		SessionID: s.id, N: step.N, At: step.At.UTC().Format(time.RFC3339), Kind: string(step.Kind),
		Content: step.Content, Command: step.Command, ExitCode: step.ExitCode, DurationMS: step.DurationMS,
		PromptTokens: step.Usage.PromptTokens, CompletionTokens: step.Usage.CompletionTokens, Model: step.Model,
	})
	if err != nil {
		s.warn(err)
		return
	}
	payload := map[string]any{"session": s.id, "n": step.N, "kind": string(step.Kind)}
	if step.ExitCode != nil {
		payload["exit_code"] = *step.ExitCode
	}
	if job := os.Getenv("PULSE_JOB"); job != "" {
		payload["job"] = job
	}
	if runID := os.Getenv("PULSE_SESSION"); runID != "" {
		payload["run_id"] = runID
	}
	if _, err := bus.Emit(s.s, actorOf(s.c), "harness.step", s.id, payload); err != nil {
		s.warn(err)
	}
}

func (s *storeSink) finish(result Result, trajectory string) error {
	if err := s.s.FinishHarnessSession(s.id, result.ExitStatus, result.ExitStatus, result.Submission, result.Error, result.Usage.Cost, trajectory); err != nil {
		return err
	}
	_, err := bus.Emit(s.s, actorOf(s.c), "harness.finish", s.id, map[string]any{
		"project": os.Getenv("PULSE_PROJECT"), "agent": os.Getenv("PULSE_AGENT"), "job": os.Getenv("PULSE_JOB"),
		"model": s.model, "provider": s.provider, "status": result.ExitStatus, "steps": result.Steps, "calls": result.Calls,
	})
	return err
}

func (s *storeSink) warn(err error) {
	if s.warned || err == nil {
		return
	}
	s.warned = true
	fmt.Fprintf(s.c.Err, "pulse harness: store warning: %v\n", err)
}

type stepsFileSink struct {
	c        *cli.Context
	path     string
	session  string
	provider string
	model    string
	warned   bool
	count    int
}

type trajectorySink struct {
	c      *cli.Context
	path   string
	agent  *Agent
	warned bool
}

func (s *trajectorySink) Step(Step) {
	if s.agent == nil || s.path == "" || s.warned {
		return
	}
	if err := WriteTrajectory(s.path, s.agent.Trajectory()); err != nil {
		s.warn(err)
	}
}

func (s *trajectorySink) warn(err error) {
	if s.warned || err == nil {
		return
	}
	s.warned = true
	if s.c != nil {
		fmt.Fprintf(s.c.Err, "pulse harness: trajectory warning: %v\n", err)
	}
}

func (s *stepsFileSink) begin(providerName, model, cwd string) {
	s.provider, s.model = providerName, model
	s.write(map[string]any{
		"event": "begin", "session": s.session, "run_id": os.Getenv("PULSE_SESSION"), "job_id": os.Getenv("PULSE_JOB"),
		"agent": os.Getenv("PULSE_AGENT"), "provider": providerName, "model": model, "cwd": cwd,
		"at": time.Now().UTC().Format(time.RFC3339),
	})
}

func (s *stepsFileSink) Step(step Step) {
	s.count++
	ev := struct {
		Event string `json:"event"`
		Step
	}{Event: "step", Step: step}
	s.write(ev)
}

func (s *stepsFileSink) finish(result Result) {
	s.write(map[string]any{
		"event": "finish", "exit_status": result.ExitStatus, "submission": result.Submission, "error": result.Error,
		"cost": result.Usage.Cost, "steps": s.count, "at": time.Now().UTC().Format(time.RFC3339),
	})
}

func (s *stepsFileSink) write(v any) {
	if s.warned || s.path == "" {
		return
	}
	if err := os.MkdirAll(filepath.Dir(s.path), 0o700); err != nil {
		s.warn(err)
		return
	}
	data, err := json.Marshal(v)
	if err != nil {
		s.warn(err)
		return
	}
	data = append(data, '\n')
	f, err := os.OpenFile(s.path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		s.warn(err)
		return
	}
	_, werr := f.Write(data)
	cerr := f.Close()
	if werr != nil {
		s.warn(werr)
	} else if cerr != nil {
		s.warn(cerr)
	}
}

func (s *stepsFileSink) warn(err error) {
	if s.warned || err == nil {
		return
	}
	s.warned = true
	fmt.Fprintf(s.c.Err, "pulse harness: steps-file warning: %v\n", err)
}

func actorOf(c *cli.Context) string {
	if c.Agent != "" {
		return c.Agent
	}
	if a := os.Getenv("PULSE_AGENT"); a != "" {
		return a
	}
	return store.HumanAgent
}

func runJSON(session string, result Result, trajectory string) map[string]any {
	return map[string]any{
		"session": session, "exit_status": result.ExitStatus, "submission": result.Submission,
		"steps": result.Steps, "calls": result.Calls, "usage": result.Usage,
		"elapsed_seconds": result.Elapsed.Seconds(), "error": result.Error, "trajectory": trajectory,
	}
}

func writeRunText(w io.Writer, steps []Step, result Result) {
	if result.ExitStatus == ExitSubmitted {
		fmt.Fprint(w, result.Submission)
		if !strings.HasSuffix(result.Submission, "\n") {
			fmt.Fprintln(w)
		}
		return
	}
	summary := fmt.Sprintf("SUMMARY: harness stopped: %s after %d model calls and %d commands", result.ExitStatus, result.Calls, result.Steps)
	if result.Error != "" {
		summary += "; error: " + oneLine(result.Error, 200)
	}
	fmt.Fprintln(w, summary)
	for i := len(steps) - 1; i >= 0; i-- {
		if steps[i].Kind == StepAssistant && strings.TrimSpace(steps[i].Content) != "" {
			fmt.Fprintf(w, "FINDING: last message: %s\n", oneLine(steps[i].Content, 200))
			return
		}
	}
}

func exitCodeFor(status string) int {
	switch status {
	case ExitSubmitted:
		return 0
	case ExitProviderError:
		return 3
	case ExitNotLoggedIn:
		return 4
	case ExitStepLimit, ExitTimeLimit, ExitCostLimit, ExitFormatError, ExitCancelled:
		return 1
	default:
		return 1
	}
}

type harnessStatusReport struct {
	Harness  string `json:"harness"`
	Provider struct {
		Name  string         `json:"name"`
		Label string         `json:"label"`
		Login provider.Login `json:"login"`
	} `json:"provider"`
	Models struct {
		Worker string `json:"worker"`
		Light  string `json:"light"`
	} `json:"models"`
	Limits struct {
		MaxSteps           int     `json:"max_steps"`
		MaxCost            float64 `json:"max_cost"`
		StepTimeoutSeconds int     `json:"step_timeout_seconds"`
		OutputLimit        int     `json:"output_limit"`
	} `json:"limits"`
	Sessions *struct {
		Running     int `json:"running"`
		Today       int `json:"today"`
		FailedToday int `json:"failed_today"`
	} `json:"sessions,omitempty"`
}

func statusReport(st *store.Store, p provider.Provider, login provider.Login, loginErr error) harnessStatusReport {
	var r harnessStatusReport
	r.Harness = "native"
	r.Provider.Name, r.Provider.Label, r.Provider.Login = p.Name(), p.Label(), login
	r.Models.Worker, r.Models.Light = "", "same as worker"
	r.Limits.MaxSteps = DefaultMaxSteps
	r.Limits.MaxCost = 0
	r.Limits.StepTimeoutSeconds = int(DefaultStepTimeout.Seconds())
	r.Limits.OutputLimit = DefaultOutputLimit
	if st != nil {
		r.Harness = st.Setting("harness", "native")
		r.Models.Worker = st.Setting("worker_model", "")
		if light := st.Setting("light_model", ""); light != "" {
			r.Models.Light = light
		}
		r.Limits.MaxSteps = st.SettingInt("harness.max_steps")
		r.Limits.MaxCost = st.SettingFloat("harness.max_cost")
		r.Limits.StepTimeoutSeconds = st.SettingInt("harness.step_timeout")
		r.Limits.OutputLimit = st.SettingInt("harness.output_limit")
		if running, today, failed, err := st.HarnessSessionCounts(); err == nil {
			r.Sessions = &struct {
				Running     int `json:"running"`
				Today       int `json:"today"`
				FailedToday int `json:"failed_today"`
			}{running, today, failed}
		}
	}
	if loginErr != nil {
		r.Provider.Login.Detail = "status unknown (" + oneLine(loginErr.Error(), 200) + ")"
	}
	return r
}

func writeHarnessStatus(w io.Writer, r harnessStatusReport) {
	login := "not signed in"
	if r.Provider.Login.LoggedIn {
		login = "signed in as " + r.Provider.Login.User
		if r.Provider.Login.Source != "" {
			login += " via " + r.Provider.Login.Source
		}
	} else if r.Provider.Login.Detail != "" {
		login += " - " + r.Provider.Login.Detail
	} else {
		login += " - " + loginFix
	}
	fmt.Fprintf(w, "harness  %s\nprovider %s (%s)\nlogin    %s\n", r.Harness, r.Provider.Label, r.Provider.Name, login)
	fmt.Fprintf(w, "models   worker=%s, light=%s\n", orDefault(r.Models.Worker, "provider default"), r.Models.Light)
	fmt.Fprintf(w, "limits   max_steps=%d, max_cost=%s, step_timeout=%ds, output_limit=%d\n", r.Limits.MaxSteps, strconv.FormatFloat(r.Limits.MaxCost, 'f', -1, 64), r.Limits.StepTimeoutSeconds, r.Limits.OutputLimit)
	if r.Sessions != nil {
		fmt.Fprintf(w, "sessions running=%d, today=%d, failed_today=%d\n", r.Sessions.Running, r.Sessions.Today, r.Sessions.FailedToday)
	}
}

func writeSessionsTable(w io.Writer, color bool, sessions []store.HarnessSession) {
	rows := make([][]string, 0, len(sessions))
	for _, h := range sessions {
		rows = append(rows, []string{h.ID, h.Status, h.Agent, h.Project, h.Model, strconv.Itoa(h.Steps), strconv.Itoa(h.Calls),
			strconv.Itoa(h.PromptTokens + h.CompletionTokens), h.StartedAt})
	}
	cli.Table(w, color, []string{"ID", "STATUS", "AGENT", "PROJECT", "MODEL", "STEPS", "CALLS", "TOKENS", "STARTED"}, rows)
}

func writeHarnessTranscript(w io.Writer, h store.HarnessSession, steps []store.HarnessStep) {
	fmt.Fprintf(w, "%s %s project=%s agent=%s model=%s\n\n", h.ID, h.Status, h.Project, h.Agent, h.Model)
	for _, s := range steps {
		switch s.Kind {
		case string(StepAssistant):
			fmt.Fprintf(w, "[%d] assistant\n%s\n\n", s.N, s.Content)
		case string(StepCommand):
			fmt.Fprintf(w, "[%d] $ %s\n\n", s.N, s.Command)
		case string(StepObservation):
			code := "-"
			if s.ExitCode != nil {
				code = strconv.Itoa(*s.ExitCode)
			}
			fmt.Fprintf(w, "[%d] output exit=%s duration=%dms\n%s\n\n", s.N, code, s.DurationMS, headTail(s.Content, 20))
		case string(StepError):
			fmt.Fprintf(w, "[%d] error: %s\n\n", s.N, s.Content)
		case string(StepExit):
			fmt.Fprintf(w, "[%d] exit: %s\n\n", s.N, s.Content)
		}
	}
	fmt.Fprintf(w, "exit_status: %s\n", h.ExitStatus)
	if h.Error != "" {
		fmt.Fprintf(w, "error: %s\n", h.Error)
	}
	if h.Submission != "" {
		fmt.Fprintf(w, "submission:\n%s\n", h.Submission)
	}
}

func headTail(text string, n int) string {
	sc := bufio.NewScanner(strings.NewReader(text))
	var lines []string
	for sc.Scan() {
		lines = append(lines, sc.Text())
	}
	if len(lines) <= 2*n {
		return strings.Join(lines, "\n")
	}
	return strings.Join(append(append([]string{}, lines[:n]...), append([]string{"..."}, lines[len(lines)-n:]...)...), "\n")
}

func yesNo(ok bool) string {
	if ok {
		return "yes"
	}
	return "no"
}

func intText(n int) string {
	if n == 0 {
		return "-"
	}
	return strconv.Itoa(n)
}

func orDefault(s, def string) string {
	if s == "" {
		return def
	}
	return s
}
