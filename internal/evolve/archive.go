package evolve

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

func init() { Archive = archive }

type ScorePair struct {
	Baseline *Scores `json:"baseline"`
	Variant  *Scores `json:"variant"`
}

type Archived struct {
	ID             string    `json:"id"`
	Project        string    `json:"project"`
	SuggestionPost string    `json:"suggestion_post"`
	Hypothesis     string    `json:"hypothesis"`
	Branch         string    `json:"branch"`
	BaselineScore  float64   `json:"baseline_score"`
	VariantScore   float64   `json:"variant_score"`
	Tolerance      float64   `json:"tolerance"`
	Kept           bool      `json:"kept"`
	Fixtures       ScorePair `json:"fixtures"`
	Sandbox        string    `json:"sandbox"`
	RunID          string    `json:"run_id"`
	At             string    `json:"at"`
	Note           string    `json:"note"`
}

func publicScores(in *Scores) (*Scores, error) {
	if in == nil || !scoreValue(in.Total) {
		return nil, errors.New("a complete finite measurement is required")
	}
	out := *in
	out.Fixtures = append([]FixtureScore{}, in.Fixtures...)
	held := 0
	for i := range out.Fixtures {
		f := &out.Fixtures[i]
		if !scoreValue(f.Score) || !finite(f.Weight) || f.Weight <= 0 {
			return nil, errors.New("fixture scores and weights must be finite and valid")
		}
		if f.Kind == "heldout" {
			held++
			f.Name, f.Note = fmt.Sprintf("H%d", held), ""
		} else if f.Kind != "public" && f.Kind != "signal" {
			return nil, errors.New("unknown fixture kind")
		}
	}
	out.Heldout = held
	return &out, nil
}

// baselineShape is the one kept row the Keep rule does not govern (ADR-031):
// the baseline measurement, where the default branch was measured once and
// written as both baseline and variant. It has the baseline hypothesis, no
// suggestion post, and identical totals and per-fixture scores; anything
// else that is kept must still beat its baseline by the tolerance.
func baselineShape(row ArchiveRow, baseline, variant *Scores) bool {
	if !IsBaselineRow(row.Hypothesis, row.SuggestionPost) || baseline.Total != variant.Total ||
		len(baseline.Fixtures) != len(variant.Fixtures) {
		return false
	}
	for i := range baseline.Fixtures {
		b, v := baseline.Fixtures[i], variant.Fixtures[i]
		if b.Name != v.Name || b.Kind != v.Kind || b.Score != v.Score || b.Weight != v.Weight {
			return false
		}
	}
	return true
}

func archive(s *store.Store, row ArchiveRow) (string, error) {
	if _, err := validProject(s, row.Project); err != nil {
		return "", err
	}
	baseline, err := publicScores(row.Baseline)
	if err != nil {
		return "", err
	}
	variant, err := publicScores(row.Variant)
	if err != nil {
		return "", err
	}
	if baseline.Shadow || variant.Shadow {
		return "", inputError("Cannot archive a shadow run", "only runner experiment landing may write the archive", "pulse evolve archive")
	}
	if !scoreValue(row.Tolerance) {
		return "", inputError("Cannot archive experiment", "tolerance must be a finite fraction in [0,1]", "pulse settings set evolve.tolerance 0.02")
	}
	if keep, _ := Keep(baseline, variant, row.Tolerance); row.Kept && !keep && !baselineShape(row, baseline, variant) {
		return "", errors.New("a kept archive row must satisfy the unchanged Keep rule")
	}
	if (baseline.Project != "" && baseline.Project != row.Project) || (variant.Project != "" && variant.Project != row.Project) {
		return "", errors.New("measurement project does not match the archive project")
	}
	for _, value := range []string{row.Hypothesis, row.Note, row.Branch, row.SuggestionPost, row.RunID} {
		if store.SecretShaped(value) {
			return "", errors.New("archive text must not contain credentials")
		}
	}
	fixtures, err := json.Marshal(ScorePair{baseline, variant})
	if err != nil {
		return "", err
	}
	tx, err := s.Begin()
	if err != nil {
		return "", err
	}
	defer tx.Rollback()
	var next int
	if err := tx.QueryRow(`SELECT COALESCE(MAX(CAST(SUBSTR(id, 3) AS INTEGER)), 0) + 1
		FROM evolution_archive WHERE project = ?`, row.Project).Scan(&next); err != nil {
		return "", err
	}
	id := fmt.Sprintf("B-%03d", next)
	sandbox := row.Sandbox
	if sandbox == "" {
		sandbox = variant.Sandbox
	}
	_, err = tx.Exec(`INSERT INTO evolution_archive
		(id, project, suggestion_post, hypothesis, branch, baseline_score, variant_score, tolerance, kept,
		 fixtures_json, sandbox, run_id, at, note) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		id, row.Project, row.SuggestionPost, row.Hypothesis, row.Branch, baseline.Total, variant.Total, row.Tolerance, row.Kept,
		string(fixtures), sandbox, row.RunID, s.Now(), row.Note)
	if err != nil {
		return "", err
	}
	if err := tx.Event("pulse", "evolution.archived", id, map[string]any{
		"project": row.Project, "kept": row.Kept, "baseline": baseline.Total, "variant": variant.Total,
		"tolerance": row.Tolerance, "suggestion_post": row.SuggestionPost, "run_id": row.RunID,
	}); err != nil {
		return "", err
	}
	return id, tx.Commit()
}

// ListArchive is read-only. The CLI has no writer counterpart.
func ListArchive(s *store.Store, project string) ([]Archived, error) {
	rows, err := s.DB.Query(`SELECT id, project, suggestion_post, hypothesis, branch, baseline_score, variant_score,
		tolerance, kept, fixtures_json, sandbox, run_id, at, note FROM evolution_archive
		WHERE (? = '' OR project = ?) ORDER BY at DESC, CAST(SUBSTR(id, 3) AS INTEGER) DESC, project`, project, project)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Archived{}
	for rows.Next() {
		var row Archived
		var fixtures string
		if err := rows.Scan(&row.ID, &row.Project, &row.SuggestionPost, &row.Hypothesis, &row.Branch,
			&row.BaselineScore, &row.VariantScore, &row.Tolerance, &row.Kept, &fixtures, &row.Sandbox, &row.RunID, &row.At, &row.Note); err != nil {
			return nil, err
		}
		if strings.TrimSpace(fixtures) != "[]" {
			if err := json.Unmarshal([]byte(fixtures), &row.Fixtures); err != nil {
				return nil, errors.New("archive fixture data is malformed")
			}
			if row.Fixtures.Baseline != nil {
				row.Fixtures.Baseline, err = publicScores(row.Fixtures.Baseline)
				if err != nil {
					return nil, err
				}
			}
			if row.Fixtures.Variant != nil {
				row.Fixtures.Variant, err = publicScores(row.Fixtures.Variant)
				if err != nil {
					return nil, err
				}
			}
		}
		out = append(out, row)
	}
	return out, rows.Err()
}
