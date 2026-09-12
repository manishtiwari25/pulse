// Package harness runs a small autonomous agent loop over one bash tool.
package harness

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"text/template"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
)

const (
	DefaultMaxSteps         = 60
	DefaultStepTimeout      = 5 * time.Minute
	DefaultOutputLimit      = 20000
	DefaultKeepObservations = 5
	DefaultMaxFormatErrors  = 3
)

var BashTool = provider.Tool{
	Name:        "bash",
	Description: "Run one non-interactive /bin/sh command in the working directory. Each call is a fresh process; chain setup and use with &&.",
	Parameters:  json.RawMessage(`{"type":"object","properties":{"command":{"type":"string","description":"The shell command to run with /bin/sh -c."}},"required":["command"]}`),
}

var DefaultDeny = []string{
	`(?i)(^|[;&|]\s*)git\s+push\b`,
	`(?i)(^|[;&|]\s*)git\s+remote\s+(add|set-url|remove)\b`,
	`(?i)(^|[;&|]\s*)gh\s+`,
	"(^|[\\s\"'`])(~/.ssh/|\\.netrc\\b|/private/|~/.pulse/private\\b|~/.copilot\\b|~/.config/gh\\b)",
}

type Config struct {
	Model                string
	Task                 string
	SystemPrompt         string
	SystemPromptTemplate string
	Cwd                  string
	Env                  []string
	MaxSteps             int
	StepTimeout          time.Duration
	WallTime             time.Duration
	MaxCost              float64
	OutputLimit          int
	KeepObservations     int
	ContextWindow        int
	MaxFormatErrors      int
	Deny                 []string
	Provider             string
	CancelFile           string
}

type StepKind string

const (
	StepAssistant   StepKind = "assistant"
	StepCommand     StepKind = "command"
	StepObservation StepKind = "observation"
	StepNote        StepKind = "note"
	StepError       StepKind = "error"
	StepExit        StepKind = "exit"
)

type Step struct {
	N          int            `json:"n"`
	At         time.Time      `json:"at"`
	Kind       StepKind       `json:"kind"`
	Content    string         `json:"content"`
	Command    string         `json:"command,omitempty"`
	ExitCode   *int           `json:"exit_code,omitempty"`
	DurationMS int64          `json:"duration_ms,omitempty"`
	Usage      provider.Usage `json:"usage"`
	Model      string         `json:"model,omitempty"`
}

type Sink interface{ Step(Step) }
type SinkFunc func(Step)

func (f SinkFunc) Step(s Step) { f(s) }

const (
	ExitSubmitted     = "submitted"
	ExitStepLimit     = "step_limit"
	ExitTimeLimit     = "time_limit"
	ExitCostLimit     = "cost_limit"
	ExitFormatError   = "format_error"
	ExitProviderError = "provider_error"
	ExitNotLoggedIn   = "not_logged_in"
	ExitCancelled     = "cancelled"
)

type Result struct {
	ExitStatus string         `json:"exit_status"`
	Submission string         `json:"submission"`
	Steps      int            `json:"steps"`
	Calls      int            `json:"calls"`
	Usage      provider.Usage `json:"usage"`
	Elapsed    time.Duration  `json:"elapsed"`
	Error      string         `json:"error,omitempty"`
}

type Observation struct {
	Output   string
	ExitCode int
	TimedOut bool
	Duration time.Duration
}

type Environment interface {
	Execute(ctx context.Context, command string, timeout time.Duration) Observation
	Info() map[string]string
}

type Agent struct {
	p            provider.Provider
	env          Environment
	cfg          Config
	sinks        []Sink
	deny         []denyRule
	msgs         []provider.Message
	steps        []Step
	nextN        int
	ran          bool
	result       Result
	started      time.Time
	cancelReason string
}

var ErrOwnerCancelled = errors.New("cancelled by the owner")

type denyRule struct {
	raw         string
	description string
	re          *regexp.Regexp
}

func New(p provider.Provider, env Environment, cfg Config, sinks ...Sink) *Agent {
	return &Agent{p: p, env: env, cfg: cfg, sinks: sinks}
}

