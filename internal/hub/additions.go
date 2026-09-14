package hub

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"net/http"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

// Keep revision/ID allocation and state checks together across owner requests.
var additionWrites sync.Mutex

// additions serves the owner console without the legacy CLI identity headers.
func (h *Handler) additions(w http.ResponseWriter, r *http.Request, route string) bool {
	parts := strings.Split(strings.Trim(route, "/"), "/")
	switch parts[0] {
	case "records":
		h.additionRecords(w, r, parts)
	case "constitution":
		if len(parts) != 1 {
			additionNotFound(w)
		} else {
			h.additionConstitution(w, r)
		}
	case "events":
		h.additionEvents(w, r, parts)
	case "tasks":
		h.additionTasks(w, r, parts)
	case "jobs":
		h.additionJobs(w, r, parts)
	case "messages":
		h.additionMessages(w, r, parts)
	case "compute":
		h.additionCompute(w, r, parts)
	case "projects":
		return h.additionProjects(w, r, parts)
	case "folders":
		h.additionFolders(w, r, parts)
	case "heldout":
		h.additionHeldout(w, r, parts)
	case "harness":
		h.additionHarness(w, r, parts)
	case "service":
		h.additionService(w, r, parts)
	default:
		return false
	}
	return true
}

func additionNotFound(w http.ResponseWriter) {
	writeError(w, http.StatusNotFound, "Unknown API route", "That owner-console route does not exist.", "Use an /api/v1 route.")
}

func additionMethod(w http.ResponseWriter, r *http.Request, methods ...string) bool {
	for _, method := range methods {
		if r.Method == method {
			return true
		}
	}
	w.Header().Set("Allow", strings.Join(methods, ", "))
	writeError(w, http.StatusMethodNotAllowed, "Method not allowed", "This route does not support "+r.Method+".", "Use "+strings.Join(methods, " or ")+".")
	return false
}

func (h *Handler) additionProject(r *http.Request) string {
	if h.Project != "" {
		return h.Project
	}
	return r.URL.Query().Get("project")
}

func (h *Handler) additionAvailable(w http.ResponseWriter, r *http.Request, tables ...string) bool {
	if h.Store == nil || h.Store.DB == nil {
		code := http.StatusServiceUnavailable
		if r.Method == http.MethodPost {
			code = http.StatusConflict
		}
		writeError(w, code, "The store is unavailable", "The hub has no open PULSE store.", "Run pulse init, then restart pulse hub.")
		return false
	}
	for _, table := range tables {
		var exists int
		err := h.Store.DB.QueryRowContext(r.Context(), "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?", table).Scan(&exists)
		if err != nil {
			additionFailure(w, err)
			return false
		}
		if exists == 0 {
			what := strings.ReplaceAll(table, "_", " ") + " are not available yet"
			if table == "task_revisions" {
				what = "tasks are not available yet"
			}
			writeError(w, http.StatusServiceUnavailable, what, "This store does not have the required additive tables.", "Update PULSE and restart pulse hub.")
			return false
		}
	}
	return true
}

func additionFailure(w http.ResponseWriter, err error) {
	code := http.StatusConflict
	what := err.Error()
	why, fix := "The store could not complete this request.", "Refresh the page and check the request."
	var input *store.InputError
	var command *cli.Error
	switch {
	case errors.Is(err, store.ErrNoRecord):
		code = http.StatusNotFound
		what, why, fix = "no such record", "No record matches the requested ID and project.", "Select an existing record from the records list."
	case errors.Is(err, sql.ErrNoRows), errors.Is(err, tasks.ErrNotFound):
		code = http.StatusNotFound
		what, why, fix = "not found", "The requested item does not exist in this project.", "Select an existing item from its list."
	case errors.As(err, &input):
		code = http.StatusBadRequest
		why, fix = "The request contains an invalid value.", "Correct the value and submit again."
	case errors.As(err, &command):
		if command.Code == 2 {
			code = http.StatusBadRequest
		}
		if strings.HasPrefix(command.What, "No task ") || strings.HasPrefix(command.What, "No message ") || strings.HasPrefix(command.What, "No compute request ") ||
			strings.HasPrefix(command.What, "No project ") || strings.HasPrefix(command.What, "No executor ") {
			code = http.StatusNotFound
		}
		writeError(w, code, command.What, command.Why, command.Fix)
		return
	}
	writeError(w, code, what, why, fix)
}

