// Package store opens, creates and guards the PULSE SQLite store
// (PULSE_HOME/pulse.sqlite, schema 3) and offers the small helpers every
// command builds on: Now, Event, Settings, Projects, Agents.
//
// It mirrors legacy/pulse.py connect()/ensure_schema() and never migrates a
// store by itself; the only additive change it makes is EnsureRecords.
package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"modernc.org/sqlite"
	sqlite3 "modernc.org/sqlite/lib"
)

// SchemaVersion is the store schema this tool speaks (legacy/pulse.py SCHEMA_VERSION).
const SchemaVersion = 3

// StoreName is the file name of the store inside PULSE_HOME.
const StoreName = "pulse.sqlite"

const journalSizeLimitBytes = 64 * 1024 * 1024

// HumanAgent is the agents row the maintainer acts as.
const HumanAgent = "human"

// CommonsSlug is the pseudo project shared by every project.
const CommonsSlug = "commons"

// Store is an open PULSE store.
type Store struct {
	DB   *sql.DB
	Home string
	Path string

	now string
}

// Options controls Open.
type Options struct {
	Home        string // "" -> $PULSE_HOME or ~/.pulse
	Create      bool   // create a fresh store when none exists (pulse init)
	ReadOnly    bool   // open with mode=ro; implies SkipRecords
	SkipRecords bool   // do not run EnsureRecords
	Now         string // fixed clock for tests; else $PULSE_NOW, else the wall clock
}

// NotFoundError says there is no store file at Path.
type NotFoundError struct{ Path string }

func (e *NotFoundError) Error() string {
	return fmt.Sprintf("No store at %s. Run `pulse init` in a project first.", e.Path)
}

// InputError is a caller mistake (bad setting value, unknown key): exit code 2
// in the Python tool. It carries the Python message verbatim.
type InputError struct{ Msg string }

func (e *InputError) Error() string { return e.Msg }

func inputErr(format string, a ...any) error {
	return &InputError{Msg: fmt.Sprintf(format, a...)}
}

// ResolveHome returns the PULSE home: explicit, else $PULSE_HOME, else ~/.pulse.
func ResolveHome(explicit string) (string, error) {
	home := explicit
	if home == "" {
		home = os.Getenv("PULSE_HOME")
	}
	if home == "" {
		user, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("cannot find the home directory: %w", err)
		}
		home = filepath.Join(user, ".pulse")
	}
	if strings.HasPrefix(home, "~") {
		user, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("cannot find the home directory: %w", err)
		}
		home = filepath.Join(user, strings.TrimPrefix(strings.TrimPrefix(home, "~"), string(filepath.Separator)))
	}
	abs, err := filepath.Abs(home)
	if err != nil {
		return "", err
	}
	if real, err := filepath.EvalSymlinks(abs); err == nil {
		abs = real
	}
	return abs, nil
}

// StorePath is the store file under the resolved home.
func StorePath(explicit string) (string, error) {
	home, err := ResolveHome(explicit)
	if err != nil {
		return "", err
	}
	return filepath.Join(home, StoreName), nil
}

// WorkerGuard reports whether this process runs inside a runner's worker:
// PULSE_JOB is set, or the executable itself lives under home (a worker's
// clone or worktree). A worker never creates, alters or migrates the shared
// store. The installed binary under home/bin is not a worker.
func WorkerGuard(home string) bool {
	if os.Getenv("PULSE_JOB") != "" {
		return true
	}
	exe, err := os.Executable()
	if err != nil {
		return false
	}
	if real, err := filepath.EvalSymlinks(exe); err == nil {
		exe = real
	}
	if !isInside(exe, home) {
		return false
	}
	return !isInside(exe, filepath.Join(home, "bin"))
}

func isInside(path, root string) bool {
	rel, err := filepath.Rel(root, path)
	if err != nil {
		return false
	}
	return rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)))
}