func (a *Agent) Run(ctx context.Context) (Result, error) {
	if a.ran {
		return Result{}, errors.New("harness agent can only run once")
	}
	a.ran = true
	if a.p == nil {
		return Result{}, errors.New("harness provider is nil")
	}
	if a.env == nil {
		return Result{}, errors.New("harness environment is nil")
	}
	a.cfg = applyDefaults(a.cfg)
	if strings.TrimSpace(a.cfg.Model) == "" {
		return Result{}, errors.New("harness model is empty")
	}
	if strings.TrimSpace(a.cfg.Task) == "" {
		return Result{}, errors.New("harness task is empty")
	}
	if ctx == nil {
		ctx = context.Background()
	}
	a.started = time.Now()
	if a.cfg.SystemPrompt == "" {
		tmpl := a.cfg.SystemPromptTemplate
		if tmpl == "" {
			tmpl = DefaultPromptTemplate
		}
		prompt, err := RenderPrompt(tmpl, promptData(a.env.Info(), a.cfg))
		if err != nil {
			return Result{}, err
		}
		a.cfg.SystemPrompt = prompt
	}
	if err := a.compileDeny(); err != nil {
		return Result{}, err
	}
	runCtx := ctx
	if a.cfg.WallTime > 0 {
		var cancel context.CancelFunc
		runCtx, cancel = context.WithDeadline(ctx, a.started.Add(a.cfg.WallTime))
		defer cancel()
	}
	a.msgs = []provider.Message{
		{Role: provider.RoleSystem, Content: a.cfg.SystemPrompt},
		{Role: provider.RoleUser, Content: a.cfg.Task},
	}
	initiator := "user"
	formatErrors := 0
	keepObservations := a.cfg.KeepObservations
	lastPromptTokens := 0
	lastPruneLevel := 0
	emittedPruneNotes := map[string]bool{}
	for {
		if status := a.limitStatus(ctx); status != "" {
			return a.finish(status, "", a.limitError(status)), nil
		}
		if a.result.Calls > 0 {
			level := 1
			if a.cfg.ContextWindow > 0 && lastPromptTokens > int(0.8*float64(a.cfg.ContextWindow)) {
				level = 2
				if lastPruneLevel == 2 && keepObservations > 1 {
					keepObservations = max(1, keepObservations/2)
				}
			}
			next, changed := pruneHistory(a.msgs, keepObservations, level)
			a.msgs = next
			key := fmt.Sprintf("%d/%d", level, keepObservations)
			if changed > 0 && !emittedPruneNotes[key] {
				a.emit(Step{Kind: StepNote, Content: fmt.Sprintf("collapsed %d older observations to fit the context window (level %d, keep %d)", changed, level, keepObservations)})
				emittedPruneNotes[key] = true
			}
			lastPruneLevel = level
		}
		req := provider.Request{Model: a.cfg.Model, Messages: cloneMessages(a.msgs), Tools: []provider.Tool{BashTool}, Initiator: initiator}
		resp, err := a.p.Chat(runCtx, req)
		a.result.Calls++
		if err != nil {
			if a.cfg.WallTime > 0 && time.Since(a.started) >= a.cfg.WallTime {
				return a.finish(ExitTimeLimit, "", a.limitError(ExitTimeLimit)), nil
			}
			status := ExitProviderError
			if errors.Is(err, provider.ErrNotLoggedIn) {
				status = ExitNotLoggedIn
			}
			msg := oneLine(err.Error(), 400)
			a.emit(Step{Kind: StepError, Content: msg, Model: resp.Model})
			return a.finish(status, "", msg), nil
		}
		initiator = "agent"
		a.result.Usage.Add(resp.Usage)
		lastPromptTokens = resp.Usage.PromptTokens
		msg := resp.Message
		msg.Role = provider.RoleAssistant
		a.msgs = append(a.msgs, msg)
		model := resp.Model
		if model == "" {
			model = a.cfg.Model
		}
		a.emit(Step{Kind: StepAssistant, Content: msg.Content, Usage: resp.Usage, Model: model})
		clean, submitted, submission := a.handleAssistant(runCtx, msg)
		if submitted {
			return a.finish(ExitSubmitted, submission, ""), nil
		}
		if clean {
			formatErrors = 0
		} else {
			formatErrors++
			if formatErrors >= a.cfg.MaxFormatErrors {
				return a.finish(ExitFormatError, "", ""), nil
			}
		}
	}
}

