// Package hooks reacts to store events (D-E): rows in the hooks table name a
// script or a webhook for an action glob, the Runner fires them from the bus,
// and `pulse hooks install` writes the harness-side hook config that makes
// interactive Copilot CLI / Claude Code sessions read the board on start and
// reflect before they stop.
package hooks

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Hook is one row of the hooks table.
type Hook struct {
	ID          int64  `json:"id"`
	Event       string `json:"event"`
	Kind        string `json:"kind"`
	Target      string `json:"target"`
	Project     string `json:"project"`
	Enabled     bool   `json:"enabled"`
	CreatedBy   string `json:"created_by"`
	CreatedAt   string `json:"created_at"`
	LastFiredAt string `json:"last_fired_at"`
	LastStatus  string `json:"last_status"`
	Fires       int    `json:"fires"`
}

// Timeouts for one firing.
const (
	ScriptTimeout  = 30 * time.Second
	WebhookTimeout = 10 * time.Second
)

const hookCols = "id, event, kind, target, project, enabled, created_by, created_at, last_fired_at, last_status, fires"

func scanHook(row interface{ Scan(...any) error }) (*Hook, error) {
	var h Hook
	var enabled int
	if err := row.Scan(&h.ID, &h.Event, &h.Kind, &h.Target, &h.Project, &enabled, &h.CreatedBy, &h.CreatedAt, &h.LastFiredAt, &h.LastStatus, &h.Fires); err != nil {
		return nil, err
	}
	h.Enabled = enabled != 0
	return &h, nil
}

// Add registers a hook. kind is "script" (target: an executable path) or
// "webhook" (target: an http(s) URL); project "" means every project.
func Add(s *store.Store, event, kind, target, project, actor string) (*Hook, error) {
	event = strings.TrimSpace(event)
	if event == "" {
		return nil, cli.Errorf(2, "An event glob is needed", "hooks fire on actions such as post, task.*, compute.approved or *", "pulse hook add 'task.*' --script PATH")
	}
	if _, err := path.Match(event, "x"); err != nil {
		return nil, cli.Errorf(2, "Bad event glob "+strconv.Quote(event), err.Error(), "use * and ? only, e.g. task.* or post")
	}
	switch kind {
	case "script":
		if target == "" {
			return nil, cli.Errorf(2, "A script path is needed", "--script names the executable the hook runs", "pulse hook add EVENT --script /path/to/hook.sh")
		}
		if st, err := os.Stat(target); err != nil || st.IsDir() {
			return nil, cli.Errorf(2, "No script at "+target, "the path must exist when the hook is added", "chmod +x the script and pass its absolute path")
		}
	case "webhook":
		if !strings.HasPrefix(target, "http://") && !strings.HasPrefix(target, "https://") {
			return nil, cli.Errorf(2, "A webhook needs an http(s) URL", "got "+strconv.Quote(target), "pulse hook add EVENT --url https://example.test/hook")
		}
	default:
		return nil, cli.Errorf(2, "A hook is a --script PATH or a --url URL", "pass exactly one", "pulse help hook")
	}
	if actor == "" {
		actor = store.HumanAgent
	}
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	res, err := tx.Exec("INSERT INTO hooks (event, kind, target, project, enabled, created_by, created_at) VALUES (?, ?, ?, ?, 1, ?, ?)",
		event, kind, target, project, actor, now)
	if err != nil {
		return nil, err
	}
	id, _ := res.LastInsertId()
	ev, err := bus.EmitTx(tx, now, actor, "hook.added", strconv.FormatInt(id, 10), map[string]any{"event": event, "kind": kind, "target": target, "project": project})
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(ev)
	return Get(s, id)
}

