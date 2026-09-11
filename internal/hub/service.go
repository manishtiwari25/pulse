package hub

import (
	"net/http"
	"strconv"

	"github.com/manishtiwari25/pulse/internal/runner"
)

// The user services that keep PULSE alive, from the console (ADR-038):
//
//	GET  /api/v1/service            the service manager, each unit's file and load state, whether the
//	                                daemon is kept alive, and whether this hub is the service's
//	POST /api/v1/service {dry_run}  preview (dry_run true or absent) or install (dry_run false) the
//	                                daemon's and the hub's units, through the function `pulse service
//	                                install` runs; loopback-only and JSON-only like every owner write
//
// A foreground hub on the service's port hands over: it answers, stops
// listening, loads the hub unit and exits, so the service's hub binds the
// same address and the page reconnects. A hub the service already runs
// installs idempotently and keeps serving.

// serviceHubPort is the port the hub unit listens on; a foreground hub elsewhere does not hand over.
const serviceHubPort = 8765

func (h *Handler) additionService(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) != 1 {
		additionNotFound(w)
		return
	}
	if !additionMethod(w, r, http.MethodGet, http.MethodPost) || !h.additionAvailable(w, r) {
		return
	}
	state := runner.ServiceStatus(h.Store.Home)
	if r.Method == http.MethodGet {
		writeJSON(w, http.StatusOK, map[string]any{"service": state, "hub_is_service": h.Service, "hub_port": h.Port,
			"hub_url": "http://127.0.0.1:" + strconv.Itoa(serviceHubPort), "command": "pulse service install"})
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
	if !state.Supported {
		writeError(w, http.StatusBadRequest, "No user service manager here", "PULSE keeps itself alive with launchd (macOS) or systemd --user (Linux); this system has neither.", "Run pulse daemon and pulse hub yourself, or from your own supervisor.")
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	deferHub := !dryRun && !h.Service && h.Port == serviceHubPort
	result, err := runner.InstallService(nil, h.Store.Home, runner.ServiceInstallOptions{DryRun: dryRun, DeferHub: deferHub})
	if err != nil {
		output := ""
		if result != nil {
			output = result.Output
		}
		writeJSON(w, http.StatusConflict, map[string]any{"error": err.Error(), "why": "The service manager refused a step; the files it wrote stay.", "fix": "Run pulse service install in a terminal to see the step.", "output": output})
		return
	}
	handover := result.LoadHub != nil && h.Handover != nil
	writeJSON(w, http.StatusOK, map[string]any{"dry_run": dryRun, "installed": !dryRun, "kind": result.Kind, "daemon_path": result.DaemonPath,
		"hub_path": result.HubPath, "output": result.Output, "handover": handover, "hub_url": "http://127.0.0.1:" + strconv.Itoa(serviceHubPort)})
	if handover {
		// After the response: stop serving, load the hub unit, exit; the service's hub takes the port.
		go h.Handover(result.LoadHub)
	}
}
