package hub

import (
	"encoding/json"
	"reflect"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
)

// assertEvolutionFixtures accepts the Go document ([]map[string]any) and its
// decoded JSON ([]any): each fixture row carries exactly the seven public keys.
func assertEvolutionFixtures(t *testing.T, value any) []map[string]any {
	t.Helper()
	rows := []map[string]any{}
	switch list := value.(type) {
	case []map[string]any:
		rows = list
	case []any:
		for _, item := range list {
			row, ok := item.(map[string]any)
			if !ok {
				t.Fatalf("fixture row is not an object: %#v", item)
			}
			rows = append(rows, row)
		}
	default:
		t.Fatalf("fixtures is not a list: %#v", value)
	}
	for _, row := range rows {
		if !reflect.DeepEqual(sortedKeys(row), []string{"at", "created_by", "enabled", "kind", "name", "starter", "weight"}) {
			t.Fatalf("fixture row keys: %v", sortedKeys(row))
		}
	}
	return rows
}

func starterStore(t *testing.T) (*store.Store, string) {
	t.Helper()
	s := additionsStore(t, false)
	p, err := board.AddProject(s, lifecycleRepo(t), "Seeded", "", false, "", "human", "seeded")
	if err != nil {
		t.Fatal(err)
	}
	return s, p.Slug
}

func TestEvolutionGatesAndFixturesNeverExposeTexts(t *testing.T) {
	s, slug := starterStore(t)
	if _, err := evolve.AddFixture(s, evolve.FixtureInput{Project: slug, Name: "owner-fixture", Request: "OWNER_REQUEST_TEXT",
		Scorer: "OWNER_SCORER_TEXT", Kind: "public", Weight: 2, Actor: "human"}); err != nil {
		t.Fatal(err)
	}
	if err := evolve.SetFixtureEnabled(s, slug, "starter-hardstop-publish", false, "human"); err != nil {
		t.Fatal(err)
	}
	h := New(s, nil)
	status, data := additionsRequest(t, h, "GET", "/api/v1/projects/"+slug+"/evolution", "")
	if status != 200 {
		t.Fatalf("evolution: %d %#v", status, data)
	}
	assertEvolutionShape(t, data)
	if _, failed := data["gates_error"]; failed {
		t.Fatalf("gates failed for a registered project: %#v", data["gates_error"])
	}
	gates := data["gates"].(map[string]any)
	for _, key := range []string{"project", "stop", "loop", "limits", "tolerance", "constitution", "evaluators", "baseline",
		"baseline_job", "baseline_failed_at", "public_fixtures", "heldout", "kept", "discarded", "voted", "signal", "product_scope",
		"cycles", "hours", "cap_reached", "level", "reason", "next_human_step", "planning"} {
		if _, ok := gates[key]; !ok {
			t.Fatalf("gates lacks %s: %v", key, sortedKeys(gates))
		}
	}
	if gates["project"] != slug || gates["public_fixtures"] != float64(6) || gates["level"] != "B" || gates["baseline"] != nil ||
		gates["baseline_job"] != nil || gates["baseline_failed_at"] != "" {
		t.Fatalf("gates: %#v", gates)
	}
	planning, ok := gates["planning"].(map[string]any)
	if !ok || planning["state"] != "gated" || !strings.Contains(str(planning["reason"]), str(gates["reason"])) {
		t.Fatalf("planning does not carry the actual gate: %#v", gates["planning"])
	}
	rows := assertEvolutionFixtures(t, data["fixtures"])
	if len(rows) != 7 {
		t.Fatalf("fixture rows: %d", len(rows))
	}
	starters, disabled := 0, 0
	for _, row := range rows {
		name := row["name"].(string)
		if row["starter"] != strings.HasPrefix(name, "starter-") || row["kind"] != "public" {
			t.Fatalf("fixture row: %#v", row)
		}
		if row["starter"] == true {
			starters++
		}
		if row["enabled"] == false {
			disabled++
			if name != "starter-hardstop-publish" {
				t.Fatalf("wrong fixture disabled: %#v", row)
			}
		}
		if name == "owner-fixture" && row["weight"] != float64(2) {
			t.Fatalf("owner fixture row: %#v", row)
		}
	}
	if starters != 6 || disabled != 1 {
		t.Fatalf("starters %d disabled %d", starters, disabled)
	}
	encoded, _ := json.Marshal(data)
	for _, forbidden := range []string{"OWNER_REQUEST_TEXT", "OWNER_SCORER_TEXT", "pulse evolve score", "recieve", "squash every commit"} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatalf("fixture text %q escaped into the evolution payload", forbidden)
		}
	}
}