// gitTopLevel returns the git checkout containing dir, or "".
func gitTopLevel(dir string) string {
	out, err := exec.Command("git", "-C", dir, "rev-parse", "--show-toplevel").Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func dsn(path string, readOnly bool) string {
	q := url.Values{}
	q.Add("_pragma", "busy_timeout(5000)")
	q.Add("_pragma", "foreign_keys(1)")
	if readOnly {
		q.Set("mode", "ro")
	} else {
		q.Add("_pragma", "journal_mode(WAL)")
		q.Add("_pragma", fmt.Sprintf("journal_size_limit(%d)", journalSizeLimitBytes))
		// every db.Begin() is BEGIN IMMEDIATE: a writer takes the lock up front
		// and waits on busy_timeout instead of failing with SQLITE_BUSY when it
		// tries to upgrade a deferred read transaction (parallel CLI start-up)
		q.Set("_txlock", "immediate")
	}
	return "file:" + path + "?" + q.Encode()
}

// Open opens (or with Options.Create, creates) the store.
func Open(o Options) (*Store, error) {
	home, err := ResolveHome(o.Home)
	if err != nil {
		return nil, err
	}
	path := filepath.Join(home, StoreName)
	guard := WorkerGuard(home)
	exists := fileExists(path)
	if !exists {
		if !o.Create || guard || o.ReadOnly {
			return nil, &NotFoundError{Path: path}
		}
		if err := refuseInsideCheckout(home); err != nil {
			return nil, err
		}
		if err := os.MkdirAll(home, 0o700); err != nil {
			return nil, err
		}
		_ = os.Chmod(home, 0o700)
	}
	db, err := sql.Open("sqlite", dsn(path, o.ReadOnly))
	if err != nil {
		return nil, err
	}
	db.SetConnMaxLifetime(0)
	s := &Store{DB: db, Home: home, Path: path, now: o.Now}
	if !exists {
		if err := initSchema(db, s.Now()); err != nil {
			db.Close()
			os.Remove(path)
			return nil, fmt.Errorf("creating %s: %w", path, err)
		}
	} else if err := s.checkVersion(); err != nil {
		db.Close()
		return nil, err
	}
	if !o.ReadOnly && !o.SkipRecords && !guard {
		if err := s.EnsureRecords(); err != nil {
			db.Close()
			return nil, err
		}
	}
	return s, nil
}

// refuseInsideCheckout mirrors the Python rule that the store must live
// outside every project checkout.
func refuseInsideCheckout(home string) error {
	dir := home
	for !fileExists(dir) && dir != filepath.Dir(dir) {
		dir = filepath.Dir(dir)
	}
	if top := gitTopLevel(dir); top != "" {
		return inputErr("The store %s must live outside the project %s; set PULSE_HOME elsewhere",
			filepath.Join(home, StoreName), top)
	}
	return nil
}

func fileExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

func (s *Store) checkVersion() error {
	var have string
	err := s.DB.QueryRow("SELECT value FROM meta WHERE key = 'schema_version'").Scan(&have)
	if err != nil {
		return fmt.Errorf("%s has no schema_version (not a PULSE store?): %w", s.Path, err)
	}
	n, err := strconv.Atoi(have)
	if err != nil {
		return fmt.Errorf("%s has a schema_version that is not a number: %q", s.Path, have)
	}
	switch {
	case n > SchemaVersion:
		return fmt.Errorf("Store schema %d is newer than this tool (%d); update pulse", n, SchemaVersion)
	case n < SchemaVersion:
		return fmt.Errorf("Store schema %d is older than this tool (%d); run `python3 legacy/pulse.py status` once to migrate it", n, SchemaVersion)
	}
	return nil
}

// Create makes a fresh store file at path exactly like the Python
// connect(create=True): DDL, meta schema_version and created_at, the default
// settings rows, the human agent, then the records tables.
func Create(path string) error {
	if fileExists(path) {
		return fmt.Errorf("%s already exists", path)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	db, err := sql.Open("sqlite", dsn(path, false))
	if err != nil {
		return err
	}
	s := &Store{DB: db, Home: filepath.Dir(path), Path: path}
	defer db.Close()
	if err := initSchema(db, s.Now()); err != nil {
		os.Remove(path)
		return err
	}
	return s.EnsureRecords()
}

// initSchema is ensure_schema() for a store without a schema_version row.
func initSchema(db *sql.DB, now string) error {
	tx, err := db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.Exec(DDL); err != nil {
		return err
	}
	var have string
	err = tx.QueryRow("SELECT value FROM meta WHERE key = 'schema_version'").Scan(&have)
	if err == nil {
		return tx.Commit()
	}
	if !errors.Is(err, sql.ErrNoRows) {
		return err
	}
	if _, err := tx.Exec("INSERT INTO meta (key, value) VALUES ('schema_version', ?)", strconv.Itoa(SchemaVersion)); err != nil {
		return err
	}
	if _, err := tx.Exec("INSERT INTO meta (key, value) VALUES ('created_at', ?)", now); err != nil {
		return err
	}
	for _, name := range SettingNames() {
		if _, err := tx.Exec("INSERT OR IGNORE INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, 'init')",
			name, DefaultSettings[name], now); err != nil {
			return err
		}
	}
	if _, err := tx.Exec("INSERT OR IGNORE INTO agents (name, display, kind, status, created_at, notes) VALUES (?, ?, ?, ?, ?, ?)",
		HumanAgent, "the human", "human", "active", now, "the maintainer; records yes or no with `pulse decide`"); err != nil {
		return err
	}
	return tx.Commit()
}

// AdditiveDDL lists, in order, every statement list EnsureRecords runs:
// the records tables plus the lists other packages own in this directory.
func AdditiveDDL() [][]string {
	return [][]string{RecordsDDL, TasksDDL, HooksDDL, RunnerDDL, EvolveDDL, HarnessDDL}
}

// Column is one column an additive list adds to a schema-3 table after the
// fact: EnsureRecords runs ALTER TABLE ... ADD COLUMN when it is missing. DDL
// is the column's type and default; it must allow existing rows (a default).
type Column struct {
	Table, Name, DDL string
}

// AdditiveColumns lists every column EnsureRecords adds to existing tables.
func AdditiveColumns() []Column {
	out := append([]Column{}, RunnerColumns...)
	out = append(out, HarnessColumns...)
	return out
}

// hasColumn reports whether table has a column of that name.
func hasColumn(q querier, table, column string) (bool, error) {
	rows, err := q.Query("SELECT name FROM pragma_table_info(?)", table)
	if err != nil {
		return false, err
	}
	defer rows.Close()
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			return false, err
		}
		if name == column {
			return true, nil
		}
	}
	return false, rows.Err()
}

