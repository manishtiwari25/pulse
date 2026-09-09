package hub

import (
	"net/http"
	"sync"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Project lifecycle in the owner console (ADR-027): register, remove and
// restore a project, and remove an executor, through the functions the CLI
// runs. Only these paths are claimed; every other projects/... route still
// belongs to the legacy project API.

func lifecycleRoute(parts []string) bool {
	switch len(parts) {
	case 1:
		return true
	case 3:
		return parts[2] == "remove" || parts[2] == "restore"
	}
	return false
}

func (h *Handler) additionProjects(w http.ResponseWriter, r *http.Request, parts []string) bool {
	if !lifecycleRoute(parts) {
		return false
	}
	if len(parts) == 1 {
		if !additionMethod(w, r, http.MethodGet, http.MethodPost) || !h.additionAvailable(w, r, "projects") {
			return true
		}
		if r.Method == http.MethodPost {
			h.additionProjectRegister(w, r)
		} else {
			h.additionProjectList(w, r)
		}
		return true
	}
	if !additionMethod(w, r, http.MethodPost) || !h.additionAvailable(w, r, "projects") {
		return true
	}
	if parts[2] == "remove" {
		h.additionProjectRemove(w, r, parts[1])
	} else {
		h.additionProjectRestore(w, r, parts[1])
	}
	return true
}

func (h *Handler) additionProjectList(w http.ResponseWriter, r *http.Request) {
	projects, err := h.Store.Projects(false)
	if err != nil {
		additionFailure(w, err)
		return
	}
	rows := make([]map[string]any, len(projects))
	brain := ""
	var probes sync.WaitGroup
	for i := range projects {
		p := &projects[i]
		if p.Brain {
			brain = p.Slug
		}
		probes.Go(func() {
			row := map[string]any{
				"slug": p.Slug, "name": p.Name, "path": p.Path, "remote": p.Remote, "brain": p.Brain,
				"status": p.Status, "verify_cmd": p.VerifyCmd, "default_branch": p.DefaultBranch,
				"added_at": p.AddedAt, "added_by": p.AddedBy, "last_seen": p.LastSeen, "present": present(p),
			}
			for key, value := range projectGates(h.Store, p.Slug) {
				row[key] = value
			}
			rows[i] = row
		})
	}
	probes.Wait()
	writeJSON(w, http.StatusOK, map[string]any{"projects": rows, "brain": brain})
}

// projectGates is the loop's position for one project row (ADR-029), read
// the way evolve status reads it, with the loop's two records (ADR-030): id,
// revision and whether the text is still the seeded copy, or null when the
// record is absent, and the baseline job (ADR-031): baseline_job is {id,
// status} while the daemon's measurement is queued, leased or blocked, else
// null; baseline_failed_at is when the newest baseline job failed within the
// retry window, else "". A project the loop cannot describe (removed, or a
// store without the loop's tables) reports the zero values for the gates.
func projectGates(s *store.Store, slug string) map[string]any {
	gates := map[string]any{"fixtures_enabled": 0, "baseline": false, "evolve_level": "", "evolve_next": "",
		"constitution": loopRecordSummary(s, slug, "constitution"), "evaluators": loopRecordSummary(s, slug, "evaluators"),
		"baseline_job": nil, "baseline_failed_at": ""}
	status, err := evolve.Status(s, slug)
	if err != nil {
		return gates
	}
	gates["fixtures_enabled"], gates["baseline"] = status.PublicFixtures, status.Baseline != nil
	gates["evolve_level"], gates["evolve_next"] = status.Level, status.NextHumanStep
	if status.BaselineJob != nil {
		gates["baseline_job"] = object("id status", status.BaselineJob.ID, status.BaselineJob.Status)
	}
	gates["baseline_failed_at"] = status.BaselineFailedAt
	return gates
}

// loopRecordSummary is {id, rev, seeded, seeded_from} for the project's
// evolution record with that slug, or nil.
func loopRecordSummary(s *store.Store, slug, name string) any {
	rows, err := s.ListRecords(store.RecordFilter{Project: slug, Kind: "evolution", Slug: name})
	if err != nil || len(rows) == 0 {
		return nil
	}
	head, revision, err := s.GetRecord(rows[0].ID, 0)
	if err != nil {
		return nil
	}
	seeded, from := seededRecord(head, revision)
	return object("id rev seeded seeded_from", head.ID, head.CurrentRev, seeded, from)
}

func (h *Handler) additionProjectRegister(w http.ResponseWriter, r *http.Request) {
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	if _, present := payload["brain"]; present {
		writeError(w, http.StatusBadRequest, "The brain is chosen in the terminal", "The console does not move the brain flag.", "Run pulse init PATH --brain.")
		return
	}
	path, ok := additionString(w, payload, "path", true)
	if !ok {
		return
	}
	name, ok := additionString(w, payload, "name", false)
	if !ok {
		return
	}
	verify, ok := additionString(w, payload, "verify", false)
	if !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	result, err := board.Init(h.Store, path, name, "", false, verify, store.HumanAgent)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, result)
}

func (h *Handler) additionProjectRemove(w http.ResponseWriter, r *http.Request, slug string) {
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	dryRun, ok := additionDryRun(w, payload)
	if !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	preview, err := board.RemoveProject(h.Store, slug, store.HumanAgent, dryRun)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, preview)
}

func (h *Handler) additionProjectRestore(w http.ResponseWriter, r *http.Request, slug string) {
	if _, ok := readPayload(w, r); !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	p, err := h.Store.Project(slug)
	if err != nil {
		additionFailure(w, err)
		return
	}
	if p == nil {
		additionFailure(w, &cli.Error{Code: 2, What: "No project " + slug, Why: "only a registered project can be restored", Fix: "pulse projects"})
		return
	}
	if p.Status == "active" {
		additionFailure(w, &cli.Error{Code: 2, What: "Project " + slug + " is already active", Why: "restore registers an inactive project again", Fix: "pulse projects"})
		return
	}
	result, err := board.Init(h.Store, p.Path, "", "", false, "", store.HumanAgent)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, result)
}

func (h *Handler) additionExecutorRemove(w http.ResponseWriter, r *http.Request, name string) {
	if !additionMethod(w, r, http.MethodPost) || !h.additionAvailable(w, r, "executors") {
		return
	}
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	dryRun, ok := additionDryRun(w, payload)
	if !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	preview, err := board.RemoveExecutor(h.Store, name, store.HumanAgent, dryRun)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, preview)
}

// additionDryRun reads dry_run; a missing field previews, so applying always
// takes an explicit false.
func additionDryRun(w http.ResponseWriter, payload map[string]any) (bool, bool) {
	value, present := payload["dry_run"]
	if !present {
		return true, true
	}
	dryRun, ok := value.(bool)
	if !ok {
		writeError(w, http.StatusBadRequest, "Invalid dry_run", "dry_run must be true or false.", "Send dry_run:true to preview or dry_run:false to apply.")
		return false, false
	}
	return dryRun, true
}
