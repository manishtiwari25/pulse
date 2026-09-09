package hub

import (
	"encoding/json"
	"net/http"
	"os"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
)

// GET /api/v1/running: which pulse is running where, against the brain's HEAD
// (ADR-035). The hub's own build, the daemon's from its heartbeat, the brain
// checkout's branch and head, the daemon.self_update caps, and the last
// rebuild the daemon recorded. The owner reads it on the Settings page under
// "Running"; there is no POST - the daemon rebuilds itself, and turning that
// off is the ordinary settings write.
//
// state is one word for the whole picture:
//
//	off        daemon.self_update is off
//	no-daemon  the daemon never wrote a heartbeat
//	no-brain   no active project is the brain
//	current    the daemon runs the brain's HEAD
//	unstamped  the daemon's build carries no revision (a worktree build); it
//	           rebuilds once on the next landing
//	behind     the brain's HEAD differs from the daemon's build; the daemon
//	           rebuilds on its next tick unless the checkout is off its
//	           default branch or dirty
//	waiting    the brain checkout is dirty or on another branch, so the
//	           daemon waits (rule 4)
func (h *Handler) running(r *http.Request) (map[string]any, error) {
	hub := object("version revision pid", cli.Version(), cli.Revision(), os.Getpid())
	// macOS ties a folder answer to the binary's signature (ADR-035 rule 8): null off macOS.
	var signed any
	if ok, applicable := cli.CodeSigned(cli.Executable()); applicable {
		signed = ok
	}
	if h.Store == nil {
		return object("available self_update retry_minutes hub daemon brain last state signed", false, "", 0, hub, nil, nil, nil, "no-store", signed), nil
	}
	selfUpdate := h.Store.Setting("daemon.self_update", "on")
	retry := h.Store.SettingInt("daemon.self_update_retry_minutes")
	if retry <= 0 {
		retry = 60
	}

	var daemon map[string]any
	if raw, ok, err := h.Store.Meta("daemon"); err != nil {
		return nil, err
	} else if ok {
		var beat map[string]any
		if json.Unmarshal([]byte(raw), &beat) == nil {
			daemon = object("pid at version revision bus", beat["pid"], str(beat["at"]), str(beat["version"]), str(beat["revision"]), beat["bus"])
		}
	}

	var brain map[string]any
	if p, err := h.Store.Brain(); err != nil {
		return nil, err
	} else if p != nil {
		head := gitValue(p.Path, "rev-parse", "HEAD")
		branch := gitValue(p.Path, "rev-parse", "--abbrev-ref", "HEAD")
		dirty := gitValue(p.Path, "status", "--porcelain") != ""
		brain = object("slug default_branch branch head dirty", p.Slug, p.DefaultBranch, branch, head, dirty)
	}

	var last map[string]any
	rows, err := h.additionRows(r, "SELECT at, detail FROM events WHERE action = 'daemon.self_update' ORDER BY seq DESC LIMIT 1")
	if err != nil {
		return nil, err
	}
	if len(rows) == 1 {
		var detail map[string]any
		_ = json.Unmarshal([]byte(str(rows[0]["detail"])), &detail)
		last = object("at from to inflight", str(rows[0]["at"]), str(detail["from"]), str(detail["to"]), num(detail["inflight"]))
	}

	state := "current"
	switch {
	case selfUpdate != "on":
		state = "off"
	case daemon == nil:
		state = "no-daemon"
	case brain == nil:
		state = "no-brain"
	case str(daemon["revision"]) == "":
		state = "unstamped"
	case str(brain["head"]) != "" && str(brain["head"]) != str(daemon["revision"]):
		state = "behind"
		if brain["dirty"] == true || (str(brain["branch"]) != str(brain["default_branch"]) && !strings.HasPrefix(str(brain["branch"]), "HEAD")) {
			state = "waiting"
		}
	}
	return object("available self_update retry_minutes hub daemon brain last state signed", true, selfUpdate, retry, hub, daemon, brain, last, state, signed), nil
}

func (h *Handler) additionRunning(w http.ResponseWriter, r *http.Request) {
	if !readMethod(w, r) {
		return
	}
	data, err := h.running(r)
	h.respond(w, data, err)
}
