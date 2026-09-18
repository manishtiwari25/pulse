package tally

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// The constitution lives in the store as a record of kind "evolution", slug
// "constitution" (D-H), one per project. Records are keyed by id alone, so the
// brain's is "EVO-constitution" (the id `pulse migrate --records` gives the
// framework's own docs/evolution/constitution.md) and every other project's
// is "EVO-constitution-<slug>". Readers never rely on the id: they ask
// ListRecords for project + kind + slug first and fall back to those ids.

// ConstitutionID is the record id a project's constitution gets when it is
// created through this package (see the package note above).
func ConstitutionID(s *store.Store, project string) string {
	if brain, _ := s.Brain(); brain != nil && brain.Slug == project {
		return "EVO-constitution"
	}
	return "EVO-constitution-" + project
}

// hasRecords reports whether the records tables exist (a read-only snapshot
// opened with SkipRecords may predate them).
func hasRecords(s *store.Store) bool {
	var n int
	err := s.DB.QueryRow("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'records'").Scan(&n)
	return err == nil && n > 0
}

// ConstitutionRecord finds a project's constitution record and the requested
// revision (rev <= 0: current). It returns store.ErrNoRecord when the project
// has none.
func ConstitutionRecord(s *store.Store, project string, rev int) (*store.Record, *store.Revision, error) {
	if !hasRecords(s) {
		return nil, nil, store.ErrNoRecord
	}
	list, err := s.ListRecords(store.RecordFilter{Project: project, Kind: "evolution", Slug: "constitution"})
	if err != nil {
		return nil, nil, err
	}
	id := ""
	if len(list) > 0 {
		id = list[0].ID
	} else {
		for _, cand := range []string{"EVO-constitution-" + project, "EVO-constitution"} {
			r, _, err := s.GetRecord(cand, 0)
			if err == nil && r.Project == project {
				id = cand
				break
			}
		}
	}
	if id == "" {
		return nil, nil, store.ErrNoRecord
	}
	return s.GetRecord(id, rev)
}

// ConstitutionText is the body of the project's current constitution, "" when
// the project has none (the Python behaviour for a missing file).
func ConstitutionText(s *store.Store, project string) string {
	_, rev, err := ConstitutionRecord(s, project, 0)
	if err == nil && rev != nil {
		return rev.Body
	}
	return legacyFile(s, project, "constitution.md")
}

// LegacyFiles is the D-L residual: when a project has no constitution record
// yet (a store `pulse migrate --records` has not seen, or the parity
// snapshot), the tally reads <project path>/docs/evolution/constitution.md
// and honours docs/evolution/STOP, exactly as the Python tool does, so the
// two agree until the record exists. SQL wins whenever a record is present.
// Set it to false (or delete legacyFile) once every registered project has
// its EVO-constitution record; the "file" mode of TestFixtureParity and the
// hub's constitution_present golden allowance go with it.
var LegacyFiles = true

// legacyFile is the docs/evolution/<name> text of a project, "" when
// LegacyFiles is off, the project is unknown or the file is missing.
func legacyFile(s *store.Store, project, name string) string {
	if !LegacyFiles || project == "" || project == store.CommonsSlug {
		return ""
	}
	p, err := s.Project(project)
	if err != nil || p == nil || p.Path == "" {
		return ""
	}
	b, err := os.ReadFile(filepath.Join(p.Path, "docs", "evolution", name))
	if err != nil {
		return ""
	}
	return string(b)
}

// stopPresent is the kill switch: the `stop` setting (D-M), or, while
// LegacyFiles is on, the docs/evolution/STOP file the Python tool honours.
func stopPresent(s *store.Store, project string) bool {
	if s.Setting("stop", "") == "1" {
		return true
	}
	if !LegacyFiles || project == "" || project == store.CommonsSlug {
		return false
	}
	p, err := s.Project(project)
	if err != nil || p == nil || p.Path == "" {
		return false
	}
	_, err = os.Stat(filepath.Join(p.Path, "docs", "evolution", "STOP"))
	return err == nil
}