func additionString(w http.ResponseWriter, payload map[string]any, key string, required bool) (string, bool) {
	value, present := payload[key]
	if !present && !required {
		return "", true
	}
	text, ok := value.(string)
	if !ok || (required && strings.TrimSpace(text) == "") || strings.ContainsRune(text, '\x00') {
		kind := "string"
		if required {
			kind = "non-empty string"
		}
		writeError(w, http.StatusBadRequest, "Invalid "+key, key+" must be a "+kind+".", "Send a JSON object with a valid "+key+".")
		return "", false
	}
	return text, true
}

func additionPriority(w http.ResponseWriter, payload map[string]any) (int, bool) {
	value, exists := payload["priority"]
	if !exists {
		return 0, true
	}
	var priority int64
	var err error
	switch value := value.(type) {
	case json.Number:
		priority, err = value.Int64()
	case float64:
		if math.Trunc(value) != value || value > math.MaxInt32 || value < math.MinInt32 {
			err = errors.New("priority is not an integer")
		} else {
			priority = int64(value)
		}
	default:
		err = errors.New("priority is not a number")
	}
	if err != nil || priority > math.MaxInt32 || priority < math.MinInt32 {
		writeError(w, http.StatusBadRequest, "Invalid priority", "priority must be a signed 32-bit JSON integer.", "Send a whole number for priority.")
		return 0, false
	}
	return int(priority), true
}

func (h *Handler) additionRows(r *http.Request, query string, args ...any) ([]map[string]any, error) {
	return h.queryRows(r.Context(), query, args...)
}

func (h *Handler) queryRows(ctx context.Context, query string, args ...any) ([]map[string]any, error) {
	rows, err := h.Store.DB.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	columns, err := rows.Columns()
	if err != nil {
		return nil, err
	}
	out := []map[string]any{}
	for rows.Next() {
		values, targets := make([]any, len(columns)), make([]any, len(columns))
		for i := range values {
			targets[i] = &values[i]
		}
		if err := rows.Scan(targets...); err != nil {
			return nil, err
		}
		row := make(map[string]any, len(columns))
		for i, key := range columns {
			value := values[i]
			if raw, ok := value.([]byte); ok {
				value = string(raw)
			}
			row[key] = value
		}
		out = append(out, row)
	}
	return out, rows.Err()
}

const additionTaskColumns = `t.id,t.project,t.title,t.status,t.priority,t.lead,t.job_id,t.created_by,t.created_at,t.updated_at,t.current_rev,rv.brief`
const additionTaskFrom = ` FROM tasks t JOIN task_revisions rv ON rv.task_id=t.id AND rv.rev=t.current_rev`

func (h *Handler) additionTask(r *http.Request, id string) (map[string]any, error) {
	rows, err := h.additionRows(r, "SELECT "+additionTaskColumns+additionTaskFrom+" WHERE t.id=?", id)
	if err != nil {
		return nil, err
	}
	project := h.additionProject(r)
	if len(rows) == 0 || (project != "" && project != rows[0]["project"]) {
		return nil, cli.Errorf(2, "No task "+strconv.Quote(id), "No task matches this ID in the selected project.", "Select an existing task from the task list.")
	}
	if err := h.additionTaskWork(rows); err != nil {
		return nil, err
	}
	return rows[0], nil
}

func (h *Handler) additionTaskWork(rows []map[string]any) error {
	ts := make([]tasks.Task, 0, len(rows))
	for _, row := range rows {
		ts = append(ts, tasks.Task{
			ID:      additionRowString(row, "id"),
			Project: additionRowString(row, "project"),
			JobID:   additionRowString(row, "job_id"),
		})
	}
	work, err := tasks.WorkFor(h.Store, ts)
	if err != nil {
		return err
	}
	for _, row := range rows {
		w := work[additionRowString(row, "id")]
		row["jobs_total"] = w.Total
		row["jobs_queued"] = w.Queued
		row["jobs_blocked"] = w.Blocked
		row["jobs_running"] = w.Running
		row["jobs_done"] = w.Done
		row["jobs_failed"] = w.Failed
		if w.Working == nil {
			w.Working = []tasks.Worker{}
		}
		row["working"] = w.Working
	}
	return nil
}

