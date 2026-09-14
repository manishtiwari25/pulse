package hub

import (
	"net/http"
	"strconv"

	"github.com/manishtiwari25/pulse/internal/codedelivery"
)

const (
	codeDeliveryDefaultLimit = 100
	codeDeliveryMaxLimit     = 200
)

func (h *Handler) additionCodeDeliveries(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) != 1 {
		additionNotFound(w)
		return
	}
	if !additionMethod(w, r, http.MethodGet) || !h.additionAvailable(w, r, "events", "jobs", "tasks", "posts", "agents") {
		return
	}
	project := h.additionProject(r)
	if !h.validCodeDeliveryProject(w, project) {
		return
	}
	limit, ok := codeDeliveryPositiveInt(w, r.URL.Query().Get("limit"), "limit", codeDeliveryDefaultLimit, codeDeliveryMaxLimit)
	if !ok {
		return
	}
	before, ok := codeDeliveryBefore(w, r.URL.Query().Get("before"))
	if !ok {
		return
	}
	report, err := codedelivery.Recent(r.Context(), h.Store, project, limit, before)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, report)
}

func (h *Handler) validCodeDeliveryProject(w http.ResponseWriter, project string) bool {
	if project == "" {
		return true
	}
	p, err := h.Store.Project(project)
	if err != nil {
		additionFailure(w, err)
		return false
	}
	if p == nil {
		writeError(w, http.StatusNotFound, "no such project", "No project matches the requested code-delivery filter.", "Select an existing project.")
		return false
	}
	return true
}

func codeDeliveryPositiveInt(w http.ResponseWriter, value, name string, fallback, maximum int) (int, bool) {
	if value == "" {
		return fallback, true
	}
	n, err := strconv.ParseInt(value, 10, 64)
	if err != nil || n <= 0 || n > int64(maximum) {
		writeError(w, http.StatusBadRequest, "Invalid "+name, name+" must be a positive integer within the supported range.", "Use a valid "+name+" query parameter.")
		return 0, false
	}
	return int(n), true
}

func codeDeliveryBefore(w http.ResponseWriter, value string) (int64, bool) {
	if value == "" {
		return 1<<63 - 1, true
	}
	n, err := strconv.ParseInt(value, 10, 64)
	if err != nil || n <= 0 {
		writeError(w, http.StatusBadRequest, "Invalid before", "before must be a positive event sequence cursor.", "Use a valid before query parameter.")
		return 0, false
	}
	return n, true
}