// PassedAmendments lists the open amendments of a project whose tally status
// is "passed ..." (quorum reached and the supermajority met, nothing blocking).
// Ratification paused by STOP still counts as passed here; ApplyAmendment
// refuses it.
func PassedAmendments(s *store.Store, project string, th Thresholds) ([]Amendment, error) {
	res, err := ComputeLight(s, project, "", th)
	if err != nil {
		return nil, err
	}
	var out []Amendment
	for _, a := range res.Amendments {
		if a.Open && strings.HasPrefix(a.Status, "passed") {
			out = append(out, a)
		}
	}
	return out, nil
}

// Applied is what ApplyAmendment did or found.
type Applied struct {
	PostID    string `json:"post_id"`
	Project   string `json:"project"`
	RecordID  string `json:"record_id"`
	Rev       int    `json:"rev"`
	Hash      string `json:"hash"`
	Section   string `json:"section"`
	Author    string `json:"author"`
	AlreadyAt string `json:"already_at,omitempty"` // when the event log shows it was applied before
	Notice    string `json:"notice"`               // the notice text for the amendment's thread
}

// ApplyAmendment writes a passed amendment into the project's constitution
// record as a new revision (author = the amendment's agent, note = the post
// id) and logs `constitution.amended`. It is idempotent: an amendment the
// event log already shows as applied returns Applied.AlreadyAt and changes
// nothing. It refuses (with a *cli-friendly error text) an amendment that has
// not passed, is paused by STOP, or touches a protected section - those need
// the human: `pulse decide <post-id> yes`.
//
// The caller posts Applied.Notice on the amendment's thread (see Ratify):
// a `result` with `resolution: ratified` is what closes the amendment in the
// tally, exactly as the amend workflow always required of humans.
func ApplyAmendment(s *store.Store, postID string, th Thresholds) (*Applied, error) {
	post, err := loadPost(s, postID)
	if err != nil {
		return nil, err
	}
	if post.Kind != "amendment" {
		return nil, &ApplyError{What: fmt.Sprintf("%s is a %s, not an amendment", postID, post.Kind), Fix: "pulse board"}
	}
	if at := appliedAt(s, postID); at != "" {
		rec, _, err := ConstitutionRecord(s, post.Project, 0)
		if err != nil {
			return nil, err
		}
		return &Applied{PostID: postID, Project: post.Project, RecordID: rec.ID, Rev: rec.CurrentRev, Author: post.Agent, AlreadyAt: at,
			Notice: fmt.Sprintf("amendment %s was already applied at %s", postID, at)}, nil
	}
	res, err := ComputeLight(s, post.Project, "", th)
	if err != nil {
		return nil, err
	}
	var am *Amendment
	for i := range res.Amendments {
		if res.Amendments[i].ID == postID {
			am = &res.Amendments[i]
		}
	}
	if am == nil {
		return nil, &ApplyError{What: "amendment " + postID + " is not in the tally of " + post.Project, Fix: "pulse board --project " + post.Project}
	}
	switch {
	case strings.HasPrefix(am.Status, "blocked: protected section"):
		return nil, &ApplyError{What: "amendment " + postID + " touches a protected section", Why: am.Status,
			Fix: "only the human ratifies it: pulse decide " + postID + " yes --words '...'"}
	case strings.HasPrefix(am.Status, "passed; ratification paused"):
		return nil, &ApplyError{What: "amendment " + postID + " passed but ratification is paused", Why: am.Status, Fix: "pulse start"}
	case !strings.HasPrefix(am.Status, "passed"):
		return nil, &ApplyError{What: "amendment " + postID + " has not passed", Why: am.Status, Fix: "pulse ballot --project " + post.Project}
	}
	rec, rev, err := ConstitutionRecord(s, post.Project, 0)
	if errors.Is(err, store.ErrNoRecord) {
		return nil, &ApplyError{What: post.Project + " has no constitution record", Why: "nothing to amend", Fix: "pulse migrate --records --repo PATH --project " + post.Project}
	}
	if err != nil {
		return nil, err
	}
	body, ok := replaceCurrentText(rev.Body, post.Body)
	if !ok {
		return nil, &ApplyError{What: "the replaced text is not in the constitution exactly once", Why: "the record changed since the tally", Fix: "pulse constitution --project " + post.Project}
	}
	newRev, err := s.PutRecord(*rec, rev.Frontmatter, body, post.Agent, postID)
	if err != nil {
		return nil, err
	}
	detail := map[string]any{"project": post.Project, "record": rec.ID, "rev": newRev, "hash": am.Hash, "section": am.Section}
	if err := s.Event(post.Agent, "constitution.amended", postID, detail); err != nil {
		return nil, err
	}
	return &Applied{PostID: postID, Project: post.Project, RecordID: rec.ID, Rev: newRev, Hash: am.Hash, Section: am.Section, Author: post.Agent,
		Notice: fmt.Sprintf("amendment %s ratified: %s revision %d replaces the quoted text (section %s; %d up, %d down, %d voters)",
			postID, rec.ID, newRev, am.Section, am.Up, am.Down, am.Voters)}, nil
}