func (a *Agent) Messages() []provider.Message { return cloneMessages(a.msgs) }

func (a *Agent) Trajectory() Trajectory {
	return Trajectory{
		TrajectoryFormat: "pulse-harness-1",
		Info: TrajectoryInfo{
			ModelStats: ModelStats{
				APICalls:         a.result.Calls,
				PromptTokens:     a.result.Usage.PromptTokens,
				CompletionTokens: a.result.Usage.CompletionTokens,
				Cost:             a.result.Usage.Cost,
			},
			ExitStatus: a.result.ExitStatus,
			Submission: a.result.Submission,
			Config: TrajectoryConfig{
				Model:              a.cfg.Model,
				Provider:           a.cfg.Provider,
				MaxSteps:           a.cfg.MaxSteps,
				StepTimeoutSeconds: int64(a.cfg.StepTimeout.Seconds()),
				Cwd:                a.cfg.Cwd,
			},
		},
		Messages: a.Messages(),
		Steps:    append([]Step(nil), a.steps...),
	}
}

type Trajectory struct {
	TrajectoryFormat string             `json:"trajectory_format"`
	Info             TrajectoryInfo     `json:"info"`
	Messages         []provider.Message `json:"messages"`
	Steps            []Step             `json:"steps"`
}

type TrajectoryInfo struct {
	ModelStats ModelStats       `json:"model_stats"`
	ExitStatus string           `json:"exit_status"`
	Submission string           `json:"submission"`
	Config     TrajectoryConfig `json:"config"`
}

type ModelStats struct {
	APICalls         int     `json:"api_calls"`
	PromptTokens     int     `json:"prompt_tokens"`
	CompletionTokens int     `json:"completion_tokens"`
	Cost             float64 `json:"cost"`
}

type TrajectoryConfig struct {
	Model              string `json:"model"`
	Provider           string `json:"provider"`
	MaxSteps           int    `json:"max_steps"`
	StepTimeoutSeconds int64  `json:"step_timeout_seconds"`
	Cwd                string `json:"cwd"`
}

func WriteTrajectory(path string, t Trajectory) error {
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return err
	}
	data, err := json.MarshalIndent(t, "", "  ")
	if err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, append(data, '\n'), 0600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

const DefaultPromptTemplate = `You are an autonomous software engineering agent working in {{.Info.cwd}} on {{.Info.os}}.
You have exactly one tool: bash. Call it with a JSON object containing a command string.
If you cannot call tools, write exactly one fenced ` + "```bash" + ` or ` + "```sh" + ` block per message.
Each command runs in a fresh non-interactive {{.Info.shell}} in the working directory.
No state survives between commands: cd, exports, aliases, and shell variables are lost.
Chain setup and use with &&, or use absolute paths.
Commands time out after {{.StepTimeout}}.
Keep outputs small with head, tail, grep, and focused test selectors.
Output over {{.OutputLimit}} characters is cut in the middle.
Older command outputs are shortened to their first and last lines as the conversation grows; rerun a command if you need its full output again.
Never run interactive programs: editors, pagers, watch modes, or foreground servers.
To edit files, use heredocs, sed -i, or a small python3 -c script.
Verify your work by running the project's own checks.
On a command failure, inspect the cause, make a small safe correction and rerun the targeted check.
If it still fails, try a different evidence-based approach or use the task's peer-help channel; do not repeat an unchanged failure or poll while waiting for help.
Never bypass a policy refusal or approval boundary, weaken a check, or report unfinished work as completed.
An unfinished task is reported as SUMMARY: blocked: <cause and attempted fixes> or SUMMARY: failed: <cause and attempted fixes>.
When done, or when you cannot continue, reply with no command and a plain-text final report.
The report must include a SUMMARY: line and any FINDING:, SUGGESTION:, QUESTION:, or VOTE: lines the task asked for.
Those key words must start their lines because a program reads the report.
A message with neither a command nor a report wastes a step.
You have at most {{.MaxSteps}} model calls.
The task follows in the next message and its rules apply in full.`