// Get returns one hook.
func Get(s *store.Store, id int64) (*Hook, error) {
	h, err := scanHook(s.DB.QueryRow("SELECT "+hookCols+" FROM hooks WHERE id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, cli.Errorf(2, fmt.Sprintf("No hook %d", id), "the id is not in this store", "pulse hook list")
	}
	return h, err
}

// List returns hooks by id; enabledOnly drops removed ones.
func List(s *store.Store, enabledOnly bool) ([]Hook, error) {
	q := "SELECT " + hookCols + " FROM hooks"
	if enabledOnly {
		q += " WHERE enabled = 1"
	}
	rows, err := s.DB.Query(q + " ORDER BY id")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Hook
	for rows.Next() {
		h, err := scanHook(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, *h)
	}
	return out, rows.Err()
}

// SetEnabled is `hook remove` (false: rows are never deleted) and `hook enable`.
func SetEnabled(s *store.Store, id int64, enabled bool, actor string) (*Hook, error) {
	if _, err := Get(s, id); err != nil {
		return nil, err
	}
	if actor == "" {
		actor = store.HumanAgent
	}
	action, flag := "hook.removed", 0
	if enabled {
		action, flag = "hook.enabled", 1
	}
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("UPDATE hooks SET enabled = ? WHERE id = ?", flag, id); err != nil {
		return nil, err
	}
	ev, err := bus.EmitTx(tx, now, actor, action, strconv.FormatInt(id, 10), nil)
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(ev)
	return Get(s, id)
}

// Matches reports whether h fires for e: enabled, the action matches the
// glob, and the hook's project (when set) equals the event's.
func (h Hook) Matches(e bus.Event) bool {
	if !h.Enabled || !bus.Glob(h.Event, e.Action) {
		return false
	}
	if h.Project != "" && e.Project() != h.Project && e.Subject != h.Project {
		return false
	}
	return true
}

// Payload is the JSON a hook receives (stdin for scripts, body for webhooks).
type Payload struct {
	Hook    int64  `json:"hook"`
	ID      int64  `json:"id"`
	At      string `json:"at"`
	Actor   string `json:"actor"`
	Action  string `json:"action"`
	Subject string `json:"subject"`
	Detail  string `json:"detail"`
	Project string `json:"project"`
}

// Fire runs one hook for one event and records the outcome on the row
// (last_fired_at, last_status, fires). Scripts get PULSE_EVENT_ID/AT/ACTOR/
// ACTION/SUBJECT/DETAIL and PULSE_HOOK_ID in the environment plus the JSON
// payload on stdin, 30 s; webhooks get the payload as a POST body, 10 s.
// The event is passed through bus.Redactor first (bus.PrivateSettings).
func Fire(ctx context.Context, s *store.Store, h Hook, e bus.Event) (string, error) {
	// hooks are agent-facing: owner-private setting values are masked first
	status, err := fire(ctx, s.Home, h, bus.Redactor(s)(e))
	if err != nil {
		status = "error: " + err.Error()
	}
	if len(status) > 200 {
		status = status[:200]
	}
	if _, dbErr := s.DB.Exec("UPDATE hooks SET last_fired_at = ?, last_status = ?, fires = fires + 1 WHERE id = ?", s.Now(), status, h.ID); dbErr != nil && err == nil {
		err = dbErr
	}
	return status, err
}

func fire(ctx context.Context, home string, h Hook, e bus.Event) (string, error) {
	body, _ := json.Marshal(Payload{Hook: h.ID, ID: e.ID, At: e.At, Actor: e.Actor, Action: e.Action, Subject: e.Subject, Detail: e.Detail, Project: e.Project()})
	switch h.Kind {
	case "script":
		ctx, cancel := context.WithTimeout(ctx, ScriptTimeout)
		defer cancel()
		cmd := exec.CommandContext(ctx, h.Target)
		cmd.Env = append(os.Environ(),
			"PULSE_HOOK_ID="+strconv.FormatInt(h.ID, 10),
			"PULSE_EVENT_ID="+strconv.FormatInt(e.ID, 10),
			"PULSE_EVENT_AT="+e.At,
			"PULSE_EVENT_ACTOR="+e.Actor,
			"PULSE_EVENT_ACTION="+e.Action,
			"PULSE_EVENT_SUBJECT="+e.Subject,
			"PULSE_EVENT_DETAIL="+e.Detail,
			"PULSE_EVENT_PROJECT="+e.Project(),
			"PULSE_HOME="+home,
		)
		cmd.Stdin = bytes.NewReader(body)
		var out bytes.Buffer
		cmd.Stdout = &out
		cmd.Stderr = &out
		err := cmd.Run()
		if ctx.Err() == context.DeadlineExceeded {
			return "", fmt.Errorf("timeout after %s", ScriptTimeout)
		}
		if err != nil {
			return "", fmt.Errorf("%v: %s", err, strings.TrimSpace(out.String()))
		}
		return "ok", nil
	case "webhook":
		ctx, cancel := context.WithTimeout(ctx, WebhookTimeout)
		defer cancel()
		req, err := http.NewRequestWithContext(ctx, http.MethodPost, h.Target, bytes.NewReader(body))
		if err != nil {
			return "", err
		}
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("User-Agent", "pulse-hook/1")
		req.Header.Set("X-Pulse-Event", e.Action)
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			return "", err
		}
		defer resp.Body.Close()
		if resp.StatusCode >= 300 {
			return "", fmt.Errorf("HTTP %d", resp.StatusCode)
		}
		return "ok " + strconv.Itoa(resp.StatusCode), nil
	}
	return "", fmt.Errorf("unknown hook kind %q", h.Kind)
}

// Runner subscribes to a bus and fires the matching enabled hooks for every
// event, each in its own goroutine so the bus never waits on a script or a
// server. The daemon runs one; tests run one against their own bus.
type Runner struct {
	Bus   *bus.Bus
	Store *store.Store
	// Log receives one line per firing when set.
	Log func(format string, a ...any)

	wg sync.WaitGroup
}

// Run consumes events until ctx ends and returns after every firing it
// started has finished.
func (r *Runner) Run(ctx context.Context) {
	b := r.Bus
	if b == nil {
		b = bus.Default()
	}
	ch, cancel := b.Subscribe(bus.Filter{}, 256)
	defer cancel()
	for {
		select {
		case <-ctx.Done():
			r.wg.Wait()
			return
		case e, ok := <-ch:
			if !ok {
				r.wg.Wait()
				return
			}
			if strings.HasPrefix(e.Action, "hook.") {
				continue // a hook that fires on its own bookkeeping would loop
			}
			r.dispatch(ctx, e)
		}
	}
}

func (r *Runner) dispatch(ctx context.Context, e bus.Event) {
	hooks, err := List(r.Store, true)
	if err != nil {
		r.logf("hooks: %v", err)
		return
	}
	for _, h := range hooks {
		if !h.Matches(e) {
			continue
		}
		r.wg.Add(1)
		go func(h Hook) {
			defer r.wg.Done()
			status, err := Fire(ctx, r.Store, h, e)
			if err != nil {
				r.logf("hook %d (%s %s) on %s #%d: %v", h.ID, h.Kind, h.Target, e.Action, e.ID, err)
				return
			}
			r.logf("hook %d (%s %s) on %s #%d: %s", h.ID, h.Kind, h.Target, e.Action, e.ID, status)
		}(h)
	}
}

// Wait blocks until the firings in flight are done (tests).
func (r *Runner) Wait() { r.wg.Wait() }

func (r *Runner) logf(format string, a ...any) {
	if r.Log != nil {
		r.Log(format, a...)
	}
}
