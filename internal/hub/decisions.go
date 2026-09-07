package hub

import (
	"net/http"
	"strings"

	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
)

// The human gates in the owner console (ADR-028): accept, decline or defer a
// Proposed ADR with the owner's words, through the functions the CLI runs.
// The store's rule is untouched - Accepted is refused for every author and
// only records.Accept, which demands the words, sets it.

const consoleNote = " (via console)"

// additionRecordDecision serves records/{id}/accept and records/{id}/status.
func (h *Handler) additionRecordDecision(w http.ResponseWriter, r *http.Request, id, action string) {
	if action != "accept" && action != "status" {
		additionNotFound(w)
		return
	}
	if !additionMethod(w, r, http.MethodPost) || !h.additionAvailable(w, r, "records", "record_revisions") {
		return
	}
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	// Optional at parse time so a missing or blank reason gets the owner's
	// refusal below rather than the generic "Invalid words".
	words, ok := additionString(w, payload, "words", false)
	if !ok {
		return
	}
	words = strings.TrimSpace(words)
	if words == "" {
		writeError(w, http.StatusBadRequest, "Your words are required", "A decision is recorded with the reason in the owner's words.", "Say why in a sentence.")
		return
	}
	status := ""
	if action == "status" {
		raw, ok := additionString(w, payload, "status", true)
		if !ok {
			return
		}
		switch strings.ToLower(strings.TrimSpace(raw)) {
		case "rejected":
			status = "Rejected"
		case "deferred":
			status = "Deferred"
		default:
			writeError(w, http.StatusBadRequest, "The console declines or defers", "Only Rejected and Deferred are set here; acceptance is its own action and every other status stays in the terminal.", "Use Accept, or pulse record status "+id+" STATUS.")
			return
		}
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	var err error
	if action == "accept" {
		err = records.Accept(h.Store, id, words+consoleNote)
	} else {
		err = records.StatusNote(h.Store, id, status, store.HumanAgent, words+consoleNote)
	}
	if err != nil {
		additionFailure(w, err)
		return
	}
	rec, _, err := h.Store.GetRecord(id, 0)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"record": additionRecord(*rec)})
}