var createNameRe = regexp.MustCompile(`(?i)CREATE\s+(?:TABLE|INDEX|TRIGGER)\s+IF\s+NOT\s+EXISTS\s+(\w+)`)

// additiveComplete reports whether every object the additive lists declare
// exists and the records stamp is current, i.e. start-up needs no write.
func (s *Store) additiveComplete() bool {
	have, ok, err := s.Meta("records_version")
	if err != nil || !ok || have != RecordsVersion {
		return false
	}
	names := map[string]bool{}
	for _, list := range AdditiveDDL() {
		for _, stmt := range list {
			if m := createNameRe.FindStringSubmatch(stmt); m != nil {
				names[m[1]] = true
			}
		}
	}
	rows, err := s.DB.Query("SELECT name FROM sqlite_master WHERE type IN ('table', 'index', 'trigger')")
	if err != nil {
		return false
	}
	defer rows.Close()
	for rows.Next() {
		var name string
		if err := rows.Scan(&name); err != nil {
			return false
		}
		delete(names, name)
	}
	if rows.Err() != nil || len(names) > 0 {
		return false
	}
	for _, c := range AdditiveColumns() {
		if has, err := hasColumn(s.DB, c.Table, c.Name); err != nil || !has {
			return false
		}
	}
	return true
}