func TestEvolutionGatesFailSoftly(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "raw")
	h := New(s, nil)
	data, err := h.evolution("raw")
	if err != nil {
		t.Fatal(err)
	}
	assertEvolutionShape(t, data)
	if len(data["fixtures"].([]map[string]any)) != 0 {
		t.Fatalf("a raw project row invented fixtures: %#v", data["fixtures"])
	}
	// A project the loop knows: registration copied its constitution and
	// evaluators from the bundled templates (ADR-038), so the gates describe
	// the next step, the baseline.
	s2, slug := starterStore(t)
	data, err = New(s2, nil).evolution(slug)
	if err != nil {
		t.Fatal(err)
	}
	assertEvolutionShape(t, data)
	if gates := data["gates"].(map[string]any); gates["constitution"] != true || gates["evaluators"] != true ||
		gates["reason"] != "no baseline measured yet; the daemon records it once the loop is on" || !strings.Contains(gates["next_human_step"].(string), "evolve.loop") {
		t.Fatalf("gates for a fresh project: %#v", gates)
	}
	// The commons is not a project the loop describes: gates nil, the message says why.
	data, err = New(s2, nil).evolution(store.CommonsSlug)
	if err != nil {
		t.Fatal(err)
	}
	assertEvolutionShape(t, data)
	if data["gates"] != nil || data["gates_error"] != "Evolution needs a registered project" {
		t.Fatalf("commons gates: %#v %#v", data["gates"], data["gates_error"])
	}
}

func TestProjectRowsCarryTheLoopPosition(t *testing.T) {
	s, slug := starterStore(t)
	additionsProject(t, s, "raw")
	h := New(s, nil)
	status, result := additionsRequest(t, h, "GET", "/api/v1/projects", "")
	if status != 200 {
		t.Fatalf("projects: %d %#v", status, result)
	}
	rows := map[string]map[string]any{}
	for _, item := range result["projects"].([]any) {
		row := item.(map[string]any)
		rows[row["slug"].(string)] = row
	}
	seeded, raw := rows[slug], rows["raw"]
	if seeded == nil || raw == nil {
		t.Fatalf("project rows: %#v", result)
	}
	for _, row := range rows {
		for _, key := range []string{"fixtures_enabled", "baseline", "evolve_level", "evolve_next", "constitution", "evaluators",
			"baseline_job", "baseline_failed_at"} {
			if _, ok := row[key]; !ok {
				t.Fatalf("row %s lacks %s: %v", row["slug"], key, sortedKeys(row))
			}
		}
		if row["baseline_job"] != nil || row["baseline_failed_at"] != "" {
			t.Fatalf("row %s carries a baseline job nobody queued: %#v", row["slug"], row)
		}
	}
	// Registration seeded the loop's records from the bundled templates (ADR-038).
	constitution, _ := seeded["constitution"].(map[string]any)
	evaluators, _ := seeded["evaluators"].(map[string]any)
	if seeded["fixtures_enabled"] != float64(6) || seeded["baseline"] != false || seeded["evolve_level"] != "B" ||
		!strings.Contains(seeded["evolve_next"].(string), "evolve.loop") || constitution["seeded_from"] != store.CommonsSlug+"/TPL-evolution-constitution@r1" ||
		evaluators["seeded_from"] != store.CommonsSlug+"/TPL-evolution-evaluators@r1" {
		t.Fatalf("seeded row: %#v", seeded)
	}
	if raw["fixtures_enabled"] != float64(0) || raw["baseline"] != false || raw["evolve_level"] != "B" || raw["evolve_next"] == "" {
		t.Fatalf("raw row: %#v", raw)
	}
	if err := evolve.SetFixtureEnabled(s, slug, "starter-hardstop-publish", false, "human"); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("UPDATE projects SET status = 'inactive' WHERE slug = 'raw'"); err != nil {
		t.Fatal(err)
	}
	_, result = additionsRequest(t, h, "GET", "/api/v1/projects", "")
	for _, item := range result["projects"].([]any) {
		row := item.(map[string]any)
		switch row["slug"] {
		case slug:
			if row["fixtures_enabled"] != float64(5) {
				t.Fatalf("seeded row after a disable: %#v", row)
			}
		case "raw":
			// A removed project keeps its position: restoring it changes nothing there.
			if row["status"] != "inactive" || row["evolve_level"] != "B" || row["fixtures_enabled"] != float64(0) {
				t.Fatalf("inactive row: %#v", row)
			}
		}
	}
}

