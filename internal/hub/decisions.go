package hub

import (
	"net/http"
	"strings"

	"github.com/manishtiwari25/pulse/internal/evolve"
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
	switch action {
	case "adopt", "discard", "draft-job":
		h.additionRecordDraft(w, r, id, action)
		return
	case "accept", "status":
	default:
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

// additionRecordDraft is the console's side of record drafts (ADR-046):
// adopt writes the waiting draft as the record's next revision in the
// human's name (words optional), discard drops it, draft-job queues the
// drafting job for the record's project at once, ignoring the planner's
// window. The console never edits the text itself.
func (h *Handler) additionRecordDraft(w http.ResponseWriter, r *http.Request, id, action string) {
	if !additionMethod(w, r, http.MethodPost) || !h.additionAvailable(w, r, "records", "record_revisions", "record_drafts") {
		return
	}
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	words, ok := additionString(w, payload, "words", false)
	if !ok {
		return
	}
	if words = strings.TrimSpace(words); words != "" {
		words += consoleNote
	} else if action != "draft-job" {
		words = strings.TrimSpace(consoleNote)
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	switch action {
	case "adopt":
		rec, err := records.Adopt(h.Store, id, store.HumanAgent, words)
		if err != nil {
			additionFailure(w, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{"record": additionRecord(*rec)})
	case "discard":
		gone, err := records.Discard(h.Store, id, store.HumanAgent, words)
		if err != nil {
			additionFailure(w, err)
			return
		}
		rec, _, err := h.Store.GetRecord(id, 0)
		if err != nil {
			additionFailure(w, err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{"record": additionRecord(*rec), "discarded": gone})
	default:
		rec, _, err := h.Store.GetRecord(id, 0)
		if err != nil {
			additionFailure(w, err)
			return
		}
		job, created, reason, err := evolve.PlanDraftJob(h.Store, rec.Project, store.HumanAgent, h.Store.Now(), true)
		if err != nil {
			additionFailure(w, err)
			return
		}
		if job == nil {
			writeError(w, http.StatusConflict, "Nothing to draft", reason, "The seeded records are already drafted or adopted; refresh the page.")
			return
		}
		writeJSON(w, http.StatusOK, map[string]any{"job": object("id status", job.ID, job.Status), "created": created, "reason": reason})
	}
}