func additionRowString(row map[string]any, key string) string {
	if text, ok := row[key].(string); ok {
		return text
	}
	return ""
}

func (h *Handler) additionTasks(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) > 3 || (len(parts) == 3 && parts[2] != "amend" && parts[2] != "cancel") {
		additionNotFound(w)
		return
	}
	methods := []string{http.MethodGet}
	if len(parts) == 1 {
		methods = append(methods, http.MethodPost)
	} else if len(parts) == 3 {
		methods = []string{http.MethodPost}
	}
	if !additionMethod(w, r, methods...) || !h.additionAvailable(w, r, "tasks", "task_revisions", "messages") {
		return
	}
	if r.Method == http.MethodPost {
		h.additionTaskWrite(w, r, parts)
		return
	}
	if len(parts) == 1 {
		query, args := "SELECT "+additionTaskColumns+additionTaskFrom+" WHERE 1=1", []any{}
		if project := h.additionProject(r); project != "" {
			query += " AND t.project=?"
			args = append(args, project)
		}
		if status := r.URL.Query().Get("status"); status != "" {
			if !slices.Contains(tasks.Statuses, status) {
				writeError(w, http.StatusBadRequest, "Invalid task status", "The requested status is not a task state.", "Use open, planned, in_progress, review, done, or cancelled.")
				return
			}
			query += " AND t.status=?"
			args = append(args, status)
		}
		query += " ORDER BY t.priority DESC,t.created_at,t.id"
		tasks, err := h.additionRows(r, query, args...)
		if err != nil {
			additionFailure(w, err)
			return
		}
		if err := h.additionTaskWork(tasks); err != nil {
			additionFailure(w, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{"tasks": tasks})
		return
	}
	task, err := h.additionTask(r, parts[1])
	if err != nil {
		additionFailure(w, err)
		return
	}
	revisions, err := h.additionRows(r, "SELECT rev,title,brief,note,author,at FROM task_revisions WHERE task_id=? ORDER BY rev", parts[1])
	if err != nil {
		additionFailure(w, err)
		return
	}
	messages, err := h.additionRows(r, "SELECT id,project,task_id,author,body,at FROM messages WHERE task_id=? ORDER BY rowid", parts[1])
	if err != nil {
		additionFailure(w, err)
		return
	}
	jobs, err := h.additionRows(r, `SELECT id,project,kind,tier,title,brief,source,priority,status,created_at,created_by,
		lease_agent AS agent,lease_until,generation,attempts,finished_at,result,branch,
		(SELECT hs.id FROM harness_sessions hs WHERE hs.job_id = jobs.id ORDER BY hs.started_at DESC LIMIT 1) AS harness_session
		FROM jobs WHERE project=? AND (source=? OR (? != '' AND id=?)) ORDER BY created_at,id`, task["project"], "task:"+parts[1], task["job_id"], task["job_id"])
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"task": task, "revisions": revisions, "messages": messages, "jobs": jobs})
}

func (h *Handler) additionMessages(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) != 1 {
		additionNotFound(w)
		return
	}
	if !additionMethod(w, r, http.MethodGet, http.MethodPost) || !h.additionAvailable(w, r, "messages", "tasks") {
		return
	}
	if r.Method == http.MethodPost {
		h.additionMessageWrite(w, r)
		return
	}
	query, args := "SELECT id,project,task_id,author,body,at FROM messages WHERE 1=1", []any{}
	if project := h.additionProject(r); project != "" {
		query += " AND project=?"
		args = append(args, project)
	}
	if task := r.URL.Query().Get("task"); task != "" {
		query += " AND task_id=?"
		args = append(args, task)
	}
	if since := r.URL.Query().Get("since"); since != "" {
		// Numeric suffixes are not zero-padded; MSG-...-10 follows MSG-...-9.
		query += " AND rowid>(SELECT rowid FROM messages WHERE id=?)"
		args = append(args, since)
	}
	messages, err := h.additionRows(r, query+" ORDER BY rowid", args...)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"messages": messages})
}