// ADR-031: the project rows and the Evolution tab carry the daemon's baseline
// job while it is open and the failure that holds the next attempt.
func TestProjectRowsCarryTheBaselineJob(t *testing.T) {
	s, slug := templateStore(t)
	h := New(s, nil)
	projectRow := func() map[string]any {
		t.Helper()
		_, result := additionsRequest(t, h, "GET", "/api/v1/projects", "")
		for _, item := range result["projects"].([]any) {
			if row := item.(map[string]any); row["slug"] == slug {
				return row
			}
		}
		t.Fatalf("no row for %s: %#v", slug, result)
		return nil
	}
	gates := func() map[string]any {
		t.Helper()
		status, data := additionsRequest(t, h, "GET", "/api/v1/projects/"+slug+"/evolution", "")
		if status != 200 {
			t.Fatalf("evolution: %d %#v", status, data)
		}
		assertEvolutionShape(t, data)
		return data["gates"].(map[string]any)
	}
	job, _, err := board.AddJob(s, board.JobInput{Project: slug, Kind: "baseline", Title: "Record baseline", Priority: 20,
		Source: "baseline:" + slug, CreatedBy: "daemon"})
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]any{"id": job.ID, "status": "queued"}
	if row := projectRow(); !reflect.DeepEqual(row["baseline_job"], want) || row["baseline_failed_at"] != "" || row["baseline"] != false {
		t.Fatalf("row with a queued baseline job: %#v", row)
	}
	if g := gates(); !reflect.DeepEqual(g["baseline_job"], want) || g["baseline_failed_at"] != "" ||
		!strings.Contains(g["reason"].(string), "baseline measuring (job "+job.ID+", queued)") {
		t.Fatalf("gates with a queued baseline job: %#v", g)
	}
	if _, err := board.FinishJob(s, job.ID, "aurora", "failed", "measurement failed", "", nil); err != nil {
		t.Fatal(err)
	}
	if row := projectRow(); row["baseline_job"] != nil || row["baseline_failed_at"] != s.Now() || row["baseline"] != false {
		t.Fatalf("row after a failed baseline job: %#v", row)
	}
	if g := gates(); g["baseline_job"] != nil || g["baseline_failed_at"] != s.Now() || !strings.Contains(g["reason"].(string), "retries after 24 hours") {
		t.Fatalf("gates after a failed baseline job: %#v", g)
	}
	// The raw archive rows keep their hypothesis, so the tab can tell the baseline row from an experiment.
	scores := &evolve.Scores{Project: slug, Total: 1, Fixtures: []evolve.FixtureScore{{Name: "starter-readme", Kind: "public", Score: 1, Weight: 1}}}
	id, err := evolve.Archive(s, evolve.ArchiveRow{Project: slug, Hypothesis: evolve.BaselineHypothesis, Branch: "main", Baseline: scores,
		Variant: scores, Tolerance: .02, Kept: true, RunID: "run-1", Note: "baseline measurement"})
	if err != nil {
		t.Fatal(err)
	}
	if row := projectRow(); row["baseline"] != true || row["baseline_failed_at"] != s.Now() {
		t.Fatalf("row after the baseline row: %#v", row)
	}
	_, data := additionsRequest(t, h, "GET", "/api/v1/projects/"+slug+"/evolution", "")
	archive := data["archive"].(map[string]any)["rows"].([]any)
	if len(archive) != 1 {
		t.Fatalf("archive rows: %#v", archive)
	}
	if arow := archive[0].(map[string]any); arow["id"] != id || arow["hypothesis"] != "baseline" || arow["kept"] != float64(1) {
		t.Fatalf("archive row: %#v", arow)
	}
	if g := data["gates"].(map[string]any); g["baseline"] != float64(1) || g["kept"] != float64(0) || g["cycles"] != float64(0) {
		t.Fatalf("gates after the baseline row: %#v", g)
	}
}

