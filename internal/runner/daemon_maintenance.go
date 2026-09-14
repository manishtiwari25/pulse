package runner

import (
	"context"
	"fmt"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// One lane owns landing and its following sweep. Dispatch continues while a
// check runs; another landing pass and the self-update phase wait for that check.
type daemonMaintenance struct {
	done      chan struct{}
	lines     []string
	held      map[string]string
	lastSweep time.Time
	pending   map[landingRevision]bool
}

type landingRevision struct {
	job, branch string
	generation  int
}

func landingQueueState(s *store.Store, projects []row) map[landingRevision]bool {
	state := map[landingRevision]bool{}
	for _, project := range projects {
		for _, job := range pendingLandings(s.DB, project.str("slug")) {
			state[landingRevision{job.str("id"), job.str("branch"), job.int("generation")}] = true
		}
	}
	return state
}

func (m *daemonMaintenance) hasNewPending(s *store.Store, projects []row) bool {
	for revision := range landingQueueState(s, projects) {
		if !m.pending[revision] {
			return true
		}
	}
	return false
}

func (m *daemonMaintenance) start(ctx context.Context, s *store.Store, projects []row, actor, now string, dry bool, inflight map[string]bool, binary selfUpdateEnv) {
	if m.done != nil {
		return
	}
	m.pending = landingQueueState(s, projects)
	m.done = make(chan struct{})
	go func() {
		defer close(m.done)
		defer func() {
			if r := recover(); r != nil {
				m.lines = append(m.lines, fmt.Sprintf("daemon maintenance error: %v", r))
			}
		}()
		m.lines, m.held = tendLandingsContext(ctx, s, projects, actor, now, m.held)
		if !dry && ctx.Err() == nil && runnerStopped(s) == "" {
			m.lines = append(m.lines, tendSweep(s, projects, s.Now(), actor, inflight, &m.lastSweep)...)
			if ctx.Err() == nil && runnerStopped(s) == "" {
				m.lines = append(m.lines, tendBinary(s, s.Now(), binary)...)
			}
		}
	}()
}

func (m *daemonMaintenance) drain() (lines []string) {
	if m.done == nil {
		return nil
	}
	select {
	case <-m.done:
		lines, m.lines, m.done = m.lines, nil, nil
		return lines
	default:
		return nil
	}
}
