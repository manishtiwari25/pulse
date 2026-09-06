package evolve

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

var managedID = regexp.MustCompile(`^[a-zA-Z][a-zA-Z0-9_-]{1,100}$`)

type Fleet struct {
	ID        string   `json:"id"`
	Project   string   `json:"project"`
	Ref       string   `json:"ref"`
	Workers   int      `json:"workers"`
	Paths     []string `json:"paths"`
	At        string   `json:"at"`
	CreatedBy string   `json:"created_by"`
	CleanedAt string   `json:"cleaned_at"`
	keys      []string
}

type FleetWorker struct {
	Worker   int    `json:"worker"`
	Path     string `json:"path"`
	Detached bool   `json:"detached"`
	NoRemote bool   `json:"no_remote"`
	NoTags   bool   `json:"no_tags"`
	Stripped bool   `json:"stripped"`
	Problem  string `json:"problem,omitempty"`
}

type FleetJob struct {
	ID     string `json:"id"`
	Status string `json:"status"`
	Result string `json:"result"`
	Branch string `json:"branch"`
}

type FleetVerification struct {
	Fleet   *Fleet        `json:"fleet"`
	OK      bool          `json:"ok"`
	Workers []FleetWorker `json:"workers"`
	Jobs    []FleetJob    `json:"jobs"`
}

func fleetPaths(home string, f *Fleet) {
	f.Paths = []string{}
	if f.CleanedAt != "" {
		return
	}
	for i := 1; i <= f.Workers; i++ {
		f.Paths = append(f.Paths, filepath.Join(home, "fleet", f.ID, fmt.Sprintf("w%d", i)))
	}
}

func PrepareFleet(ctx context.Context, s *store.Store, project, ref string, n int, actor string) (fleet *Fleet, retErr error) {
	if n < 1 || n > 256 {
		return nil, inputError("Cannot prepare fleet", "choose 1-256 workers per wave", "pulse fleet prepare --project P --n 1")
	}
	p, err := validProject(s, project)
	if err != nil {
		return nil, err
	}
	repo, commit, err := resolveRevision(ctx, p.Path, ref)
	if err != nil {
		return nil, err
	}
	base, err := fleetRoot(s, true)
	if err != nil {
		return nil, err
	}
	if within(base, repo) {
		return nil, inputError("Cannot prepare fleet", "the fleet directory must be outside the source checkout", "pulse --home DIR fleet prepare --project P --n 1")
	}
	keys, err := projectAnswerKeys(s, project)
	if err != nil {
		return nil, err
	}
	id := board.NewID(s, "fleet")
	root := filepath.Join(base, id)
	if err := os.Mkdir(root, 0o700); err != nil {
		return nil, err
	}
	defer func() {
		if retErr != nil {
			retErr = errors.Join(retErr, removeManaged(s, id))
		}
	}()
	fleet = &Fleet{ID: id, Project: project, Ref: commit, Workers: n, At: s.Now(), CreatedBy: actor, keys: keys}
	fleetPaths(filepath.Dir(base), fleet)
	for _, clone := range fleet.Paths {
		if err := strippedClone(ctx, repo, commit, clone, keys); err != nil {
			return nil, err
		}
	}
	encoded, err := json.Marshal(keys)
	if err != nil {
		return nil, err
	}
	tx, err := s.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	if _, err := tx.Exec(`INSERT INTO evolution_fleets (id, project, ref, workers, stripped_json, at, created_by)
		VALUES (?, ?, ?, ?, ?, ?, ?)`, id, project, commit, n, string(encoded), fleet.At, actor); err != nil {
		return nil, err
	}
	if err := tx.Event(actor, "evolution.fleet.prepared", id, map[string]any{"project": project, "workers": n, "ref": commit}); err != nil {
		return nil, err
	}
	return fleet, tx.Commit()
}

func ListFleets(s *store.Store, project string) ([]Fleet, error) {
	rows, err := s.DB.Query(`SELECT id, project, ref, workers, stripped_json, at, created_by, cleaned_at
		FROM evolution_fleets WHERE (? = '' OR project = ?) ORDER BY at DESC, id`, project, project)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Fleet{}
	for rows.Next() {
		var f Fleet
		var keys string
		if err := rows.Scan(&f.ID, &f.Project, &f.Ref, &f.Workers, &keys, &f.At, &f.CreatedBy, &f.CleanedAt); err != nil {
			return nil, err
		}
		if !managedID.MatchString(f.ID) || f.Workers < 1 || f.Workers > 256 || json.Unmarshal([]byte(keys), &f.keys) != nil {
			return nil, errors.New("fleet record is malformed")
		}
		for _, key := range f.keys {
			if _, err := relativePath(key); err != nil {
				return nil, errors.New("fleet strip list is malformed")
			}
		}
		fleetPaths(s.Home, &f)
		out = append(out, f)
	}
	return out, rows.Err()
}