// templateStore is a store whose brain holds the template-owned records a
// registration copies (ADR-030): the project registered after it is seeded.
func templateStore(t *testing.T) (*store.Store, string) {
	t.Helper()
	s := additionsStore(t, false)
	// Registering the brain seeds the framework's records from the bundle
	// (ADR-038), the templates at r1 under the commons; the owner's shaping
	// of them is their r2.
	if _, err := board.AddProject(s, lifecycleRepo(t), "Brain", "", true, "", "human", "brain"); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"constitution", "evaluators"} {
		title := "<Project> Evolution " + strings.ToUpper(name[:1]) + name[1:]
		rec := store.Record{ID: "TPL-evolution-" + name, Project: store.CommonsSlug, Kind: "template", Slug: "evolution-" + name, Title: title, Status: "Active",
			Path: "docs/evolution/_template-" + name + ".md"}
		if _, err := s.PutRecord(rec, map[string]any{"id": strings.ToUpper(name), "title": title, "status": "Active", "loop": "off"},
			"# "+title+"\n\nThe <Project> loop.\n", "human", "shaped"); err != nil {
			t.Fatal(err)
		}
	}
	p, err := board.AddProject(s, lifecycleRepo(t), "Lamplit Labs", "", false, "", "alice", "lamplitlabs")
	if err != nil {
		t.Fatal(err)
	}
	return s, p.Slug
}

func TestProjectRowsAndEvolutionTellSeededRecordsApart(t *testing.T) {
	s, slug := templateStore(t)
	additionsProject(t, s, "raw")
	h := New(s, nil)
	loopRecord := func(row map[string]any, name string) map[string]any {
		t.Helper()
		value, ok := row[name].(map[string]any)
		if !ok {
			t.Fatalf("%s of %v is not an object: %#v", name, row["slug"], row[name])
		}
		if !reflect.DeepEqual(sortedKeys(value), []string{"draft", "draft_job", "id", "rev", "seeded", "seeded_from"}) {
			t.Fatalf("%s keys: %v", name, sortedKeys(value))
		}
		return value
	}
	_, result := additionsRequest(t, h, "GET", "/api/v1/projects", "")
	rows := map[string]map[string]any{}
	for _, item := range result["projects"].([]any) {
		row := item.(map[string]any)
		rows[row["slug"].(string)] = row
	}
	constitution := loopRecord(rows[slug], "constitution")
	if constitution["id"] != "EVO-constitution-lamplitlabs" || constitution["rev"] != float64(1) || constitution["seeded"] != true ||
		constitution["seeded_from"] != store.CommonsSlug+"/TPL-evolution-constitution@r2" || constitution["draft"] != nil || constitution["draft_job"] != nil {
		t.Fatalf("seeded constitution: %#v", constitution)
	}
	if evaluators := loopRecord(rows[slug], "evaluators"); evaluators["id"] != "EVO-evaluators-lamplitlabs" || evaluators["seeded"] != true ||
		evaluators["seeded_from"] != store.CommonsSlug+"/TPL-evolution-evaluators@r2" {
		t.Fatalf("seeded evaluators: %#v", evaluators)
	}
	if rows["raw"]["constitution"] != nil || rows["raw"]["evaluators"] != nil || rows["brain"]["constitution"] != nil {
		t.Fatalf("records invented for a project without them: %#v %#v", rows["raw"], rows["brain"])
	}
	if next := rows[slug]["evolve_next"].(string); strings.Contains(next, "evolve seed") || strings.Contains(next, "record new") {
		t.Fatalf("a seeded project is still told to write its constitution: %q", next)
	}
	status, data := additionsRequest(t, h, "GET", "/api/v1/projects/"+slug+"/evolution", "")
	if status != 200 {
		t.Fatalf("evolution: %d %#v", status, data)
	}
	assertEvolutionShape(t, data)
	doc := data["constitution"].(map[string]any)
	if doc["seeded"] != true || doc["seeded_from"] != store.CommonsSlug+"/TPL-evolution-constitution@r2" ||
		!strings.Contains(doc["body"].(string), "The Lamplit Labs loop.") {
		t.Fatalf("evolution constitution: %#v", doc)
	}
	if doc := data["evaluators"].(map[string]any); doc["seeded"] != true || doc["seeded_from"] != store.CommonsSlug+"/TPL-evolution-evaluators@r2" {
		t.Fatalf("evolution evaluators: %#v", doc)
	}
	// The owner's first edit makes the record theirs: still marked where it
	// came from, no longer the seeded text.
	if _, err := records.Amend(s, "EVO-constitution-lamplitlabs", "# Ours\n\nShaped by the owner.\n", "human", "shaped"); err != nil {
		t.Fatal(err)
	}
	_, result = additionsRequest(t, h, "GET", "/api/v1/projects", "")
	for _, item := range result["projects"].([]any) {
		row := item.(map[string]any)
		if row["slug"] != slug {
			continue
		}
		constitution, evaluators := loopRecord(row, "constitution"), loopRecord(row, "evaluators")
		if constitution["rev"] != float64(2) || constitution["seeded"] != false || constitution["seeded_from"] != store.CommonsSlug+"/TPL-evolution-constitution@r2" ||
			evaluators["seeded"] != true {
			t.Fatalf("rows after the owner's edit: %#v %#v", constitution, evaluators)
		}
	}
	_, data = additionsRequest(t, h, "GET", "/api/v1/projects/"+slug+"/evolution", "")
	if doc := data["constitution"].(map[string]any); doc["seeded"] != false || doc["seeded_from"] != store.CommonsSlug+"/TPL-evolution-constitution@r2" {
		t.Fatalf("evolution constitution after the owner's edit: %#v", doc)
	}
	// A hand-written record was never seeded: no mark, not seeded.
	additionsRecord(t, s, "EVO-constitution-raw", "raw", "evolution", "constitution", "# Raw\n")
	data, err := h.evolution("raw")
	if err != nil {
		t.Fatal(err)
	}
	if doc := data["constitution"].(map[string]any); doc["seeded"] != false || doc["seeded_from"] != "" {
		t.Fatalf("hand-written constitution: %#v", doc)
	}
}