// ApplyError is a refusal told as what / why / fix (exit code 2).
type ApplyError struct{ What, Why, Fix string }

func (e *ApplyError) Error() string {
	msg := e.What
	if e.Why != "" {
		msg += "\n  why: " + e.Why
	}
	if e.Fix != "" {
		msg += "\n  fix: " + e.Fix
	}
	return msg
}

// appliedAt returns the time of the constitution.amended event for the post,
// "" when there is none.
func appliedAt(s *store.Store, postID string) string {
	var at string
	err := s.DB.QueryRow("SELECT at FROM events WHERE action = 'constitution.amended' AND subject = ? ORDER BY seq DESC LIMIT 1", postID).Scan(&at)
	if err != nil {
		return ""
	}
	return at
}

// replaceCurrentText swaps the amendment's "Current text" block for its
// "Proposed text" in the constitution, using the same normalization the check
// used (trailing whitespace off every line, one trailing newline). ok is false
// unless the current text occurs exactly once.
func replaceCurrentText(constitution, amendmentBody string) (string, bool) {
	cur := fencedBlock(amendmentBody, "Current text")
	prop := fencedBlock(amendmentBody, "Proposed text")
	if cur == nil || prop == nil {
		return "", false
	}
	lines := strings.Split(constitution, "\n")
	for i := range lines {
		lines[i] = strings.TrimRight(lines[i], " \t")
	}
	if len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	hay := "\n" + strings.Join(lines, "\n") + "\n"
	needle := "\n" + strings.Join(cur, "\n") + "\n"
	if strings.Count(hay, needle) != 1 {
		return "", false
	}
	out := strings.Replace(hay, needle, "\n"+strings.Join(prop, "\n")+"\n", 1)
	return strings.TrimPrefix(out, "\n"), true
}

func loadPost(s *store.Store, id string) (*Post, error) {
	rows, err := s.DB.Query("SELECT "+postCols+" FROM posts WHERE id = ?", id)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	posts, err := scanPosts(rows)
	if err != nil {
		return nil, err
	}
	if len(posts) == 0 {
		return nil, &NoPostError{ID: id}
	}
	return &posts[0], nil
}

// NoPostError is "No post 'x'" (exit code 2 in the Python tool).
type NoPostError struct{ ID string }

func (e *NoPostError) Error() string { return fmt.Sprintf("No post %q", e.ID) }

// LoadPost returns one post by id or a *NoPostError.
func LoadPost(s *store.Store, id string) (*Post, error) { return loadPost(s, id) }

// SystemAgent is the identity the runner uses when it acts on the board by
// itself (ratifying a passed amendment, auto-approving compute).
const SystemAgent = "pulse"