func getFleet(s *store.Store, id string) (*Fleet, error) {
	if !managedID.MatchString(id) {
		return nil, inputError("Cannot find fleet", "expected a fleet id, not a directory path", "pulse fleet list")
	}
	fleets, err := ListFleets(s, "")
	if err != nil {
		return nil, err
	}
	for _, f := range fleets {
		if f.ID == id {
			return &f, nil
		}
	}
	return nil, inputError("No such fleet", "this store has no fleet with that id", "pulse fleet list")
}

func VerifyFleet(ctx context.Context, s *store.Store, id string) (*FleetVerification, error) {
	fleet, err := getFleet(s, id)
	if err != nil {
		return nil, err
	}
	base, err := fleetRoot(s, false)
	if err != nil {
		return nil, err
	}
	report := &FleetVerification{Fleet: fleet, OK: true, Workers: []FleetWorker{}, Jobs: []FleetJob{}}
	if fleet.CleanedAt != "" {
		return nil, inputError("Fleet has already been cleaned", "there are no worker clones to inspect", "pulse fleet list")
	}
	parent, err := os.OpenRoot(base)
	if err != nil {
		return nil, err
	}
	defer parent.Close()
	entry, err := parent.Lstat(id)
	if err != nil || !entry.IsDir() || entry.Mode()&os.ModeSymlink != 0 {
		return nil, &ViolationError{Reason: "fleet root is missing or no longer its original directory"}
	}
	for i, clone := range fleet.Paths {
		w := FleetWorker{Worker: i + 1, Path: clone}
		entry, err := os.Lstat(clone)
		if err != nil || !entry.IsDir() || entry.Mode()&os.ModeSymlink != 0 {
			w.Problem = "clone is missing or replaced"
		} else if err := checkClone(ctx, clone, fleet.keys); err != nil {
			w.Problem = "clone violates detached, remote-free, tag-free or stripping constraints"
		} else {
			w.Detached, w.NoRemote, w.NoTags, w.Stripped = true, true, true, true
		}
		if w.Problem != "" {
			report.OK = false
		}
		report.Workers = append(report.Workers, w)
	}
	rows, err := s.DB.Query("SELECT id, status, result, branch FROM jobs WHERE source = ? ORDER BY created_at, id", "fleet:"+id)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	for rows.Next() {
		var j FleetJob
		if err := rows.Scan(&j.ID, &j.Status, &j.Result, &j.Branch); err != nil {
			return nil, err
		}
		if store.SecretShaped(j.Result) {
			j.Result = "[credential-shaped result withheld]"
		}
		report.Jobs = append(report.Jobs, j)
	}
	return report, rows.Err()
}

func CleanFleet(s *store.Store, id, actor string) error {
	fleet, err := getFleet(s, id)
	if err != nil {
		return err
	}
	if fleet.CleanedAt != "" {
		return nil
	}
	var active int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM jobs WHERE source = ? AND status IN ('queued', 'leased')", "fleet:"+id).Scan(&active); err != nil {
		return err
	}
	if active != 0 {
		return inputError("Cannot clean active fleet", "worker jobs are still queued or leased", "pulse jobs")
	}
	if err := removeManaged(s, id); err != nil {
		return err
	}
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	result, err := tx.Exec("UPDATE evolution_fleets SET cleaned_at = ? WHERE id = ? AND cleaned_at = ''", s.Now(), id)
	if err != nil {
		return err
	}
	n, err := result.RowsAffected()
	if err != nil {
		return err
	}
	if n == 0 {
		return nil
	}
	if err := tx.Event(actor, "evolution.fleet.cleaned", id, map[string]any{"project": fleet.Project}); err != nil {
		return err
	}
	return tx.Commit()
}

func archivedForJob(s *store.Store, project, job string) (*Archived, error) {
	var id string
	err := s.DB.QueryRow(`SELECT a.id FROM evolution_archive a JOIN runs r ON r.id = a.run_id
		WHERE a.project = ? AND r.job_id = ? ORDER BY a.at DESC, CAST(SUBSTR(a.id,3) AS INTEGER) DESC LIMIT 1`, project, job).Scan(&id)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	rows, err := ListArchive(s, project)
	if err != nil {
		return nil, err
	}
	for _, row := range rows {
		if row.ID == id {
			return &row, nil
		}
	}
	return nil, errors.New("job archive disappeared")
}