func (h *Handler) additionCompute(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) == 4 && parts[1] == "executors" && parts[3] == "remove" {
		h.additionExecutorRemove(w, r, parts[2])
		return
	}
	if len(parts) != 1 && (len(parts) != 3 || (parts[2] != "approve" && parts[2] != "deny")) {
		additionNotFound(w)
		return
	}
	method := http.MethodGet
	if len(parts) == 3 {
		method = http.MethodPost
	}
	if !additionMethod(w, r, method) || !h.additionAvailable(w, r, "compute_requests") {
		return
	}
	if r.Method == http.MethodPost {
		h.additionComputeWrite(w, r, parts[1], parts[2])
		return
	}
	query := "SELECT id,project,requested_by,size,hours,reason,status,decided_by,executor,at,updated_at FROM compute_requests"
	args := []any{}
	if project := h.additionProject(r); project != "" {
		query += " WHERE project=?"
		args = append(args, project)
	}
	requests, err := h.additionRows(r, query+" ORDER BY at,id", args...)
	if err != nil {
		additionFailure(w, err)
		return
	}
	executors, err := h.additionRows(r, "SELECT * FROM executors ORDER BY name")
	if err != nil {
		additionFailure(w, err)
		return
	}
	caps, err := tasks.GetCaps(h.Store)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"requests": requests, "executors": executors, "caps": caps})
}

func (h *Handler) additionTaskWrite(w http.ResponseWriter, r *http.Request, parts []string) {
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	if len(parts) == 1 {
		project, ok := additionString(w, payload, "project", h.Project == "")
		if !ok {
			return
		}
		if project == "" {
			project = h.Project
		}
		if h.Project != "" && project != h.Project {
			writeError(w, http.StatusBadRequest, "Project mismatch", "This hub serves a different project.", "Send the task to the selected project.")
			return
		}
		title, ok := additionString(w, payload, "title", true)
		if !ok {
			return
		}
		brief, ok := additionString(w, payload, "brief", true)
		if !ok {
			return
		}
		priority, ok := additionPriority(w, payload)
		if !ok {
			return
		}
		created, err := tasks.New(h.Store, project, title, brief, priority, store.HumanAgent)
		if err != nil {
			additionFailure(w, err)
			return
		}
		task, err := h.additionTask(r, created.ID)
		if err != nil {
			additionFailure(w, err)
			return
		}
		writeJSON(w, http.StatusCreated, map[string]any{"task": task})
		return
	}
	var title, brief, note, reason string
	if parts[2] == "amend" {
		_, hasTitle := payload["title"]
		title, ok = additionString(w, payload, "title", hasTitle)
		if !ok {
			return
		}
		brief, ok = additionString(w, payload, "brief", true)
		if !ok {
			return
		}
		note, ok = additionString(w, payload, "note", false)
	} else {
		reason, ok = additionString(w, payload, "reason", false)
	}
	if !ok {
		return
	}
	task, err := h.additionTask(r, parts[1])
	if err != nil {
		additionFailure(w, err)
		return
	}
	if task["status"] == "done" || task["status"] == "cancelled" {
		writeError(w, http.StatusConflict, "Task is closed", "Completed or cancelled work cannot be amended.", "Create a new task instead.")
		return
	}
	if parts[2] == "amend" {
		_, err = tasks.Amend(h.Store, parts[1], title, brief, note, store.HumanAgent)
	} else {
		_, err = tasks.Cancel(h.Store, parts[1], reason, store.HumanAgent)
	}
	if err != nil {
		additionFailure(w, err)
		return
	}
	task, err = h.additionTask(r, parts[1])
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"task": task})
}

func (h *Handler) additionMessageWrite(w http.ResponseWriter, r *http.Request) {
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	project, ok := additionString(w, payload, "project", h.Project == "")
	if !ok {
		return
	}
	if project == "" {
		project = h.Project
	}
	if h.Project != "" && project != h.Project {
		writeError(w, http.StatusBadRequest, "Project mismatch", "This hub serves a different project.", "Send the message to the selected project.")
		return
	}
	taskID, ok := additionString(w, payload, "task_id", false)
	if !ok {
		return
	}
	body, ok := additionString(w, payload, "body", true)
	if !ok {
		return
	}
	message, err := tasks.SendMessage(h.Store, project, taskID, store.HumanAgent, body)
	if err != nil {
		additionFailure(w, err)
		return
	}
	messages, err := h.additionRows(r, "SELECT id,project,task_id,author,body,at FROM messages WHERE id=?", message.ID)
	if err != nil {
		additionFailure(w, err)
		return
	}
	if len(messages) == 0 {
		additionFailure(w, sql.ErrNoRows)
		return
	}
	writeJSON(w, http.StatusCreated, map[string]any{"message": messages[0]})
}

