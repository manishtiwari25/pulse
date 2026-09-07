package hub

import (
	"encoding/json"
	"errors"
	"math"
	"net/http"

	"github.com/manishtiwari25/pulse/internal/evolve"
)

// The owner's held-out fixtures from the console (ADR-032): the hub is the pen,
// never the reader. GET is counts and the judges an owner may pick; the two
// POSTs write under the validated private directory through the helpers
// behind pulse evolve setup DIR and answer with counts. A scorer is a program
// the daemon will run, so a write needs a loopback Host and the intent header
// like decide and settings. No response carries the directory, a stem, a
// request or a scorer.

const heldoutHeader = "X-Pulse-Heldout"
const heldoutSlotLimit = 1000

func (h *Handler) additionHeldout(w http.ResponseWriter, r *http.Request, parts []string) {
	fixtures := len(parts) == 2 && parts[1] == "fixtures"
	if len(parts) != 1 && !fixtures {
		additionNotFound(w)
		return
	}
	methods := []string{http.MethodGet, http.MethodPost}
	if fixtures {
		methods = []string{http.MethodPost}
	}
	if !additionMethod(w, r, methods...) || !h.additionAvailable(w, r) {
		return
	}
	if r.Method == http.MethodGet {
		view := evolve.HeldoutStatus(h.Store)
		writeJSON(w, http.StatusOK, object("configured present count pending problem judges",
			view.Configured, view.Present, view.Count, view.Pending, view.Problem, view.Judges))
		return
	}
	if !loopbackHost(r) {
		writeError(w, http.StatusForbidden, "Held-out fixtures are written only on loopback", "The request named a host other than this machine.", "Open the hub on 127.0.0.1.")
		return
	}
	if r.Header.Get(heldoutHeader) != "yes" {
		writeError(w, http.StatusForbidden, "missing "+heldoutHeader+": yes", "An explicit confirmation header is required.", "Confirm the change in the hub.")
		return
	}
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	var result *evolve.SetupResult
	var err error
	if fixtures {
		write, ok := heldoutWrite(w, payload)
		if !ok {
			return
		}
		result, err = evolve.WriteHeldout(h.Store, write)
	} else {
		dir, ok := additionString(w, payload, "dir", true)
		if !ok {
			return
		}
		result, err = evolve.Setup(h.Store, dir)
	}
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, object("configured present count pending created problem",
		true, true, result.Count, result.Pending, result.Created, ""))
}

func heldoutWrite(w http.ResponseWriter, payload map[string]any) (evolve.HeldoutWrite, bool) {
	var write evolve.HeldoutWrite
	var ok bool
	if write.Request, ok = additionString(w, payload, "request", true); !ok {
		return write, false
	}
	if write.Judge, ok = additionString(w, payload, "judge", false); !ok {
		return write, false
	}
	if write.Scorer, ok = additionString(w, payload, "scorer", false); !ok {
		return write, false
	}
	write.Slot, ok = heldoutSlot(w, payload)
	return write, ok
}

// heldoutSlot reads the optional 1-based slot like additionPriority reads a
// priority; 0 or absent lets the helper choose the pair.
func heldoutSlot(w http.ResponseWriter, payload map[string]any) (int, bool) {
	value, exists := payload["slot"]
	if !exists {
		return 0, true
	}
	var slot int64
	var err error
	switch value := value.(type) {
	case json.Number:
		slot, err = value.Int64()
	case float64:
		if math.Trunc(value) != value || value > heldoutSlotLimit || value < 0 {
			err = errors.New("slot is not an integer")
		} else {
			slot = int64(value)
		}
	default:
		err = errors.New("slot is not a number")
	}
	if err != nil || slot < 0 || slot > heldoutSlotLimit {
		writeError(w, http.StatusBadRequest, "Invalid slot", "slot must be a whole number from 1 to 1000, or 0 for the next pair.", "Send the fixture's position in name order, or omit slot.")
		return 0, false
	}
	return int(slot), true
}