type PromptData struct {
	Cwd         string
	Model       string
	Provider    string
	StepTimeout string
	OutputLimit int
	MaxSteps    int
	Info        map[string]string
}

func RenderPrompt(tmpl string, data PromptData) (string, error) {
	data = normalizePromptData(data)
	parsed, err := template.New("prompt").Option("missingkey=error").Parse(tmpl)
	if err != nil {
		return "", err
	}
	var out bytes.Buffer
	if err := parsed.Execute(&out, data); err != nil {
		return "", err
	}
	return out.String(), nil
}

func normalizePromptData(data PromptData) PromptData {
	if data.Info == nil {
		data.Info = map[string]string{}
	}
	cp := map[string]string{}
	for k, v := range data.Info {
		cp[k] = v
	}
	if cp["cwd"] == "" {
		cp["cwd"] = data.Cwd
	}
	if cp["cwd"] == "" {
		cp["cwd"] = "."
	}
	if data.Cwd == "" {
		data.Cwd = cp["cwd"]
	}
	if cp["os"] == "" {
		cp["os"] = "this system"
	}
	if cp["shell"] == "" {
		cp["shell"] = "/bin/sh"
	}
	data.Info = cp
	return data
}

func promptData(info map[string]string, cfg Config) PromptData {
	cfg = applyDefaults(cfg)
	cp := map[string]string{}
	for k, v := range info {
		cp[k] = v
	}
	cwd := cp["cwd"]
	if cwd == "" {
		cwd = cfg.Cwd
	}
	if cwd == "" {
		cwd = "."
	}
	cp["cwd"] = cwd
	if cp["os"] == "" {
		cp["os"] = "this system"
	}
	if cp["shell"] == "" {
		cp["shell"] = "/bin/sh"
	}
	return normalizePromptData(PromptData{
		Cwd: cwd, Model: cfg.Model, Provider: cfg.Provider, StepTimeout: cfg.StepTimeout.String(),
		OutputLimit: cfg.OutputLimit, MaxSteps: cfg.MaxSteps, Info: cp,
	})
}

func DefaultSystemPrompt(info map[string]string, cfg Config) string {
	prompt, err := RenderPrompt(DefaultPromptTemplate, promptData(info, cfg))
	if err != nil {
		return ""
	}
	return prompt
}

func applyDefaults(cfg Config) Config {
	if cfg.MaxSteps == 0 {
		cfg.MaxSteps = DefaultMaxSteps
	}
	if cfg.StepTimeout == 0 {
		cfg.StepTimeout = DefaultStepTimeout
	}
	if cfg.OutputLimit == 0 {
		cfg.OutputLimit = DefaultOutputLimit
	}
	if cfg.KeepObservations == 0 {
		cfg.KeepObservations = DefaultKeepObservations
	}
	if cfg.MaxFormatErrors == 0 {
		cfg.MaxFormatErrors = DefaultMaxFormatErrors
	}
	return cfg
}

func (a *Agent) compileDeny() error {
	patterns := append([]string{}, DefaultDeny...)
	patterns = append(patterns, a.cfg.Deny...)
	a.deny = make([]denyRule, 0, len(patterns))
	for _, p := range patterns {
		re, err := regexp.Compile(p)
		if err != nil {
			return fmt.Errorf("compile deny regexp %q: %w", p, err)
		}
		a.deny = append(a.deny, denyRule{raw: p, description: denyDescription(p), re: re})
	}
	return nil
}

func (a *Agent) limitStatus(ctx context.Context) string {
	if a.cfg.CancelFile != "" {
		if _, err := os.Stat(a.cfg.CancelFile); err == nil {
			a.cancelReason = ErrOwnerCancelled.Error()
			return ExitCancelled
		}
	}
	if err := ctx.Err(); err != nil {
		if errors.Is(context.Cause(ctx), ErrOwnerCancelled) {
			a.cancelReason = ErrOwnerCancelled.Error()
		}
		return ExitCancelled
	}
	if a.result.Calls >= a.cfg.MaxSteps {
		return ExitStepLimit
	}
	if a.cfg.WallTime > 0 && time.Since(a.started) >= a.cfg.WallTime {
		return ExitTimeLimit
	}
	if a.cfg.MaxCost > 0 && a.result.Usage.Cost >= a.cfg.MaxCost {
		return ExitCostLimit
	}
	return ""
}

