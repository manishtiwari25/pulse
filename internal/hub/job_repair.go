package hub

import (
	"net/http"
	"strconv"

	"github.com/manishtiwari25/pulse/internal/runner"
)

const jobRepairHeader = "X-Pulse-Job-Repair"

// additionJobs owns /api/v1/jobs/failed and /api/v1/jobs/repair. The owner
// confirms explicit IDs, never a query or all-history retry.
func (h *Handler) additionJobs(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) != 2 || parts[0] != "jobs" || parts[1] != "failed" && parts[1] != "repair" {
		additionNotFound(w)
		return
	}
	method := http.MethodGet
	if parts[1] == "repair" {
		method = http.MethodPost
	}
	if !additionMethod(w, r, method) || !h.additionAvailable(w, r, "jobs", "projects", "runs", "events", "posts", "settings", "tasks") {
		return
	}
	project := r.URL.Query().Get("project")
	if r.Method == http.MethodGet {
		var ok bool
		if project, ok = h.jobRepairProject(w, project, ""); !ok {
			return
		}
		limit := runner.FailedJobsDefaultLimit
		if text, exists := r.URL.Query()["limit"]; exists {
			var err error
			if len(text) != 1 {
				writeError(w, 400, "Invalid limit", "Send one page size.", "Use limit=50.")
				return
			}
			limit, err = strconv.Atoi(text[0])
			if err != nil || limit < 1 || limit > runner.FailedJobsMaxLimit {
				writeError(w, 400, "Invalid limit", "limit must be between 1 and "+strconv.Itoa(runner.FailedJobsMaxLimit)+".", "Use limit=50.")
				return
			}
		}
		page, err := runner.ReadFailedJobs(h.Store, runner.FailedJobsQuery{
			Project: project, Limit: limit, Before: r.URL.Query().Get("before"),
		})
		if err != nil {
			additionFailure(w, err)
			return
		}
		writeJSON(w, http.StatusOK, page)
		return
	}
	if !loopbackHost(r) {
		writeError(w, http.StatusForbidden, "Job repair is available only on loopback", "The request named a host other than this machine.", "Open the hub on 127.0.0.1.")
		return
	}
	if r.Header.Get(jobRepairHeader) != "yes" {
		writeError(w, http.StatusForbidden, "missing "+jobRepairHeader+": yes", "An explicit confirmation header is required.", "Select up to five eligible jobs and confirm in the hub.")
		return
	}
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	for key := range payload {
		if key != "job_ids" && key != "project" {
			writeError(w, http.StatusBadRequest, "Unknown repair field: "+key, "Only job_ids and project are accepted; the actor is server-owned.", "Send explicit job IDs without an actor or override.")
			return
		}
	}
	bodyProject, ok := additionString(w, payload, "project", false)
	if !ok {
		return
	}
	if project, ok = h.jobRepairProject(w, project, bodyProject); !ok {
		return
	}
	ids, ok := payload["job_ids"].([]any)
	if !ok || len(ids) < 1 || len(ids) > runner.JobRepairMaxSelection {
		writeError(w, http.StatusBadRequest, "Invalid job_ids", "Select between 1 and "+strconv.Itoa(runner.JobRepairMaxSelection)+" explicit job IDs.", "Select a small set of eligible entries, not all history.")
		return
	}
	input := runner.RepairJobsInput{Project: project}
	for _, value := range ids {
		id, ok := value.(string)
		if !ok {
			writeError(w, http.StatusBadRequest, "Invalid job_ids", "Every job ID must be a string.", "Select existing jobs from the failed-job list.")
			return
		}
		input.JobIDs = append(input.JobIDs, id)
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	result, err := runner.RepairFailedJobs(h.Store, input)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, result)
}

func (h *Handler) jobRepairProject(w http.ResponseWriter, query, body string) (string, bool) {
	if h.Project != "" && (query != "" && query != h.Project || body != "" && body != h.Project) {
		writeError(w, http.StatusNotFound, "No such project in this hub", "This hub is scoped to a different project.", "Use this hub's project.")
		return "", false
	}
	if query != "" && body != "" && query != body {
		writeError(w, http.StatusBadRequest, "Conflicting projects", "The query and body name different projects.", "Use one project for the whole selection.")
		return "", false
	}
	if h.Project != "" {
		return h.Project, true
	}
	if body != "" {
		return body, true
	}
	return query, true
}
