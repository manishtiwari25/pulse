package codedelivery

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

type Scope struct {
	Project    *string `json:"project"`
	Limit      int     `json:"limit"`
	Scanned    int     `json:"scanned"`
	HasMore    bool    `json:"has_more"`
	NextBefore *int64  `json:"next_before"`
}

type Summary struct {
	CodeDeliveries int `json:"code_deliveries"`
	CodeFiles      int `json:"code_files"`
	DocsOnly       int `json:"docs_only"`
	NonCode        int `json:"non_code"`
	Unknown        int `json:"unknown"`
	Duplicates     int `json:"duplicates"`
}

type JobRef struct {
	ID     string `json:"id"`
	Title  string `json:"title"`
	Kind   string `json:"kind"`
	Status string `json:"status"`
}

type TaskRef struct {
	ID    string `json:"id"`
	Title string `json:"title"`
}

type WorkerRef struct {
	Name   string `json:"name"`
	Runner string `json:"runner"`
	Model  string `json:"model"`
}

type Votes struct {
	Up    int `json:"up"`
	Down  int `json:"down"`
	Score int `json:"score"`
}

type SuggestionRef struct {
	ID      string `json:"id"`
	Summary string `json:"summary"`
	Area    string `json:"area"`
	Thread  string `json:"thread"`
	Votes   *Votes `json:"votes"`
}

type Delivery struct {
	EventSeq       int64          `json:"event_seq"`
	Project        string         `json:"project"`
	At             string         `json:"at"`
	Commit         string         `json:"commit"`
	CodeFiles      int            `json:"code_files"`
	FilesChanged   int            `json:"files_changed"`
	CodePaths      []string       `json:"code_paths"`
	PathsTruncated bool           `json:"paths_truncated"`
	Evidence       string         `json:"evidence"`
	Job            *JobRef        `json:"job"`
	Tasks          []TaskRef      `json:"tasks"`
	Worker         *WorkerRef     `json:"worker"`
	Suggestion     *SuggestionRef `json:"suggestion"`
}

type Unavailable struct {
	EventSeq int64  `json:"event_seq"`
	Project  string `json:"project"`
	At       string `json:"at"`
	Reason   string `json:"reason"`
}

type Report struct {
	Scope       Scope         `json:"scope"`
	Summary     Summary       `json:"summary"`
	Deliveries  []Delivery    `json:"deliveries"`
	Unavailable []Unavailable `json:"unavailable"`
}

type eventRow struct {
	seq    int64
	at     string
	detail string
}

type suggestionPost struct {
	id, project, area, thread, summary string
}