func (a *Agent) limitError(status string) string {
	switch status {
	case ExitStepLimit:
		return fmt.Sprintf("stopped at the step budget: %d model calls", a.cfg.MaxSteps)
	case ExitTimeLimit:
		return fmt.Sprintf("stopped at the time budget: %dm", int(a.cfg.WallTime.Round(time.Minute).Minutes()))
	case ExitCostLimit:
		return fmt.Sprintf("stopped at the cost budget: %.2f of %s (provider units)", a.result.Usage.Cost, strconv.FormatFloat(a.cfg.MaxCost, 'f', -1, 64))
	case ExitCancelled:
		return a.cancelReason
	default:
		return ""
	}
}

func (a *Agent) handleAssistant(ctx context.Context, msg provider.Message) (clean, submitted bool, submission string) {
	if len(msg.ToolCalls) > 0 {
		allClean := true
		for _, call := range msg.ToolCalls {
			if call.Name != "bash" {
				text := fmt.Sprintf("unknown tool %s; only bash exists", call.Name)
				a.msgs = append(a.msgs, provider.Message{Role: provider.RoleTool, ToolCallID: call.ID, Name: call.Name, Content: text})
				a.emit(Step{Kind: StepError, Content: text})
				allClean = false
				continue
			}
			command, ok := parseCommand(call.Arguments)
			if !ok {
				text := "tool arguments must be a JSON object with a string command"
				a.msgs = append(a.msgs, provider.Message{Role: provider.RoleTool, ToolCallID: call.ID, Name: "bash", Content: text})
				a.emit(Step{Kind: StepError, Content: text})
				allClean = false
				continue
			}
			obsText, obs, submit, sub := a.runCommand(ctx, command)
			a.msgs = append(a.msgs, provider.Message{Role: provider.RoleTool, ToolCallID: call.ID, Name: "bash", Content: obsText})
			if submit {
				return true, true, sub
			}
			_ = obs
		}
		return allClean, false, ""
	}
	blocks := bashBlocks(msg.Content)
	if len(blocks) == 1 {
		obsText, _, submit, sub := a.runCommand(ctx, blocks[0])
		a.msgs = append(a.msgs, provider.Message{Role: provider.RoleUser, Content: obsText})
		if submit {
			return true, true, sub
		}
		return true, false, ""
	}
	if len(blocks) > 1 {
		text := "write exactly one ```bash block per message, or your final report with no command"
		a.msgs = append(a.msgs, provider.Message{Role: provider.RoleUser, Content: text})
		a.emit(Step{Kind: StepError, Content: text})
		return false, false, ""
	}
	if isReport(msg.Content) {
		return true, true, msg.Content
	}
	text := "Your message had no command and no report. Either call the bash tool (or write one ```bash block) to keep working, or finish with your report: a `SUMMARY:` line plus any `FINDING:`/`SUGGESTION:`/`QUESTION:`/`VOTE:` lines."
	a.msgs = append(a.msgs, provider.Message{Role: provider.RoleUser, Content: text})
	a.emit(Step{Kind: StepError, Content: text})
	return false, false, ""
}

func parseCommand(args string) (string, bool) {
	var obj struct {
		Command string `json:"command"`
	}
	if err := json.Unmarshal([]byte(args), &obj); err != nil {
		return "", false
	}
	if obj.Command == "" {
		return "", false
	}
	return obj.Command, true
}

var bashBlockRe = regexp.MustCompile("(?s)```(?:bash|sh)\\s*\\n(.*?)\\n```")

func bashBlocks(text string) []string {
	matches := bashBlockRe.FindAllStringSubmatch(text, -1)
	out := make([]string, 0, len(matches))
	for _, m := range matches {
		out = append(out, strings.TrimSpace(m[1]))
	}
	return out
}

var reportLineRe = regexp.MustCompile("(?m)^\\s*-?\\s*`?(SUMMARY|QUESTION|FINDING|SUGGESTION|VOTE)`?:")

func isReport(text string) bool {
	return strings.Contains(text, "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT") || reportLineRe.MatchString(text)
}

