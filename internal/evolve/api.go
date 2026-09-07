// Package evolve is the self-improvement loop in Go (brief D-M): measuring a
// tree against a project's fixtures, deciding whether an experiment is kept,
// archiving the result, the reflect digest, fleets and usage diagnostics.
//
// This file is the contract the runner builds against: the runner's landing
// for jobs of kind `experiment` calls Measure on the base and on the branch,
// Keep with the project's evolve.tolerance, then Archive - and lands only a
// kept variant. The runner's `baseline` job (ADR-031) calls Measure on the
// default branch alone and archives one kept row with BaselineHypothesis.
// Nothing else writes evolution_archive.
package evolve

import (
	"context"
	"errors"
	"io"

	"github.com/manishtiwari25/pulse/internal/store"
)

// ErrNotImplemented is returned until the package body lands.
var ErrNotImplemented = errors.New("evolve: not implemented yet")

// FixtureScore is one fixture's result on one tree.
type FixtureScore struct {
	Name   string  `json:"name"`
	Kind   string  `json:"kind"` // public | signal | heldout (held-out names are never printed to agents; use Count)
	Score  float64 `json:"score"`
	Weight float64 `json:"weight"`
	Note   string  `json:"note,omitempty"`
}

// Scores is one measurement of a tree against a project's fixtures.
type Scores struct {
	Project  string         `json:"project"`
	Repo     string         `json:"repo"`
	Ref      string         `json:"ref"`
	Fixtures []FixtureScore `json:"fixtures"`
	Total    float64        `json:"total"`   // weighted mean of Score, 0..1
	Heldout  int            `json:"heldout"` // how many held-out fixtures were measured (never which)
	Sandbox  string         `json:"sandbox"` // where the runs happened (clone root or executor)
	Shadow   bool           `json:"shadow,omitempty"`
}

// MeasureOptions tunes Measure.
type MeasureOptions struct {
	Fixtures []string  // restrict to these public fixture names; nil = all
	Shadow   bool      // a rehearsal: never archived, results may be posted as a finding by the caller
	Out      io.Writer // progress lines; nil = quiet
}

// Measure clones repo at ref once per fixture (detached, no remote, no tags,
// answer keys stripped), runs each fixture's request through the harness
// (runner_cmd -p <request>) inside the clone, scores read-only with the
// fixture's scorer and removes the clones. Held-out fixtures come from
// setting evolve.heldout_dir; their names never appear in Scores.
var Measure = func(ctx context.Context, s *store.Store, project, repo, ref string, opts MeasureOptions) (*Scores, error) {
	return nil, ErrNotImplemented
}

// Keep reports whether variant may replace baseline: variant.Total must be at
// least baseline.Total + tolerance (a fraction of 1). reason is one line for
// the archive note and the result post.
func Keep(baseline, variant *Scores, tolerance float64) (keep bool, reason string) {
	if baseline == nil || variant == nil {
		return false, "no measurement"
	}
	delta := variant.Total - baseline.Total
	if delta >= tolerance {
		return true, formatDelta("kept", delta, tolerance)
	}
	return false, formatDelta("discarded", delta, tolerance)
}

// ArchiveRow is what the runner records after an experiment was measured.
type ArchiveRow struct {
	Project        string
	SuggestionPost string
	Hypothesis     string
	Branch         string
	Baseline       *Scores
	Variant        *Scores
	Tolerance      float64
	Kept           bool
	RunID          string
	Sandbox        string
	Note           string
}

// BaselineHypothesis marks the archive row the daemon's baseline job writes
// (ADR-031): the default branch measured once, baseline and variant equal,
// kept, no suggestion post. Status reads the newest kept row as the baseline,
// so this row opens the loop for a tree that no experiment has beaten yet; it
// is not an experiment and counts in no hit rate, budget or cycle.
const BaselineHypothesis = "baseline"

// IsBaselineRow reports whether an archive row is the baseline measurement,
// not an experiment: the baseline hypothesis with no suggestion post.
func IsBaselineRow(hypothesis, suggestionPost string) bool {
	return hypothesis == BaselineHypothesis && suggestionPost == ""
}

// Archive writes one evolution_archive row (id B-NNN per project) and logs
// the event evolution.archived. Only the runner's experiment landing and its
// baseline job (ADR-031) call it.
var Archive = func(s *store.Store, row ArchiveRow) (id string, err error) {
	return "", ErrNotImplemented
}