func (h *Handler) additionComputeWrite(w http.ResponseWriter, r *http.Request, id, action string) {
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	words, ok := additionString(w, payload, "words", false)
	if !ok {
		return
	}
	query := "SELECT id,project,requested_by,size,hours,reason,status,decided_by,executor,at,updated_at FROM compute_requests WHERE id=?"
	requests, err := h.additionRows(r, query, id)
	if err != nil {
		additionFailure(w, err)
		return
	}
	if len(requests) == 0 || (h.Project != "" && requests[0]["project"] != h.Project) {
		additionFailure(w, sql.ErrNoRows)
		return
	}
	if requests[0]["status"] != "pending" {
		writeError(w, http.StatusConflict, "Compute request is already decided", "Only pending requests may be approved or denied.", "Refresh the compute requests.")
		return
	}
	if action == "approve" {
		_, err = tasks.Approve(h.Store, id, words, store.HumanAgent)
	} else {
		_, err = tasks.Deny(h.Store, id, words, store.HumanAgent)
	}
	if err != nil {
		additionFailure(w, err)
		return
	}
	requests, err = h.additionRows(r, query, id)
	if err != nil || len(requests) == 0 {
		if err == nil {
			err = sql.ErrNoRows
		}
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"request": requests[0]})
}

func additionInteger(w http.ResponseWriter, value, name string, fallback, maximum int64) (int64, bool) {
	if value == "" {
		return fallback, true
	}
	n, err := strconv.ParseInt(value, 10, 64)
	if err != nil || n < 0 || (maximum > 0 && n > maximum) {
		writeError(w, http.StatusBadRequest, "Invalid "+name, name+" must be a non-negative integer within the supported range.", "Use a valid "+name+" query parameter.")
		return 0, false
	}
	return n, true
}

func additionRecord(rec store.Record) map[string]any {
	return map[string]any{
		"id": rec.ID, "project": rec.Project, "kind": rec.Kind, "number": rec.Number,
		"slug": rec.Slug, "title": rec.Title, "status": rec.Status, "tier": rec.Tier,
		"areas": rec.Areas, "tags": rec.Tags, "delivery": rec.Delivery, "delivered": rec.Delivered,
		"date": rec.Date, "path": rec.Path, "updated_at": rec.UpdatedAt, "current_rev": rec.CurrentRev,
	}
}

func (h *Handler) additionRecords(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) == 3 {
		h.additionRecordDecision(w, r, parts[1], parts[2])
		return
	}
	if len(parts) > 2 {
		additionNotFound(w)
		return
	}
	if !additionMethod(w, r, http.MethodGet) || !h.additionAvailable(w, r, "records", "record_revisions") {
		return
	}
	if len(parts) == 1 {
		query := r.URL.Query()
		records, err := h.Store.ListRecords(store.RecordFilter{
			Project: h.additionProject(r), Kind: query.Get("kind"), Status: query.Get("status"), Area: query.Get("area"),
		})
		if err != nil {
			additionFailure(w, err)
			return
		}
		out := make([]map[string]any, 0, len(records))
		for _, rec := range records {
			out = append(out, additionRecord(rec))
		}
		writeJSON(w, http.StatusOK, map[string]any{"records": out})
		return
	}
	rev, ok := additionInteger(w, r.URL.Query().Get("rev"), "rev", 0, 0)
	if !ok {
		return
	}
	rec, revision, err := h.Store.GetRecord(parts[1], int(rev))
	if err != nil {
		if rec != nil {
			writeError(w, http.StatusNotFound, "Revision not found", err.Error(), "Select an existing revision.")
		} else {
			additionFailure(w, err)
		}
		return
	}
	if project := h.additionProject(r); project != "" && project != rec.Project {
		additionFailure(w, store.ErrNoRecord)
		return
	}
	revisions, err := h.Store.Revisions(rec.ID)
	if err != nil {
		additionFailure(w, err)
		return
	}
	history := make([]map[string]any, 0, len(revisions))
	for _, revision := range revisions {
		history = append(history, map[string]any{"rev": revision.Rev, "author": revision.Author, "at": revision.At, "note": revision.Note})
	}
	writeJSON(w, http.StatusOK, map[string]any{"record": additionRecord(*rec), "frontmatter": revision.Frontmatter, "body": revision.Body, "revisions": history})
}

