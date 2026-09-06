package bus

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

func unmarshal(text string, v any) error { return json.Unmarshal([]byte(text), v) }

// DefaultInterval is how often Tail polls when interval <= 0.
const DefaultInterval = time.Second

// Emit appends an event to the store exactly like store.Event and publishes
// it on Default(). In-process writers (board, tasks, records, runner) call
// Emit so subscribers in the same process - the hook runner, `pulse watch`
// inside the daemon, the hub's SSE route - see the event immediately.
// Writers that keep using store.Event (or store helpers whose transactions
// log events privately, like SetSetting) still reach in-process subscribers
// once a Bridge runs; Emit and Bridge dedup on the event id, so calling both
// never publishes twice. detail follows store.Event: a string is stored as
// is, nil is empty, any other value is encoded like Python's
// json.dumps(sort_keys=True).
func Emit(s *store.Store, actor, action, subject string, detail any) (Event, error) {
	return EmitOn(Default(), s, actor, action, subject, detail)
}

// EmitOn is Emit publishing on a chosen bus (tests, embedded runners).
func EmitOn(b *Bus, s *store.Store, actor, action, subject string, detail any) (Event, error) {
	text, err := DetailText(detail)
	if err != nil {
		return Event{}, err
	}
	e := Event{At: s.Now(), Actor: actor, Action: action, Subject: subject, Detail: text}
	tx, err := s.DB.Begin()
	if err != nil {
		return Event{}, err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
		e.At, e.Actor, e.Action, e.Subject, e.Detail); err != nil {
		return Event{}, fmt.Errorf("event %s: %w", action, err)
	}
	if err := tx.QueryRow("SELECT last_insert_rowid()").Scan(&e.ID); err != nil {
		return Event{}, err
	}
	if err := tx.Commit(); err != nil {
		return Event{}, err
	}
	if b != nil {
		b.PublishOnce(e)
	}
	return e, nil
}

// EmitTx writes the event inside a caller's transaction and returns it; the
// caller publishes with PublishAfter once the transaction committed (an event
// must never reach subscribers before its row is durable).
func EmitTx(tx *sql.Tx, now, actor, action, subject string, detail any) (Event, error) {
	text, err := DetailText(detail)
	if err != nil {
		return Event{}, err
	}
	e := Event{At: now, Actor: actor, Action: action, Subject: subject, Detail: text}
	if _, err := tx.Exec("INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
		e.At, e.Actor, e.Action, e.Subject, e.Detail); err != nil {
		return Event{}, fmt.Errorf("event %s: %w", action, err)
	}
	if err := tx.QueryRow("SELECT last_insert_rowid()").Scan(&e.ID); err != nil {
		return Event{}, err
	}
	return e, nil
}

// PublishAfter publishes events written with EmitTx on Default().
func PublishAfter(events ...Event) {
	for _, e := range events {
		Default().PublishOnce(e)
	}
}

// Bridge feeds every event committed to the store from now on into
// Default(), polling like Tail (interval <= 0 means DefaultInterval). A
// long-lived process (the daemon, the hub) runs one Bridge so writes that
// bypass Emit - store.Event, store.SetSetting, records helpers, other
// processes' CLIs - reach its in-process subscribers too; PublishOnce keeps
// events Emit already published from arriving twice. The returned channel
// closes when ctx ends and the bridge has stopped.
func Bridge(ctx context.Context, s *store.Store, interval time.Duration) <-chan struct{} {
	return BridgeOn(ctx, Default(), s, interval)
}

// BridgeOn is Bridge on a chosen bus.
func BridgeOn(ctx context.Context, b *Bus, s *store.Store, interval time.Duration) <-chan struct{} {
	done := make(chan struct{})
	events := Tail(ctx, s, -1, Filter{}, interval)
	go func() {
		defer close(done)
		for e := range events {
			b.PublishOnce(e)
		}
	}()
	return done
}

// DetailText encodes a detail the way store.Event does.
func DetailText(detail any) (string, error) {
	switch d := detail.(type) {
	case nil:
		return "", nil
	case string:
		return d, nil
	case []byte:
		return string(d), nil
	}
	return store.PyJSON(detail)
}

// LastID is the highest event seq in the store (0 when empty).
func LastID(s *store.Store) (int64, error) {
	var id sql.NullInt64
	if err := s.DB.QueryRow("SELECT MAX(seq) FROM events").Scan(&id); err != nil {
		return 0, err
	}
	return id.Int64, nil
}

// Since returns the events after sinceID that pass filter, oldest first;
// limit <= 0 means every one.
func Since(s *store.Store, sinceID int64, filter Filter, limit int) ([]Event, error) {
	return since(context.Background(), s.DB, sinceID, filter, limit)
}

type querier interface {
	QueryContext(ctx context.Context, query string, args ...any) (*sql.Rows, error)
}

func since(ctx context.Context, q querier, sinceID int64, filter Filter, limit int) ([]Event, error) {
	rows, err := q.QueryContext(ctx, "SELECT seq, at, actor, action, subject, detail FROM events WHERE seq > ? ORDER BY seq", sinceID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Event
	for rows.Next() {
		var e Event
		if err := rows.Scan(&e.ID, &e.At, &e.Actor, &e.Action, &e.Subject, &e.Detail); err != nil {
			return nil, err
		}
		if !filter.Match(e) {
			continue
		}
		out = append(out, e)
		if limit > 0 && len(out) >= limit {
			break
		}
	}
	return out, rows.Err()
}

// Tail streams events with seq > sinceID from the store until ctx ends,
// polling every interval (DefaultInterval when <= 0). It holds one dedicated
// connection and re-reads only when that connection's `PRAGMA data_version`
// changed, i.e. when another connection - the daemon, a CLI in another
// process - committed. sinceID < 0 means "from now": only new events.
// The channel is closed when ctx is done.
func Tail(ctx context.Context, s *store.Store, sinceID int64, filter Filter, interval time.Duration) <-chan Event {
	if interval <= 0 {
		interval = DefaultInterval
	}
	out := make(chan Event, 64)
	go func() {
		defer close(out)
		conn, err := s.DB.Conn(ctx)
		if err != nil {
			return
		}
		defer conn.Close()
		last := sinceID
		if last < 0 {
			var id sql.NullInt64
			if err := conn.QueryRowContext(ctx, "SELECT MAX(seq) FROM events").Scan(&id); err != nil {
				return
			}
			last = id.Int64
		}
		var version int64 = -1
		ticker := time.NewTicker(interval)
		defer ticker.Stop()
		for {
			var v int64
			if err := conn.QueryRowContext(ctx, "PRAGMA data_version").Scan(&v); err != nil {
				return
			}
			if v != version {
				version = v
				events, err := since(ctx, conn, last, Filter{}, 0)
				if err != nil {
					return
				}
				for _, e := range events {
					last = e.ID
					if !filter.Match(e) {
						continue
					}
					select {
					case out <- e:
					case <-ctx.Done():
						return
					}
				}
			}
			select {
			case <-ticker.C:
			case <-ctx.Done():
				return
			}
		}
	}()
	return out
}

// The store's canonical helpers (SetSetting, PutRecord, Tx.Event, store.Event)
// log their events privately; store.OnEvent hands each durable row to the
// default bus so in-process subscribers see them at once. PublishOnce makes
// this safe next to Emit and Bridge: an id is delivered a single time.
func init() {
	store.OnEvent(func(r store.EventRecord) {
		Default().PublishOnce(Event{ID: r.Seq, At: r.At, Actor: r.Actor, Action: r.Action, Subject: r.Subject, Detail: r.Detail})
	})
}