// EnsureRecords adds the records tables and the other additive lists
// (IF NOT EXISTS) and stamps meta.records_version = RecordsVersion. It is a
// read-only no-op when everything is already there; otherwise it runs every
// statement in one immediate transaction and, when the stamp changed, logs
// one schema.records event.
func (s *Store) EnsureRecords() error {
	if s.additiveComplete() {
		return nil
	}
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	for _, list := range AdditiveDDL() {
		for _, stmt := range list {
			if _, err := tx.Exec(stmt); err != nil {
				return fmt.Errorf("records schema: %w", err)
			}
		}
	}
	for _, c := range AdditiveColumns() {
		has, err := hasColumn(tx, c.Table, c.Name)
		if err != nil {
			return fmt.Errorf("records schema: %w", err)
		}
		if !has {
			if _, err := tx.Exec(fmt.Sprintf("ALTER TABLE %s ADD COLUMN %s %s", c.Table, c.Name, c.DDL)); err != nil {
				return fmt.Errorf("records schema: %w", err)
			}
		}
	}
	res, err := tx.Exec("INSERT INTO meta (key, value) VALUES ('records_version', ?) "+
		"ON CONFLICT(key) DO UPDATE SET value = excluded.value WHERE value <> excluded.value", RecordsVersion)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n > 0 {
		if err := event(tx, s.Now(), "pulse", "schema.records", "records_version="+RecordsVersion, ""); err != nil {
			return err
		}
	}
	return tx.Commit()
}

// Close closes the database.
func (s *Store) Close() error {
	if s == nil || s.DB == nil {
		return nil
	}
	return s.DB.Close()
}

// CheckpointResult is one WAL checkpoint attempt.
type CheckpointResult struct {
	Busy         bool
	Log          int
	Checkpointed int
	Truncated    bool
}

// Checkpoint nudges the WAL without waiting long on readers: PASSIVE copies
// what it can, and TRUNCATE runs only when no reader kept the log busy.
func (s *Store) Checkpoint() (CheckpointResult, error) {
	var out CheckpointResult
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	conn, err := s.DB.Conn(ctx)
	if err != nil {
		if sqliteBusy(err) {
			out.Busy = true
			return out, nil
		}
		return out, err
	}
	defer conn.Close()
	before := walSize(s.Path)
	busy, log, checkpointed, err := walCheckpoint(ctx, conn, "PASSIVE")
	if err != nil {
		if sqliteBusy(err) {
			out.Busy = true
			return out, nil
		}
		return out, err
	}
	out.Busy, out.Log, out.Checkpointed = busy != 0, log, checkpointed
	if out.Busy {
		return out, nil
	}
	busy, _, _, err = walCheckpoint(ctx, conn, "TRUNCATE")
	if err != nil {
		if sqliteBusy(err) {
			out.Busy = true
			return out, nil
		}
		return out, err
	}
	out.Busy = busy != 0
	if !out.Busy {
		out.Truncated = before > walSize(s.Path)
	}
	return out, nil
}

func walCheckpoint(ctx context.Context, conn *sql.Conn, mode string) (busy, log, checkpointed int, err error) {
	err = conn.QueryRowContext(ctx, "PRAGMA wal_checkpoint("+mode+")").Scan(&busy, &log, &checkpointed)
	return busy, log, checkpointed, err
}

func sqliteBusy(err error) bool {
	var sqliteErr *sqlite.Error
	if !errors.As(err, &sqliteErr) {
		return false
	}
	return sqliteErr.Code()&0xff == sqlite3.SQLITE_BUSY
}

func walSize(path string) int64 {
	st, err := os.Stat(path + "-wal")
	if err != nil {
		return 0
	}
	return st.Size()
}