func (h *Handler) additionConstitution(w http.ResponseWriter, r *http.Request) {
	if !additionMethod(w, r, http.MethodGet) || !h.additionAvailable(w, r, "records", "record_revisions") {
		return
	}
	project := h.additionProject(r)
	if project == "" {
		brain, err := h.Store.Brain()
		if err != nil {
			additionFailure(w, err)
			return
		}
		if brain != nil {
			project = brain.Slug
		}
	}
	if project == "" {
		writeError(w, http.StatusBadRequest, "A project is required", "There is no default framework project.", "Select a project with ?project=<slug>.")
		return
	}
	if project != store.CommonsSlug {
		entry, err := h.Store.Project(project)
		if err != nil {
			additionFailure(w, err)
			return
		}
		if entry == nil {
			writeError(w, http.StatusNotFound, "no such project", "The requested project is not registered.", "Select a registered project.")
			return
		}
	}
	records, err := h.Store.ListRecords(store.RecordFilter{Project: project, Kind: "evolution", Slug: "constitution"})
	if err != nil {
		additionFailure(w, err)
		return
	}
	if len(records) == 0 {
		writeJSON(w, http.StatusOK, map[string]any{"record": nil, "body": "", "amendments": []map[string]any{}})
		return
	}
	rec, revision, err := h.Store.GetRecord(records[0].ID, 0)
	if err != nil {
		additionFailure(w, err)
		return
	}
	amendments := []map[string]any{}
	if h.Source != nil {
		snapshot, err := h.snapshot(project, "")
		if err != nil {
			additionFailure(w, err)
			return
		}
		for _, amendment := range snapshot.Amendments {
			status, _ := amendment["status"].(string)
			amendments = append(amendments, map[string]any{
				"id": amendment["id"], "status": status, "votes_up": amendment["up"],
				"votes_down": amendment["down"], "quorum": amendment["quorum"],
				"passed":    strings.HasPrefix(status, "passed"),
				"protected": additionProtected(amendment, revision.Body),
			})
		}
	}
	writeJSON(w, http.StatusOK, map[string]any{"record": additionRecord(*rec), "body": revision.Body, "amendments": amendments})
}

func additionProtected(amendment map[string]any, constitution string) bool {
	if protected, ok := amendment["protected"].(bool); ok {
		return protected
	}
	status, _ := amendment["status"].(string)
	if strings.Contains(strings.ToLower(status), "protected") {
		return true
	}
	protected := "Hard stops, Kill switch, Amendments, frontmatter"
	for _, line := range strings.Split(constitution, "\n") {
		cells := strings.Split(strings.Trim(line, " \t|"), "|")
		if len(cells) > 1 && strings.EqualFold(strings.Trim(strings.TrimSpace(cells[0]), "*` "), "Not amendable by vote") {
			protected = cells[1]
			break
		}
	}
	sections, _ := amendment["section"].(string)
	for _, section := range strings.Split(sections, "|") {
		for _, name := range strings.Split(protected, ",") {
			name = strings.Trim(name, " `\t")
			if name != "" && strings.EqualFold(strings.Trim(section, " `\t"), name) {
				return true
			}
		}
	}
	return false
}

type additionEvent struct {
	ID      int64  `json:"id"`
	At      string `json:"at"`
	Actor   string `json:"actor"`
	Action  string `json:"action"`
	Subject string `json:"subject"`
	Detail  any    `json:"detail"`
}

// additionReadEvents returns the newest limit events after since, oldest
// first: a cursor gets what it missed, no cursor gets the recent window, and
// neither walks the log from its first row.
func (h *Handler) additionReadEvents(r *http.Request, since int64, limit int64) ([]additionEvent, error) {
	events, err := bus.Latest(h.Store, since, h.additionEventFilter(r), int(limit))
	if err != nil {
		return nil, err
	}
	out := make([]additionEvent, 0, len(events))
	redact := bus.Redactor(h.Store)
	for _, event := range events {
		out = append(out, additionBusEvent(redact(event)))
	}
	return out, nil
}

