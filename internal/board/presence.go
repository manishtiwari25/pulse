package board

import (
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

const (
	HeartbeatInterval = time.Minute
	HeartbeatGrace    = 5 * time.Minute
	heartbeatPrefix   = "agent-heartbeat:"
)

// ExecutionIdentity fences observations to one supervised run, not a name,
// recent post, or renewable lease alone. Runner sessions are their run IDs.
type ExecutionIdentity struct {
	Agent      string `json:"agent"`
	Session    string `json:"session"`
	RunID      string `json:"run_id"`
	JobID      string `json:"job_id"`
	Generation int    `json:"generation"`
	Executor   string `json:"executor"`
}

func (e ExecutionIdentity) Validate() error {
	if !ValidName(e.Agent) || e.Session == "" || e.Session != e.RunID || e.JobID == "" ||
		e.Executor == "" || e.Generation < 1 || len(e.RunID) > 160 || len(e.JobID) > 160 || len(e.Executor) > 160 {
		return fmt.Errorf("heartbeat needs a valid agent, run/session, job, generation and executor")
	}
	return nil
}

// ExecutionHeartbeat is a replaceable snapshot, never useful output or an
// event. A stop retains the last actual observation rather than inventing one.
type ExecutionHeartbeat struct {
	ExecutionIdentity
	At        string `json:"at"`
	StoppedAt string `json:"stopped_at,omitempty"`
}

func (h ExecutionHeartbeat) Validate() error {
	if err := h.ExecutionIdentity.Validate(); err != nil {
		return err
	}
	at, err := time.Parse(time.RFC3339Nano, h.At)
	if err != nil || at.IsZero() {
		return fmt.Errorf("heartbeat has no valid observation time")
	}
	if h.StoppedAt != "" {
		stop, err := time.Parse(time.RFC3339Nano, h.StoppedAt)
		if err != nil || stop.Before(at) {
			return fmt.Errorf("heartbeat has no valid stop time")
		}
	}
	return nil
}

// RecordHeartbeat only replaces the latest snapshot of the current execution.
// The transaction shares the runner's existing run/lease fencing, without
// renewing, releasing or otherwise changing a lease.
func RecordHeartbeat(s *store.Store, h ExecutionHeartbeat) (bool, error) {
	if err := h.Validate(); err != nil {
		return false, err
	}
	tx, err := s.DB.Begin()
	if err != nil {
		return false, err
	}
	defer tx.Rollback()
	changed, err := recordHeartbeat(tx, h)
	if err != nil || !changed {
		return false, err
	}
	return true, tx.Commit()
}

func recordHeartbeat(tx *sql.Tx, h ExecutionHeartbeat) (bool, error) {
	var current int
	err := tx.QueryRow(`SELECT 1 FROM runs r JOIN jobs j ON j.id=r.job_id JOIN agents a ON a.name=r.agent
		WHERE r.id=? AND r.agent=? AND r.job_id=? AND r.executor=? AND r.status='running' AND r.mode!='dry-run'
		AND a.status!='pruned' AND a.kind NOT IN ('legacy','human')
		AND j.lease_agent=r.agent AND j.generation=? AND j.status IN ('leased','done','failed')
		AND r.rowid=(SELECT r2.rowid FROM runs r2 WHERE r2.agent=r.agent AND r2.mode!='dry-run'
			ORDER BY r2.started_at DESC,r2.rowid DESC LIMIT 1)`,
		h.RunID, h.Agent, h.JobID, h.Executor, h.Generation).Scan(&current)
	if errors.Is(err, sql.ErrNoRows) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	var raw string
	err = tx.QueryRow("SELECT value FROM meta WHERE key=?", heartbeatPrefix+h.Agent).Scan(&raw)
	if err != nil && !errors.Is(err, sql.ErrNoRows) {
		return false, err
	}
	var prev ExecutionHeartbeat
	if json.Unmarshal([]byte(raw), &prev) == nil && prev.Validate() == nil && prev.ExecutionIdentity == h.ExecutionIdentity {
		before, _ := time.Parse(time.RFC3339Nano, prev.At)
		at, _ := time.Parse(time.RFC3339Nano, h.At)
		if prev.StoppedAt != "" || at.Before(before) || (at.Equal(before) && h.StoppedAt == "") {
			return false, nil
		}
	}
	data, err := json.Marshal(h)
	if err != nil {
		return false, err
	}
	if _, err := tx.Exec("INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
		heartbeatPrefix+h.Agent, string(data)); err != nil {
		return false, err
	}
	return true, nil
}

// StopHeartbeat cannot create evidence for a failed launch or stop a newer run.
func StopHeartbeat(s *store.Store, e ExecutionIdentity, at time.Time) (bool, error) {
	if err := e.Validate(); err != nil {
		return false, err
	}
	tx, err := s.DB.Begin()
	if err != nil {
		return false, err
	}
	defer tx.Rollback()
	var raw string
	err = tx.QueryRow("SELECT value FROM meta WHERE key=?", heartbeatPrefix+e.Agent).Scan(&raw)
	if errors.Is(err, sql.ErrNoRows) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	var h ExecutionHeartbeat
	if err := json.Unmarshal([]byte(raw), &h); err != nil {
		return false, fmt.Errorf("unreadable heartbeat: %w", err)
	}
	if h.ExecutionIdentity != e || h.StoppedAt != "" {
		return false, nil
	}
	h.StoppedAt = at.UTC().Format(time.RFC3339Nano)
	if err := h.Validate(); err != nil {
		return false, err
	}
	changed, err := recordHeartbeat(tx, h)
	if err != nil || !changed {
		return false, err
	}
	return true, tx.Commit()
}

type AgentPresence struct {
	Presence          string `json:"presence"`
	PresenceReason    string `json:"presence_reason"`
	LastHeartbeatAt   string `json:"last_heartbeat_at"`
	PresenceSession   string `json:"presence_session"`
	PresenceCheckedAt string `json:"presence_checked_at"`
	PresenceExpiresAt string `json:"presence_expires_at"`
}

type presenceEvidence struct {
	kind, status, runID, jobID, executor, started, runStatus, jobStatus, leaseAgent, raw string
	generation                                                                           int
	leased                                                                               bool
}

// AgentPresences takes one read snapshot and one clock for every listing.
// All identities remain available, including pruned and pre-heartbeat rows.
func AgentPresences(s *store.Store, now string) (map[string]AgentPresence, error) {
	if now == "" {
		now = s.Now()
	}
	at, err := time.Parse(time.RFC3339Nano, now)
	if err != nil {
		return nil, fmt.Errorf("presence clock: %w", err)
	}
	rows, err := s.DB.Query(`WITH latest_run AS (
		SELECT id, job_id, executor, started_at, status, agent,
			ROW_NUMBER() OVER (PARTITION BY agent ORDER BY started_at DESC, rowid DESC) AS rn
		FROM runs
		WHERE mode != 'dry-run'
	)
	SELECT a.name,a.kind,a.status,
		COALESCE(r.id,''),COALESCE(r.job_id,''),COALESCE(r.executor,''),COALESCE(r.started_at,''),COALESCE(r.status,''),
		COALESCE(j.status,''),COALESCE(j.lease_agent,''),COALESCE(j.generation,0),COALESCE(m.value,''),
		EXISTS(SELECT 1 FROM jobs busy WHERE busy.lease_agent=a.name AND busy.status='leased')
		FROM agents a
		LEFT JOIN latest_run r ON r.agent=a.name AND r.rn=1
		LEFT JOIN jobs j ON j.id=r.job_id
		LEFT JOIN meta m ON m.key=? || a.name`, heartbeatPrefix)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	result := map[string]AgentPresence{}
	for rows.Next() {
		var name string
		var e presenceEvidence
		if err := rows.Scan(&name, &e.kind, &e.status, &e.runID, &e.jobID, &e.executor, &e.started, &e.runStatus,
			&e.jobStatus, &e.leaseAgent, &e.generation, &e.raw, &e.leased); err != nil {
			return nil, err
		}
		result[name] = calculatePresence(name, e, at)
	}
	return result, rows.Err()
}

func calculatePresence(name string, e presenceEvidence, now time.Time) AgentPresence {
	p := AgentPresence{Presence: "unknown", PresenceReason: "no supervised heartbeat", PresenceCheckedAt: now.UTC().Format(time.RFC3339Nano)}
	start, startErr := time.Parse(time.RFC3339Nano, e.started)
	var h ExecutionHeartbeat
	valid := json.Unmarshal([]byte(e.raw), &h) == nil && h.Validate() == nil &&
		h.Agent == name && h.RunID == e.runID && h.JobID == e.jobID && h.Executor == e.executor
	if valid {
		at, _ := time.Parse(time.RFC3339Nano, h.At)
		valid = startErr == nil && !at.Before(start) && !at.After(now)
		if h.StoppedAt != "" {
			stop, _ := time.Parse(time.RFC3339Nano, h.StoppedAt)
			valid = valid && !stop.After(now)
		}
		if valid {
			p.LastHeartbeatAt, p.PresenceSession = h.At, h.Session
		}
	}
	switch {
	case e.status == "pruned":
		p.Presence, p.PresenceReason = "pruned", "identity permanently pruned; history retained"
	case e.kind == "legacy":
		p.Presence, p.PresenceReason = "legacy", "historical identity; no supervised execution"
	case e.kind == "human":
		p.PresenceReason = "owner presence is not monitored"
	case (e.kind == "worker" || e.kind == "lead") && (e.status == "active" || e.status == "idle") && !e.leased && e.runStatus != "running":
		p.Presence, p.PresenceReason = "idle", "reusable; no active run"
	case !valid:
		if e.raw != "" {
			p.PresenceReason = "missing, malformed or historical heartbeat evidence"
		}
		if e.runStatus == "running" && startErr == nil && now.Sub(start) >= HeartbeatGrace {
			p.Presence, p.PresenceReason = "offline", "no valid heartbeat for five minutes"
		}
	case e.runStatus != "running" || h.StoppedAt != "":
		p.Presence, p.PresenceReason = "offline", "execution is no longer supervised"
	case e.leaseAgent != name || e.generation != h.Generation || (e.jobStatus != "leased" && e.jobStatus != "done" && e.jobStatus != "failed"):
		p.Presence, p.PresenceReason = "offline", "execution no longer owns the current job generation"
	default:
		at, _ := time.Parse(time.RFC3339Nano, h.At)
		if now.Sub(at) >= HeartbeatGrace {
			p.Presence, p.PresenceReason = "offline", "no valid heartbeat for five minutes"
		} else {
			p.Presence, p.PresenceReason = "online", "supervised execution"
			p.PresenceExpiresAt = at.Add(HeartbeatGrace).UTC().Format(time.RFC3339Nano)
		}
	}
	return p
}
