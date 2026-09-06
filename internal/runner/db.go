package runner

import (
	"context"
	"database/sql"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// connQ pins one connection so BEGIN IMMEDIATE ... COMMIT runs on it.
type connQ struct {
	c   *sql.Conn
	ctx context.Context
}

func (q connQ) Query(query string, args ...any) (*sql.Rows, error) {
	return q.c.QueryContext(q.ctx, query, args...)
}
func (q connQ) QueryRow(query string, args ...any) *sql.Row {
	return q.c.QueryRowContext(q.ctx, query, args...)
}
func (q connQ) Exec(query string, args ...any) (sql.Result, error) {
	return q.c.ExecContext(q.ctx, query, args...)
}

// immediate runs fn inside BEGIN IMMEDIATE on one pinned connection, retrying
// on SQLITE_BUSY the way sqlite3's busy timeout does for Python (the driver's
// busy_timeout covers the lock wait; this covers the few cases where the
// BEGIN itself races). The panics of the must* helpers roll back.
func immediate(s *store.Store, fn func(q querier) error) (err error) {
	ctx := context.Background()
	conn, err := s.DB.Conn(ctx)
	if err != nil {
		return err
	}
	defer conn.Close()
	q := connQ{c: conn, ctx: ctx}
	deadline := time.Now().Add(30 * time.Second)
	for {
		_, err = q.Exec("BEGIN IMMEDIATE")
		if err == nil {
			break
		}
		if !isBusy(err) || time.Now().After(deadline) {
			return err
		}
		time.Sleep(5 * time.Millisecond)
	}
	defer func() {
		if r := recover(); r != nil {
			_, _ = q.Exec("ROLLBACK")
			if e, ok := r.(error); ok {
				err = e
			} else {
				panic(r)
			}
			return
		}
		if err != nil {
			_, _ = q.Exec("ROLLBACK")
			return
		}
		_, err = q.Exec("COMMIT")
	}()
	return fn(q)
}

func isBusy(err error) bool {
	if err == nil {
		return false
	}
	m := err.Error()
	return strings.Contains(m, "SQLITE_BUSY") || strings.Contains(m, "database is locked") || strings.Contains(m, "(5)") || strings.Contains(m, "(6)")
}