// Now is the UTC clock as "2006-01-02T15:04:05Z"; Options.Now, then
// $PULSE_NOW, win so tests and parity runs are reproducible.
func (s *Store) Now() string {
	if s != nil && s.now != "" {
		return s.now
	}
	return Now()
}

// Now is the package clock (see Store.Now).
func Now() string {
	if v := os.Getenv("PULSE_NOW"); v != "" {
		return v
	}
	return time.Now().UTC().Format("2006-01-02T15:04:05Z")
}

type execer interface {
	Exec(query string, args ...any) (sql.Result, error)
}

type querier interface {
	Query(query string, args ...any) (*sql.Rows, error)
}

// EventRecord is one events row as this package wrote it.
type EventRecord struct {
	Seq     int64  `json:"seq"`
	At      string `json:"at"`
	Actor   string `json:"actor"`
	Action  string `json:"action"`
	Subject string `json:"subject"`
	Detail  string `json:"detail"`
}

var (
	observersMu sync.RWMutex
	observers   []func(EventRecord)
)

// OnEvent registers fn to be called, in the writing goroutine, after every
// events row this package writes is durable: autocommit writes right away,
// transactional writes (Tx.Event, SetSetting, records helpers) after Commit.
// The in-process bus installs its publisher here so subscribers see the
// canonical helpers' events without polling. Events other packages insert
// themselves are not seen; they publish those on their own.
func OnEvent(fn func(EventRecord)) {
	observersMu.Lock()
	defer observersMu.Unlock()
	observers = append(observers, fn)
}

func publish(events ...EventRecord) {
	observersMu.RLock()
	fns := observers
	observersMu.RUnlock()
	for _, e := range events {
		for _, fn := range fns {
			fn(e)
		}
	}
}

// Tx is a write transaction that remembers the events written through it and
// publishes them to OnEvent observers once Commit succeeds.
type Tx struct {
	*sql.Tx
	s       *Store
	pending []EventRecord
}

// Begin starts an immediate write transaction (see Tx).
func (s *Store) Begin() (*Tx, error) {
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	return &Tx{Tx: tx, s: s}, nil
}

// Event appends to the events log inside the transaction; observers hear of
// it after Commit.
func (t *Tx) Event(actor, action, subject string, detail any) error {
	text, err := detailText(detail)
	if err != nil {
		return err
	}
	return event(t, t.s.Now(), actor, action, subject, text)
}

// Commit commits, then publishes the events written through this Tx.
func (t *Tx) Commit() error {
	if err := t.Tx.Commit(); err != nil {
		return err
	}
	pending := t.pending
	t.pending = nil
	publish(pending...)
	return nil
}

// event inserts one row. Through a *Tx the row is queued for publication at
// Commit; on any other execer (autocommit) it is published at once.
func event(x execer, at, actor, action, subject, detail string) error {
	res, err := x.Exec("INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
		at, actor, action, subject, detail)
	if err != nil {
		return err
	}
	rec := EventRecord{At: at, Actor: actor, Action: action, Subject: subject, Detail: detail}
	rec.Seq, _ = res.LastInsertId()
	if tx, ok := x.(*Tx); ok {
		tx.pending = append(tx.pending, rec)
		return nil
	}
	publish(rec)
	return nil
}

// Event appends to the events log (autocommit) and publishes it to OnEvent
// observers. detail may be a string (stored as is), nil (empty) or any value,
// which is encoded like Python's json.dumps(detail, sort_keys=True).
func (s *Store) Event(actor, action, subject string, detail any) error {
	text, err := detailText(detail)
	if err != nil {
		return err
	}
	return event(s.DB, s.Now(), actor, action, subject, text)
}

func detailText(detail any) (string, error) {
	switch d := detail.(type) {
	case nil:
		return "", nil
	case string:
		return d, nil
	case []byte:
		return string(d), nil
	}
	return PyJSON(detail)
}
