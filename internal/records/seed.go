package records

import (
	"errors"
	"strconv"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// SeedResult is one of the loop's two records after SeedLoopRecords. Action
// is kept (the record existed and was left alone), imported (the checkout's
// committed docs/evolution file, indexed), copied (the template's text) or,
// for the brain only, absent: its records arrive by import, never seeding.
// Source names what was indexed or copied: docs/evolution/constitution.md@abc1234
// or TPL-evolution-constitution@r1.
type SeedResult struct {
	ID     string `json:"id"`
	Action string `json:"action"`
	Source string `json:"source,omitempty"`
	Rev    int    `json:"rev"`
}

// SeedSummary is SeedLoopRecords' result for one project, constitution first.
type SeedSummary struct {
	Project      string     `json:"project"`
	Constitution SeedResult `json:"constitution"`
	Evaluators   SeedResult `json:"evaluators"`
}

// loopRecords is the seeded order.
var loopRecords = []string{"constitution", "evaluators"}

// SeedLoopRecords gives a registered project the loop's constitution and
// evaluators when it has none (ADR-030). A docs/evolution/<slug>.md committed
// at the checkout's HEAD is indexed as the record; otherwise the template's
// current text is copied with <Project> replaced by the project's name and
// the copy marked seeded_from and seeded_by, its author the actor. A record
// that exists, whatever its text, is kept. The brain's records are the text
// every copy starts from, so they are never written (absent when missing);
// nor are the commons'.
func SeedLoopRecords(s *store.Store, project, actor string) (SeedSummary, error) {
	summary := SeedSummary{Project: project}
	if project == store.CommonsSlug {
		return summary, inputError("Cannot seed the commons", "the loop's records belong to a project with a checkout", "pulse projects")
	}
	p, err := s.Project(project)
	if err != nil {
		return summary, err
	}
	if p == nil {
		return summary, inputError("Cannot seed "+project, "no such project", "pulse projects")
	}
	if actor == "" {
		actor = store.HumanAgent
	}
	summary.Project = p.Slug
	results := []*SeedResult{&summary.Constitution, &summary.Evaluators}
	committed := committedBlobs(p.Path, loopRecordFiles())
	created := false
	for i, slug := range loopRecords {
		result, err := seedLoopRecord(s, p, slug, actor, committed)
		if err != nil {
			return summary, err
		}
		*results[i] = result
		created = created || result.Action == "copied" || result.Action == "imported"
	}
	if created {
		if err := s.Event(actor, "evolution.records.seeded", p.Slug, summary); err != nil {
			return summary, err
		}
	}
	return summary, nil
}

func seedLoopRecord(s *store.Store, p *store.Project, slug, actor string, committed map[string]bool) (SeedResult, error) {
	existing, err := loopRecord(s, p.Slug, slug)
	if err != nil {
		return SeedResult{}, err
	}
	if existing != nil {
		return SeedResult{ID: existing.ID, Action: "kept", Rev: existing.CurrentRev}, nil
	}
	if p.Brain {
		return SeedResult{ID: scopedID("evolution", slug, p), Action: "absent"}, nil
	}
	file := "docs/evolution/" + slug + ".md"
	if committed[file] {
		return importLoopRecord(s, p, slug, file, actor)
	}
	return copyLoopRecord(s, p, slug, actor)
}

// loopRecord is the project's evolution record with that slug, whatever its
// id, or nil.
func loopRecord(s *store.Store, project, slug string) (*store.Record, error) {
	rows, err := s.ListRecords(store.RecordFilter{Project: project, Kind: "evolution", Slug: slug})
	if err != nil || len(rows) == 0 {
		return nil, err
	}
	return &rows[0], nil
}

// loopRecordFiles is the checkout path of every loop record, seeded order.
func loopRecordFiles() []string {
	files := make([]string, 0, len(loopRecords))
	for _, slug := range loopRecords {
		files = append(files, "docs/evolution/"+slug+".md")
	}
	return files
}

// committedBlobs reports which of paths HEAD of the checkout at repo has as a
// regular file, in one git call; a directory that is not a repository, has
// no commit yet or lacks the files yields none.
func committedBlobs(repo string, paths []string) map[string]bool {
	found := map[string]bool{}
	if repo == "" || len(paths) == 0 {
		return found
	}
	out, err := gitRead(repo, append([]string{"ls-tree", "-z", "HEAD", "--"}, paths...)...)
	if err != nil {
		return found
	}
	for _, line := range strings.Split(out, "\x00") {
		// <mode> SP <type> SP <object> TAB <path>
		meta, path, ok := strings.Cut(line, "\t")
		if !ok {
			continue
		}
		fields := strings.Fields(meta)
		if len(fields) >= 2 && fields[1] == "blob" {
			found[path] = true
		}
	}
	return found
}

// importLoopRecord indexes the committed file through ImportRepo, restricted
// to that one path, so the record's origin is markdown:<path>@<commit> as any
// import's. ImportRepo takes the write lock itself.
func importLoopRecord(s *store.Store, p *store.Project, slug, file, actor string) (SeedResult, error) {
	summary, err := ImportRepo(s, p.Path, "HEAD", p.Slug, ImportOptions{Author: actor, Paths: []string{file}})
	if err != nil {
		return SeedResult{}, err
	}
	for _, skipped := range summary.Skipped {
		return SeedResult{}, inputError("Cannot index "+file+" for "+p.Slug, skipped.Reason,
			"fix the committed file, then pulse evolve seed --project "+p.Slug)
	}
	rec, err := loopRecord(s, p.Slug, slug)
	if err != nil {
		return SeedResult{}, err
	}
	if rec == nil {
		return SeedResult{}, errors.New(p.Slug + ": " + file + " was not indexed")
	}
	return SeedResult{ID: rec.ID, Action: "imported", Source: file + "@" + summary.ShortRef, Rev: rec.CurrentRev}, nil
}

// copyLoopRecord writes the template's text as the project's record. The
// frontmatter is the template's with id, title, status, date and the seed
// marks set; loop, limits and baseline stay as the template leaves them
// (off, none, pending), so nothing runs until the owner edits the copy.
func copyLoopRecord(s *store.Store, p *store.Project, slug, actor string) (SeedResult, error) {
	source, revision, err := seedSource(s, slug)
	if err != nil {
		return SeedResult{}, err
	}
	unlock, err := lockWrites(s)
	if err != nil {
		return SeedResult{}, err
	}
	defer unlock()
	id := scopedID("evolution", slug, p)
	if rec, _, err := s.GetRecord(id, 0); err == nil {
		return SeedResult{ID: id, Action: "kept", Rev: rec.CurrentRev}, nil
	} else if !errors.Is(err, store.ErrNoRecord) {
		return SeedResult{}, err
	}
	name := strings.TrimSpace(p.Name)
	if name == "" {
		name = p.Slug
	}
	from := source.ID + "@r" + strconv.Itoa(revision.Rev)
	body := strings.ReplaceAll(revision.Body, "<Project>", name)
	front := make(map[string]any, len(revision.Frontmatter)+6)
	for key, value := range revision.Frontmatter {
		front[key] = value
	}
	front["id"] = id
	front["title"] = strings.ReplaceAll(textValue(revision.Frontmatter["title"]), "<Project>", name)
	front["status"] = "Active"
	front["date"] = s.Now()[:10]
	front["seeded_from"] = source.Project + "/" + from
	front["seeded_by"] = actor
	rec := store.Record{ID: id, Project: p.Slug, Kind: "evolution", Slug: slug, Origin: "store"}
	if err := applyColumns(&rec, front, body); err != nil {
		return SeedResult{}, err
	}
	front["title"] = rec.Title
	rev, err := s.PutRecord(rec, front, body, actor, "seeded from "+from+" (ADR-030)")
	if err != nil {
		return SeedResult{}, err
	}
	return SeedResult{ID: id, Action: "copied", Source: from, Rev: rev}, nil
}

// seedSource is the text a copy starts from: the current revision of the
// template-owned record (TPL-evolution-<slug>, the brain's), else the brain's
// own record as first written (EVO-<slug> at r1), before its owner shaped it.
func seedSource(s *store.Store, slug string) (*store.Record, *store.Revision, error) {
	candidates := []struct {
		id, kind string
		rev      int
	}{{"TPL-evolution-" + slug, "template", 0}, {"EVO-" + slug, "evolution", 1}}
	for _, c := range candidates {
		rec, rev, err := s.GetRecord(c.id, c.rev)
		if errors.Is(err, store.ErrNoRecord) {
			continue
		}
		if err != nil {
			return nil, nil, err
		}
		if rec.Kind == c.kind {
			return rec, rev, nil
		}
	}
	return nil, nil, inputError("Cannot seed the "+slug+" record",
		"the store has neither TPL-evolution-"+slug+" nor EVO-"+slug+" to copy from",
		"pulse migrate --records --repo BRAIN_CHECKOUT  (imports the templates), or pulse record new evolution --project P --title "+strings.ToUpper(slug[:1])+slug[1:]+" --body-file FILE")
}