func (a *Agent) runCommand(ctx context.Context, command string) (string, Observation, bool, string) {
	a.result.Steps++
	a.emit(Step{Kind: StepCommand, Content: command, Command: command})
	if rule, denied := a.denied(command); denied {
		obs := Observation{Output: "refused by policy: " + rule.description + ". Never push, never add remotes, never call gh, never touch credentials (job rule 3).", ExitCode: 126}
		a.emit(Step{Kind: StepError, Content: obs.Output, Command: command})
		a.emitObservation(command, obs)
		return formatObservation(obs, a.cfg.OutputLimit), obs, false, ""
	}
	obs := a.env.Execute(ctx, command, a.cfg.StepTimeout)
	a.emitObservation(command, obs)
	formatted := formatObservation(obs, a.cfg.OutputLimit)
	if obs.ExitCode == 0 && !obs.TimedOut {
		if sub, ok := submissionFromObservation(obs.Output); ok {
			return formatted, obs, true, sub
		}
	}
	return formatted, obs, false, ""
}

func (a *Agent) denied(command string) (denyRule, bool) {
	for _, rule := range a.deny {
		if rule.re.MatchString(command) {
			return rule, true
		}
	}
	return denyRule{}, false
}

func denyDescription(pattern string) string {
	switch pattern {
	case DefaultDeny[0]:
		return "git push is disabled"
	case DefaultDeny[1]:
		return "git remote add/set-url/remove is disabled"
	case DefaultDeny[2]:
		return "gh commands are disabled"
	case DefaultDeny[3]:
		return "credential and private paths are disabled"
	default:
		return pattern
	}
}

func (a *Agent) emitObservation(command string, obs Observation) {
	code := obs.ExitCode
	a.emit(Step{
		Kind:       StepObservation,
		Content:    obs.Output,
		Command:    command,
		ExitCode:   &code,
		DurationMS: obs.Duration.Milliseconds(),
	})
}

func formatObservation(obs Observation, limit int) string {
	out := truncate(obs.Output, limit)
	if obs.TimedOut {
		secs := int(obs.Duration.Round(time.Second).Seconds())
		if secs <= 0 {
			secs = 1
		}
		line := fmt.Sprintf("The command timed out after %ds and was killed; inspect partial output and retry only with a smaller scope or a different safe approach, within the run's remaining budget. Do not leave unsupervised background work.", secs)
		if out == "" {
			out = line
		} else {
			out += "\n" + line
		}
	}
	return fmt.Sprintf("<returncode>%d</returncode>\n<output>\n%s\n</output>", obs.ExitCode, out)
}

func truncate(s string, limit int) string {
	if limit <= 0 || len(s) <= limit {
		return s
	}
	half := limit / 2
	omitted := len(s) - half - half
	return s[:half] + fmt.Sprintf("\n[... %d characters omitted ...]\n", omitted) + s[len(s)-half:]
}

func submissionFromObservation(output string) (string, bool) {
	output = strings.TrimLeft(output, "\r\n")
	line, rest, found := strings.Cut(output, "\n")
	line = strings.TrimSuffix(line, "\r")
	if line != "COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT" {
		return "", false
	}
	if !found {
		return "", true
	}
	return strings.TrimLeft(rest, "\r\n"), true
}

func (a *Agent) finish(status, submission, errText string) Result {
	a.result.ExitStatus = status
	a.result.Submission = submission
	a.result.Error = errText
	a.result.Elapsed = time.Since(a.started)
	a.emit(Step{Kind: StepExit, Content: status})
	return a.result
}

func (a *Agent) emit(s Step) {
	s.N = a.nextN
	a.nextN++
	s.At = time.Now()
	a.steps = append(a.steps, s)
	for _, sink := range a.sinks {
		if sink == nil {
			continue
		}
		func() {
			defer func() { _ = recover() }()
			sink.Step(s)
		}()
	}
}

func cloneMessages(in []provider.Message) []provider.Message {
	out := append([]provider.Message(nil), in...)
	for i := range out {
		out[i].ToolCalls = append([]provider.ToolCall(nil), out[i].ToolCalls...)
	}
	return out
}

func oneLine(s string, limit int) string {
	s = strings.Join(strings.Fields(s), " ")
	if len(s) > limit {
		return s[:limit]
	}
	return s
}
