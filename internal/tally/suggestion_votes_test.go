package tally

import (
	"context"
	"database/sql"
	"database/sql/driver"
	"errors"
	"io"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func TestCodeDeliveryVotesRejectIncompleteIteration(t *testing.T) {
	iterationErr := errors.New("vote row iteration failed")
	for _, tc := range []struct {
		name string
		want error
	}{
		{"row-error", iterationErr},
		{"cancelled-at-eof", context.Canceled},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			db := sql.OpenDB(voteRowsConnector{finish: func() error {
				if errors.Is(tc.want, context.Canceled) {
					cancel()
					return io.EOF
				}
				return iterationErr
			}})
			defer db.Close()
			up, down, found, err := SuggestionVoteCountsContext(ctx, &store.Store{DB: db}, "demo", "source")
			if !errors.Is(err, tc.want) || found || up != 0 || down != 0 {
				t.Fatalf("incomplete votes returned up=%d down=%d found=%t err=%v; want %v",
					up, down, found, err, tc.want)
			}
		})
	}
}

type voteRowsConnector struct{ finish func() error }

func (c voteRowsConnector) Connect(context.Context) (driver.Conn, error) {
	return voteRowsConn{finish: c.finish}, nil
}
func (c voteRowsConnector) Driver() driver.Driver { return c }
func (c voteRowsConnector) Open(string) (driver.Conn, error) {
	return c.Connect(context.Background())
}

type voteRowsConn struct{ finish func() error }

func (voteRowsConn) Prepare(string) (driver.Stmt, error) {
	return nil, errors.New("unexpected prepare")
}
func (voteRowsConn) Begin() (driver.Tx, error) { return nil, errors.New("unexpected transaction") }
func (voteRowsConn) Close() error              { return nil }
func (c voteRowsConn) QueryContext(context.Context, string, []driver.NamedValue) (driver.Rows, error) {
	return &voteRows{finish: c.finish}, nil
}

type voteRows struct {
	index  int
	finish func() error
}

func (*voteRows) Columns() []string {
	return []string{"id", "kind", "date", "agent", "session", "on_post", "vote"}
}
func (*voteRows) Close() error { return nil }
func (r *voteRows) Next(values []driver.Value) error {
	switch r.index {
	case 0:
		copy(values, []driver.Value{"source", "suggestion", "2026-09-01", "author", "", "", ""})
	case 1:
		copy(values, []driver.Value{"vote", "vote", "2026-09-02", "voter", "", "source", "up"})
	default:
		return r.finish()
	}
	r.index++
	return nil
}