func Recent(ctx context.Context, s *store.Store, project string, limit int, before int64) (*Report, error) {
	ctx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	if limit <= 0 {
		limit = 100
	}
	if limit > 200 {
		return nil, fmt.Errorf("code-delivery limit exceeds the supported range")
	}
	if before <= 0 {
		before = 1<<63 - 1
	}
	scope := Scope{Limit: limit}
	if project != "" {
		p := project
		scope.Project = &p
	}
	report := &Report{Scope: scope, Deliveries: []Delivery{}, Unavailable: []Unavailable{}}
	events, hasMore, nextBefore, err := landingEvents(ctx, s, project, limit, before)
	if err != nil {
		return nil, err
	}
	report.Scope.Scanned = len(events)
	report.Scope.HasMore = hasMore
	if nextBefore > 0 {
		report.Scope.NextBefore = &nextBefore
	}
	seenCommits := map[string]bool{}
	suggestions := map[string]*SuggestionRef{}
	for _, ev := range events {
		if err := addEvent(ctx, s, report, ev, seenCommits, suggestions); err != nil {
			return nil, err
		}
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	return report, nil
}

func landingEvents(ctx context.Context, s *store.Store, project string, limit int, before int64) ([]eventRow, bool, int64, error) {
	const minimumEventScan = 5000
	scanLimit := limit * 100
	if scanLimit < minimumEventScan {
		scanLimit = minimumEventScan
	}
	var maxSeq int64
	if err := s.DB.QueryRowContext(ctx, "SELECT COALESCE(MAX(seq), 0) FROM events").Scan(&maxSeq); err != nil {
		return nil, false, 0, err
	}
	high := before - 1
	if high > maxSeq {
		high = maxSeq
	}
	if high <= 0 {
		return []eventRow{}, false, 0, nil
	}
	low := high - int64(scanLimit) + 1
	if low < 1 {
		low = 1
	}
	rows, err := s.DB.QueryContext(ctx, "SELECT seq, at, action, detail FROM events WHERE seq BETWEEN ? AND ? ORDER BY seq DESC", low, high)
	if err != nil {
		return nil, false, 0, err
	}
	defer rows.Close()
	events := []eventRow{}
	for rows.Next() {
		var action string
		var ev eventRow
		if err := rows.Scan(&ev.seq, &ev.at, &action, &ev.detail); err != nil {
			return nil, false, 0, err
		}
		if action != "land.landed" {
			continue
		}
		if project != "" {
			detail, ok := decodeDetail(ev.detail)
			if !ok || stringField(detail, "project") != project {
				continue
			}
		}
		events = append(events, ev)
		if len(events) > limit {
			break
		}
	}
	if err := rows.Err(); err != nil {
		return nil, false, 0, err
	}
	hasMore := len(events) > limit || low > 1
	nextBefore := int64(0)
	if len(events) > limit {
		events = events[:limit]
		nextBefore = events[len(events)-1].seq
	} else if low > 1 {
		nextBefore = low
	}
	return events, hasMore, nextBefore, nil
}

func addEvent(ctx context.Context, s *store.Store, report *Report, ev eventRow, seenCommits map[string]bool, suggestions map[string]*SuggestionRef) error {
	detail, ok := decodeDetail(ev.detail)
	project := stringField(detail, "project")
	if !ok {
		addUnavailable(report, ev, project, "No trustworthy changed-file evidence")
		return nil
	}
	evidence, ok := DecodeEvidence(detail["code_delivery"])
	if !ok || evidence.Evidence == EvidenceUnavailable {
		reason := evidence.Reason
		if reason == "" {
			reason = ReasonNoTrustworthyEvidence
		}
		addUnavailable(report, ev, project, reason)
		return nil
	}
	if evidence.ChangesTruncated {
		addUnavailable(report, ev, project, ReasonEvidenceTooLarge)
		return nil
	}
	commit := stringField(detail, "sha")
	if commit == "" {
		commit = evidence.After
	}
	if commit != "" {
		key := project + "\x00" + commit
		if seenCommits[key] {
			report.Summary.Duplicates++
			return nil
		}
		seenCommits[key] = true
	}
	if evidence.CodeFiles == 0 {
		if evidence.DocsFiles > 0 && evidence.NonCodeFiles == 0 {
			report.Summary.DocsOnly++
		} else {
			report.Summary.NonCode++
		}
		return nil
	}
	work := DecodeWorkSnapshot(detail["work"])
	delivery := Delivery{
		EventSeq: ev.seq, Project: project, At: ev.at, Commit: commit,
		CodeFiles: evidence.CodeFiles, FilesChanged: evidence.FilesChanged,
		CodePaths: safeCodePaths(evidence.CodePaths), PathsTruncated: evidence.PathsTruncated,
		Evidence: evidence.Evidence, Tasks: []TaskRef{},
	}
	if work.Job != nil && work.Job.ID != "" {
		delivery.Job = &JobRef{ID: work.Job.ID, Title: work.Job.Title, Kind: work.Job.Kind, Status: work.Job.Status}
	}
	if work.Worker != nil && work.Worker.Name != "" {
		delivery.Worker = &WorkerRef{Name: work.Worker.Name, Runner: work.Worker.Runner, Model: work.Worker.Model}
	}
	source := ""
	jobID := ""
	if work.Job != nil {
		source = work.Job.Source
		jobID = work.Job.ID
	}
	tasks, err := deliveryTasks(ctx, s, project, jobID, source)
	if err != nil {
		return err
	}
	delivery.Tasks = tasks
	key := project + "\x00" + source
	suggestion, cached := suggestions[key]
	if !cached {
		suggestion, err = deliverySuggestion(ctx, s, project, source)
		if err != nil {
			return err
		}
		suggestions[key] = suggestion
	}
	delivery.Suggestion = suggestion
	report.Deliveries = append(report.Deliveries, delivery)
	report.Summary.CodeDeliveries++
	report.Summary.CodeFiles += delivery.CodeFiles
	return nil
}

func addUnavailable(report *Report, ev eventRow, project, reason string) {
	report.Summary.Unknown++
	report.Unavailable = append(report.Unavailable, Unavailable{
		EventSeq: ev.seq, Project: project, At: ev.at, Reason: PublicReason(reason),
	})
}

func deliveryTasks(ctx context.Context, s *store.Store, project, jobID, source string) ([]TaskRef, error) {
	refs := []TaskRef{}
	seen := map[string]bool{}
	add := func(rows *sql.Rows, err error) error {
		if err != nil {
			return err
		}
		defer rows.Close()
		for rows.Next() {
			var ref TaskRef
			if err := rows.Scan(&ref.ID, &ref.Title); err != nil {
				return err
			}
			if !seen[ref.ID] {
				seen[ref.ID] = true
				refs = append(refs, ref)
			}
		}
		return rows.Err()
	}
	if id, ok := strings.CutPrefix(source, "task:"); ok && id != "" {
		if err := add(s.DB.QueryContext(ctx, "SELECT id, title FROM tasks WHERE project = ? AND id = ? ORDER BY id", project, id)); err != nil {
			return nil, err
		}
	}
	if jobID != "" {
		if err := add(s.DB.QueryContext(ctx, "SELECT id, title FROM tasks WHERE project = ? AND job_id = ? ORDER BY id", project, jobID)); err != nil {
			return nil, err
		}
	}
	return refs, nil
}

func deliverySuggestion(ctx context.Context, s *store.Store, project, source string) (*SuggestionRef, error) {
	id, ok := strings.CutPrefix(source, "suggestion:")
	if !ok || id == "" {
		return nil, nil
	}
	post, err := loadSuggestion(ctx, s, project, id)
	if err != nil || post == nil {
		return nil, err
	}
	votes, err := suggestionVotes(ctx, s, *post)
	if err != nil {
		return nil, err
	}
	return &SuggestionRef{ID: post.id, Summary: post.summary, Area: post.area, Thread: post.thread, Votes: votes}, nil
}

func loadSuggestion(ctx context.Context, s *store.Store, project, id string) (*suggestionPost, error) {
	row := s.DB.QueryRowContext(ctx, `SELECT id, project, area, thread, summary FROM posts
		WHERE project = ? AND id = ? AND kind = 'suggestion'`, project, id)
	var post suggestionPost
	if err := row.Scan(&post.id, &post.project, &post.area, &post.thread, &post.summary); err != nil {
		if err == sql.ErrNoRows {
			return nil, nil
		}
		return nil, err
	}
	return &post, nil
}

func suggestionVotes(ctx context.Context, s *store.Store, post suggestionPost) (*Votes, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	up, down, found, err := tally.SuggestionVoteCountsContext(ctx, s, post.project, post.id)
	if err != nil || !found {
		return nil, err
	}
	votes := &Votes{Up: up, Down: down}
	votes.Score = votes.Up - votes.Down
	return votes, nil
}

func decodeDetail(text string) (map[string]any, bool) {
	var detail map[string]any
	decoder := json.NewDecoder(strings.NewReader(text))
	decoder.UseNumber()
	if err := decoder.Decode(&detail); err != nil || detail == nil {
		return map[string]any{}, false
	}
	return detail, true
}

func stringField(row map[string]any, key string) string {
	value, ok := row[key]
	if !ok {
		return ""
	}
	switch v := value.(type) {
	case string:
		return v
	case json.Number:
		return v.String()
	default:
		return fmt.Sprint(v)
	}
}

func safeCodePaths(paths []string) []string {
	out := []string{}
	for _, path := range paths {
		if ClassifyPath(path) == FileCode {
			out = append(out, path)
		}
	}
	return out
}