// ADR-046: a seeded record is drafted by an agent and adopted by the owner in
// the console; the console never edits the text itself.
func TestConsoleAdoptsOrDiscardsADraftAndQueuesTheDraftingJob(t *testing.T) {
	s, slug := templateStore(t)
	h := New(s, nil)
	id := "EVO-constitution-" + slug
	// Draft it for me: the record's project gets the drafting job at once; a second click finds it open.
	status, data := additionsRequest(t, h, "POST", "/api/v1/records/"+id+"/draft-job", `{}`)
	if status != 200 {
		t.Fatalf("draft-job: %d %#v", status, data)
	}
	job := data["job"].(map[string]any)
	if job["status"] != "queued" || data["created"] != true || data["reason"] != "" {
		t.Fatalf("draft-job payload: %#v", data)
	}
	var kind, source, createdBy string
	if err := s.DB.QueryRow("SELECT kind, source, created_by FROM jobs WHERE id = ?", job["id"]).Scan(&kind, &source, &createdBy); err != nil {
		t.Fatal(err)
	}
	if kind != "reflect" || !strings.HasPrefix(source, "draft:"+slug+":") || createdBy != store.HumanAgent {
		t.Fatalf("queued job: %s %s %s", kind, source, createdBy)
	}
	status, again := additionsRequest(t, h, "POST", "/api/v1/records/"+id+"/draft-job", `{}`)
	if status != 200 || again["created"] != false || again["job"].(map[string]any)["id"] != job["id"] || !strings.Contains(again["reason"].(string), "a draft job is queued") {
		t.Fatalf("second draft-job: %d %#v", status, again)
	}
	_, rows := additionsRequest(t, h, "GET", "/api/v1/projects", "")
	for _, item := range rows["projects"].([]any) {
		row := item.(map[string]any)
		if row["slug"] != slug {
			continue
		}
		ref, _ := row["constitution"].(map[string]any)["draft_job"].(map[string]any)
		if ref == nil || ref["id"] != job["id"] || ref["status"] != "queued" || row["constitution"].(map[string]any)["draft"] != nil {
			t.Fatalf("row while drafting: %#v", row["constitution"])
		}
	}
	// The worker files the draft; the row and the evolution document show it, the record is untouched.
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'done' WHERE id = ?", job["id"]); err != nil {
		t.Fatal(err)
	}
	body := "# Lamplit Labs Evolution Constitution\n\nThe Lamplit Labs loop edits `src/`.\n"
	if _, err := records.Draft(s, id, records.DraftInput{Body: body, Author: "aurora", Note: "from the README", JobID: job["id"].(string)}); err != nil {
		t.Fatal(err)
	}
	_, rows = additionsRequest(t, h, "GET", "/api/v1/projects", "")
	for _, item := range rows["projects"].([]any) {
		row := item.(map[string]any)
		if row["slug"] != slug {
			continue
		}
		draft, _ := row["constitution"].(map[string]any)["draft"].(map[string]any)
		if draft == nil || draft["author"] != "aurora" || draft["note"] != "from the README" || draft["base_rev"] != float64(1) || draft["job_id"] != job["id"] || draft["body"] != nil ||
			row["constitution"].(map[string]any)["draft_job"] != nil || row["constitution"].(map[string]any)["seeded"] != true {
			t.Fatalf("row with a draft: %#v", row["constitution"])
		}
	}
	_, doc := additionsRequest(t, h, "GET", "/api/v1/projects/"+slug+"/evolution", "")
	if draft := doc["constitution"].(map[string]any)["draft"].(map[string]any); draft["body"] != body || draft["author"] != "aurora" {
		t.Fatalf("evolution draft: %#v", draft)
	}
	if status, rec := additionsRequest(t, h, "GET", "/api/v1/records/"+id, ""); status != 200 || rec["draft"].(map[string]any)["body"] != body || rec["body"] == body {
		t.Fatalf("record with a draft: %d %#v", status, rec)
	}
	// With the constitution drafted, the button drafts what is left: the evaluators. Once both wait, there is nothing to draft.
	status, data = additionsRequest(t, h, "POST", "/api/v1/records/"+id+"/draft-job", `{}`)
	var title string
	if err := s.DB.QueryRow("SELECT title FROM jobs WHERE id = ?", data["job"].(map[string]any)["id"]).Scan(&title); err != nil {
		t.Fatal(err)
	}
	if status != 200 || data["created"] != true || title != "Draft the evaluators of "+slug+" from its repository" {
		t.Fatalf("draft-job with the constitution drafted: %d %#v %q", status, data, title)
	}
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'done' WHERE id = ?", data["job"].(map[string]any)["id"]); err != nil {
		t.Fatal(err)
	}
	if _, err := records.Draft(s, "EVO-evaluators-"+slug, records.DraftInput{Body: "# Lamplit Labs Evaluators\n\nThe Lamplit Labs loop scores `make test`.\n", Author: "aurora"}); err != nil {
		t.Fatal(err)
	}
	if status, data := additionsRequest(t, h, "POST", "/api/v1/records/"+id+"/draft-job", `{}`); status != 409 || !strings.Contains(str(data["why"]), "draft waiting") {
		t.Fatalf("draft-job with both drafts waiting: %d %#v", status, data)
	}
	// Discard, draft again, adopt with the owner's words.
	if status, data := additionsRequest(t, h, "POST", "/api/v1/records/"+id+"/discard", `{"words":"wrong paths"}`); status != 200 || data["discarded"] != true {
		t.Fatalf("discard: %d %#v", status, data)
	}
	if d, _ := s.GetDraft(id); d != nil {
		t.Fatal("discard left the draft")
	}
	if _, err := records.Draft(s, id, records.DraftInput{Body: body, Author: "aurora"}); err != nil {
		t.Fatal(err)
	}
	status, data = additionsRequest(t, h, "POST", "/api/v1/records/"+id+"/adopt", `{"words":"paths checked"}`)
	if status != 200 || data["record"].(map[string]any)["current_rev"] != float64(2) {
		t.Fatalf("adopt: %d %#v", status, data)
	}
	head, rev, err := s.GetRecord(id, 0)
	if err != nil {
		t.Fatal(err)
	}
	if rev.Body != body || rev.Author != store.HumanAgent || rev.Note != "adopted the draft by aurora: paths checked (via console)" || head.CurrentRev != 2 {
		t.Fatalf("adopted revision: %+v", rev)
	}
	_, rows = additionsRequest(t, h, "GET", "/api/v1/projects", "")
	for _, item := range rows["projects"].([]any) {
		row := item.(map[string]any)
		if row["slug"] != slug {
			continue
		}
		c := row["constitution"].(map[string]any)
		if c["seeded"] != false || c["rev"] != float64(2) || c["draft"] != nil {
			t.Fatalf("row after adoption: %#v", c)
		}
	}
	if status, data := additionsRequest(t, h, "POST", "/api/v1/records/"+id+"/adopt", `{}`); status == 200 {
		t.Fatalf("adopt without a draft: %d %#v", status, data)
	}
	if status, _ := additionsRequest(t, h, "GET", "/api/v1/records/"+id+"/adopt", ""); status != 405 {
		t.Fatalf("GET adopt: %d", status)
	}
}
