package hub

import (
	"encoding/json"
	"io"
	"mime"
	"net/http"
	"net/url"
	"strings"
	"unicode/utf8"

	"github.com/manishtiwari25/pulse/internal/goals"
	"github.com/manishtiwari25/pulse/internal/store"
)

func (h *Handler) additionProjectGoal(w http.ResponseWriter, r *http.Request, slug string) {
	if !additionMethod(w, r, http.MethodGet, http.MethodPost) || !h.additionAvailable(w, r, "projects", "settings") {
		return
	}
	var goal goals.Goal
	var err error
	actor := r.Header.Get("X-Pulse-Agent")
	if actor == "" {
		actor = store.HumanAgent
	}
	if r.Method == http.MethodPost {
		text, reset, ok := readProjectGoalPayload(w, r)
		if !ok {
			return
		}
		if err := goals.RequireOwner(h.Store.Home, actor); err != nil || r.Header.Get("X-Pulse-Job") != "" {
			writeError(w, http.StatusForbidden, "Only the owner can change a project goal",
				"An agent or worker cannot set or reset project direction.", "Use an owner session.")
			return
		}
		additionWrites.Lock()
		defer additionWrites.Unlock()
		if reset {
			goal, err = goals.Reset(h.Store, slug, store.HumanAgent)
		} else {
			goal, err = goals.Set(h.Store, slug, text, store.HumanAgent)
		}
	} else {
		goal, err = goals.Get(h.Store, slug)
	}
	if err != nil {
		additionFailure(w, err)
		return
	}
	p, err := h.Store.Project(slug)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, goals.View{Project: slug, Goal: goal,
		CanEdit: goals.CanEdit(h.Store, p, actor) && r.Header.Get("X-Pulse-Job") == ""})
}

// Unlike the general 16 KiB JSON reader, this accepts 4000 fully escaped
// supplementary code points (12 JSON bytes each) and refuses duplicate fields.
func readProjectGoalPayload(w http.ResponseWriter, r *http.Request) (string, bool, bool) {
	if origin := r.Header.Get("Origin"); origin != "" {
		u, err := url.Parse(origin)
		if err != nil || u.Host != r.Host || (u.Scheme != "http" && u.Scheme != "https") {
			writeError(w, 403, "Cross-origin writes are not allowed", "The request did not originate at this hub.", "Open the hub on 127.0.0.1.")
			return "", false, false
		}
	}
	if r.Header.Get("Sec-Fetch-Site") == "cross-site" {
		writeError(w, 403, "Cross-origin writes are not allowed", "The browser reports a cross-site request.", "Open the hub on 127.0.0.1.")
		return "", false, false
	}
	if media, _, err := mime.ParseMediaType(r.Header.Get("Content-Type")); err != nil || media != "application/json" {
		writeError(w, 415, "Content-Type must be application/json", "Only JSON writes are accepted.", "Send Content-Type: application/json.")
		return "", false, false
	}
	bad := func() (string, bool, bool) {
		writeError(w, http.StatusBadRequest, "Invalid project goal request",
			"Send exactly one UTF-8 JSON object: {text:string} or {reset:true}, without duplicate or unknown fields.",
			"Edit the goal text (at most 4000 Unicode code points) or reset to the generated default.")
		return "", false, false
	}
	const maxBody = goals.MaxLength*12 + 1024
	data, err := io.ReadAll(io.LimitReader(r.Body, maxBody+1))
	if err != nil || len(data) == 0 || len(data) > maxBody || !utf8.Valid(data) {
		return bad()
	}
	decoder := json.NewDecoder(strings.NewReader(string(data)))
	token, err := decoder.Token()
	if err != nil || token != json.Delim('{') || !decoder.More() {
		return bad()
	}
	field, err := decoder.Token()
	if err != nil {
		return bad()
	}
	value, err := decoder.Token()
	if err != nil {
		return bad()
	}
	text, isText := value.(string)
	reset, isBool := value.(bool)
	if !((field == "text" && isText) || (field == "reset" && isBool && reset)) || decoder.More() {
		return bad()
	}
	if token, err = decoder.Token(); err != nil || token != json.Delim('}') {
		return bad()
	}
	if err := decoder.Decode(new(any)); err != io.EOF {
		return bad()
	}
	return text, reset, true
}