func additionBusEvent(event bus.Event) additionEvent {
	out := additionEvent{ID: event.ID, At: event.At, Actor: event.Actor, Action: event.Action, Subject: event.Subject}
	if json.Unmarshal([]byte(event.Detail), &out.Detail) != nil {
		out.Detail = event.Detail
	}
	return privateSettingEvent(out)
}

func (h *Handler) additionEventFilter(r *http.Request) bus.Filter {
	query := r.URL.Query()
	var actions []string
	for _, key := range []string{"kind", "action", "kinds", "actions"} {
		for _, value := range query[key] {
			for _, action := range strings.Split(value, ",") {
				if action = strings.TrimSpace(action); action != "" {
					actions = append(actions, action)
				}
			}
		}
	}
	return bus.Filter{Actions: actions, Project: h.additionProject(r), Subject: query.Get("subject")}
}

func (h *Handler) additionEvents(w http.ResponseWriter, r *http.Request, parts []string) {
	stream := len(parts) == 2 && parts[1] == "stream"
	if len(parts) != 1 && !stream {
		additionNotFound(w)
		return
	}
	if !additionMethod(w, r, http.MethodGet) || !h.additionAvailable(w, r, "events") {
		return
	}
	cursor := r.URL.Query().Get("since")
	if cursor == "" && stream {
		cursor = r.Header.Get("Last-Event-ID")
	}
	since, ok := additionInteger(w, cursor, "since", 0, 0)
	if !ok {
		return
	}
	limit, ok := additionInteger(w, r.URL.Query().Get("limit"), "limit", 100, 1000)
	if !ok {
		return
	}
	if limit == 0 {
		writeError(w, http.StatusBadRequest, "Invalid limit", "limit must be greater than zero.", "Use a limit between 1 and 1000.")
		return
	}
	// The stream replays only for a cursor: a fresh connection starts at the
	// head and leaves the recent window to GET /events, so a page load never
	// receives the whole log. The head is read before the replay so an event
	// landing in between is tailed rather than lost.
	var (
		head   int64
		events []additionEvent
		err    error
	)
	if stream {
		if head, err = bus.LastID(h.Store); err != nil {
			additionFailure(w, err)
			return
		}
	}
	if !stream || since > 0 {
		if events, err = h.additionReadEvents(r, since, limit); err != nil {
			additionFailure(w, err)
			return
		}
	}
	if !stream {
		writeJSON(w, http.StatusOK, map[string]any{"events": events})
		return
	}
	flusher, ok := w.(http.Flusher)
	if !ok {
		writeError(w, http.StatusServiceUnavailable, "Streaming is unavailable", "This connection cannot flush events.", "Use /api/v1/events for polling.")
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("X-Accel-Buffering", "no")
	controller := http.NewResponseController(w)
	// Refresh the normal server deadline while bounding slow-client writes.
	_ = controller.SetWriteDeadline(time.Now().Add(30 * time.Second))
	if _, err := fmt.Fprint(w, ": connected\n\n"); err != nil {
		return
	}
	flusher.Flush()
	send := func(event additionEvent) bool {
		data, err := json.Marshal(event)
		if err != nil {
			return false
		}
		_ = controller.SetWriteDeadline(time.Now().Add(30 * time.Second))
		action := strings.NewReplacer("\r", "", "\n", "").Replace(event.Action)
		if _, err := fmt.Fprintf(w, "id: %d\nevent: %s\ndata: %s\n\n", event.ID, action, data); err != nil {
			return false
		}
		flusher.Flush()
		return true
	}
	for _, event := range events {
		if !send(event) {
			return
		}
		if event.ID > head {
			head = event.ID
		}
	}
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	tail := bus.Tail(ctx, h.Store, head, h.additionEventFilter(r), time.Second)
	heartbeat := time.NewTicker(15 * time.Second)
	defer heartbeat.Stop()
	for {
		select {
		case <-r.Context().Done():
			return
		case event, open := <-tail:
			// Project filters can hide setting events, so refresh private values for every event.
			if !open || !send(additionBusEvent(bus.Redactor(h.Store)(event))) {
				return
			}
		case <-heartbeat.C:
			_ = controller.SetWriteDeadline(time.Now().Add(30 * time.Second))
			if _, err := fmt.Fprint(w, ": heartbeat\n\n"); err != nil {
				return
			}
			flusher.Flush()
		}
	}
}
