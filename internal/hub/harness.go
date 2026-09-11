package hub

import (
	"context"
	"database/sql"
	"errors"
	"net/http"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/provider"
	"github.com/manishtiwari25/pulse/internal/store"
)

type harnessSessionRow struct {
	store.HarnessSession
	JobTitle string `json:"job_title"`
}

func (h *Handler) additionHarness(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) < 1 || parts[0] != "harness" || len(parts) > 3 {
		additionNotFound(w)
		return
	}
	if !h.additionAvailable(w, r, "harness_sessions") {
		return
	}
	switch {
	case len(parts) == 1:
		if !additionMethod(w, r, http.MethodGet) {
			return
		}
		h.additionHarnessStatus(w, r)
	case len(parts) == 2 && parts[1] == "models":
		if !additionMethod(w, r, http.MethodGet) {
			return
		}
		h.additionHarnessModels(w, r)
	case len(parts) == 2 && parts[1] == "login":
		if !additionMethod(w, r, http.MethodPost) {
			return
		}
		h.additionHarnessLogin(w, r)
	case len(parts) == 3 && parts[1] == "login" && parts[2] == "poll":
		if !additionMethod(w, r, http.MethodPost) {
			return
		}
		h.additionHarnessLoginPoll(w, r)
	case len(parts) == 3 && parts[1] == "login" && parts[2] == "token":
		if !additionMethod(w, r, http.MethodPost) {
			return
		}
		h.additionHarnessTokenLogin(w, r)
	case len(parts) == 2 && parts[1] == "logout":
		if !additionMethod(w, r, http.MethodPost) {
			return
		}
		h.additionHarnessLogout(w, r)
	case len(parts) == 2 && parts[1] == "sessions":
		if !additionMethod(w, r, http.MethodGet) {
			return
		}
		h.additionHarnessSessions(w, r)
	case len(parts) == 3 && parts[1] == "sessions":
		if !additionMethod(w, r, http.MethodGet) {
			return
		}
		h.additionHarnessSession(w, r, parts[2])
	default:
		additionNotFound(w)
	}
}

func (h *Handler) additionHarnessStatus(w http.ResponseWriter, r *http.Request) {
	p, err := h.harnessProvider("")
	if err != nil {
		additionFailure(w, err)
		return
	}
	login := h.harnessProviderStatus(r.Context(), p, 5*time.Second)
	running, today, failedToday, err := h.Store.HarnessSessionCounts()
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, object("harness provider providers models limits sessions settings",
		h.Store.Setting("harness", ""),
		h.harnessProviderSummary(p, login),
		h.harnessProviderSummaries(),
		object("worker light", h.Store.Setting("worker_model", ""), h.Store.Setting("light_model", "")),
		object("max_steps step_timeout_seconds output_limit minutes_per_job",
			h.Store.SettingInt("harness.max_steps"), h.Store.SettingInt("harness.step_timeout"),
			h.Store.SettingInt("harness.output_limit"), h.Store.SettingInt("minutes_per_job")),
		object("running today failed_today", running, today, failedToday),
		h.harnessSettings(),
	))
}

func (h *Handler) additionHarnessModels(w http.ResponseWriter, r *http.Request) {
	p, err := h.harnessProvider("")
	if err != nil {
		additionFailure(w, err)
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), 15*time.Second)
	defer cancel()
	models, err := p.Models(ctx)
	if errors.Is(err, provider.ErrNotLoggedIn) {
		writeError(w, http.StatusServiceUnavailable, "not signed in", "The GitHub Copilot login is missing on this machine.", "Sign in with GitHub.")
		return
	}
	if err != nil {
		writeError(w, http.StatusBadGateway, "provider failed", h.redactedProviderError(err), "Retry, or sign in again.")
		return
	}
	writeJSON(w, http.StatusOK, object("models fetched_at", models, time.Now().UTC().Format(time.RFC3339)))
}

func (h *Handler) additionHarnessLogin(w http.ResponseWriter, r *http.Request) {
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	p, err := h.harnessProvider(strings.TrimSpace(str(payload["provider"])))
	if err != nil {
		additionFailure(w, err)
		return
	}
	loginer, ok := p.(provider.DeviceLoginer)
	if !ok {
		writeError(w, http.StatusBadRequest, "device login is not supported", "This provider does not implement device login.", "Choose a provider that supports device login.")
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), 15*time.Second)
	defer cancel()
	flow, err := loginer.BeginDeviceLogin(ctx)
	if err != nil {
		writeError(w, http.StatusBadGateway, "provider failed", h.redactedProviderError(err), "Retry, or sign in another way.")
		return
	}
	writeJSON(w, http.StatusOK, flow)
}

func (h *Handler) additionHarnessLoginPoll(w http.ResponseWriter, r *http.Request) {
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	deviceCode := strings.TrimSpace(str(payload["device_code"]))
	if deviceCode == "" {
		writeError(w, http.StatusBadRequest, "missing device_code", "The device login poll needs the device_code returned by /harness/login.", "Send device_code.")
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	p, err := h.harnessProvider("")
	if err != nil {
		additionFailure(w, err)
		return
	}
	loginer, ok := p.(provider.DeviceLoginer)
	if !ok {
		writeError(w, http.StatusBadRequest, "device login is not supported", "This provider does not implement device login.", "Choose a provider that supports device login.")
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), 15*time.Second)
	defer cancel()
	status, login, err := loginer.PollDeviceLogin(ctx, deviceCode)
	if err != nil {
		writeError(w, http.StatusBadGateway, "provider failed", h.redactedProviderError(err), "Retry the poll.")
		return
	}
	if status == provider.DeviceComplete {
		h.invalidateHarnessProvider(p.Name())
		p, err = h.harnessProvider(p.Name())
		if err != nil {
			additionFailure(w, err)
			return
		}
		login = h.harnessProviderStatus(r.Context(), p, 5*time.Second)
	} else {
		login = provider.Login{LoggedIn: false}
	}
	writeJSON(w, http.StatusOK, object("status login", status, login))
}

func (h *Handler) additionHarnessTokenLogin(w http.ResponseWriter, r *http.Request) {
	payload, ok := readPayload(w, r)
	if !ok {
		return
	}
	token, ok := payload["token"].(string)
	if !ok || strings.TrimSpace(token) == "" {
		writeError(w, http.StatusBadRequest, "missing token", "Token login needs a non-empty token string.", "Paste a GitHub token.")
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	p, err := h.harnessProvider("")
	if err != nil {
		additionFailure(w, err)
		return
	}
	loginer, ok := p.(provider.TokenLoginer)
	if !ok {
		writeError(w, http.StatusBadRequest, "token login is not supported", "This provider does not implement token login.", "Choose a provider that supports token login.")
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), 15*time.Second)
	defer cancel()
	login, err := loginer.LoginWithToken(ctx, token)
	if err != nil {
		writeError(w, http.StatusBadRequest, "token login failed", h.redactedProviderError(err, token), "Paste a valid token.")
		return
	}
	h.invalidateHarnessProvider(p.Name())
	writeJSON(w, http.StatusOK, object("login", login))
}

func (h *Handler) additionHarnessLogout(w http.ResponseWriter, r *http.Request) {
	if _, ok := readPayload(w, r); !ok {
		return
	}
	additionWrites.Lock()
	defer additionWrites.Unlock()
	p, err := h.harnessProvider("")
	if err != nil {
		additionFailure(w, err)
		return
	}
	logouter, ok := p.(provider.Logouter)
	if !ok {
		writeError(w, http.StatusBadRequest, "logout is not supported", "This provider does not implement logout.", "Choose a provider that supports logout.")
		return
	}
	if err := logouter.Logout(); err != nil {
		writeError(w, http.StatusBadGateway, "provider failed", h.redactedProviderError(err), "Retry logout.")
		return
	}
	h.invalidateHarnessProvider(p.Name())
	p, err = h.harnessProvider(p.Name())
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, object("ok login", true, h.harnessProviderStatus(r.Context(), p, 5*time.Second)))
}

func (h *Handler) additionHarnessSessions(w http.ResponseWriter, r *http.Request) {
	limit, ok := additionInteger(w, r.URL.Query().Get("limit"), "limit", 50, 500)
	if !ok {
		return
	}
	sessions, err := h.Store.HarnessSessions(store.HarnessSessionFilter{
		Project: r.URL.Query().Get("project"),
		Agent:   r.URL.Query().Get("agent"),
		Status:  r.URL.Query().Get("status"),
		JobID:   r.URL.Query().Get("job"),
		Limit:   int(limit),
	})
	if err != nil {
		additionFailure(w, err)
		return
	}
	titles, err := h.harnessJobTitles(r.Context(), sessions)
	if err != nil {
		additionFailure(w, err)
		return
	}
	out := make([]harnessSessionRow, 0, len(sessions))
	for _, session := range sessions {
		if len(session.Submission) > 2000 {
			session.Submission = session.Submission[:2000]
		}
		out = append(out, harnessSessionRow{HarnessSession: session, JobTitle: titles[session.JobID]})
	}
	writeJSON(w, http.StatusOK, object("sessions", out))
}

func (h *Handler) additionHarnessSession(w http.ResponseWriter, r *http.Request, id string) {
	if _, present := r.URL.Query()["after"]; present {
		after, ok := additionInteger(w, r.URL.Query().Get("after"), "after", 0, 0)
		if !ok {
			return
		}
		session, err := h.Store.HarnessSessionHead(id)
		if err != nil {
			additionFailure(w, err)
			return
		}
		if session == nil {
			additionFailure(w, sql.ErrNoRows)
			return
		}
		title, err := h.harnessJobTitle(r.Context(), session.JobID)
		if err != nil {
			additionFailure(w, err)
			return
		}
		steps, err := h.Store.HarnessSteps(id, int(after))
		if err != nil {
			additionFailure(w, err)
			return
		}
		writeJSON(w, http.StatusOK, object("session steps after", harnessSessionRow{HarnessSession: *session, JobTitle: title}, steps, after))
		return
	}
	session, steps, err := h.Store.HarnessSession(id)
	if err != nil {
		additionFailure(w, err)
		return
	}
	if session == nil {
		additionFailure(w, sql.ErrNoRows)
		return
	}
	title, err := h.harnessJobTitle(r.Context(), session.JobID)
	if err != nil {
		additionFailure(w, err)
		return
	}
	writeJSON(w, http.StatusOK, object("session steps", harnessSessionRow{HarnessSession: *session, JobTitle: title}, steps))
}

func (h *Handler) harnessProvider(name string) (provider.Provider, error) {
	name = strings.TrimSpace(name)
	if name == "" {
		name = h.Store.Setting("provider", "")
	}
	name = strings.TrimSpace(name)
	if name == "" {
		name = provider.Default
	}
	h.providerMu.Lock()
	defer h.providerMu.Unlock()
	if h.providerCache == nil {
		h.providerCache = map[string]provider.Provider{}
	}
	if p := h.providerCache[name]; p != nil {
		return p, nil
	}
	p, err := provider.Open(name, provider.Options{Home: h.Store.Home})
	if err != nil {
		return nil, err
	}
	h.providerCache[name] = p
	return p, nil
}

func (h *Handler) invalidateHarnessProvider(name string) {
	h.providerMu.Lock()
	defer h.providerMu.Unlock()
	if h.providerCache != nil {
		delete(h.providerCache, name)
	}
}

func (h *Handler) harnessProviderStatus(ctx context.Context, p provider.Provider, timeout time.Duration) provider.Login {
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	login, err := p.Status(ctx)
	if err != nil {
		return provider.Login{LoggedIn: false, Detail: h.redactedProviderError(err)}
	}
	return login
}

func (h *Handler) harnessProviderSummary(p provider.Provider, login provider.Login) map[string]any {
	return object("name label device_login token_login login", p.Name(), p.Label(), implementsDeviceLogin(p), implementsTokenLogin(p), login)
}

func (h *Handler) harnessProviderSummaries() []map[string]any {
	names := provider.Names()
	out := make([]map[string]any, 0, len(names))
	for _, name := range names {
		p, err := h.harnessProvider(name)
		if err != nil {
			out = append(out, object("name label device_login token_login", name, provider.LabelOf(name), false, false))
			continue
		}
		out = append(out, object("name label device_login token_login", p.Name(), p.Label(), implementsDeviceLogin(p), implementsTokenLogin(p)))
	}
	return out
}

func (h *Handler) harnessSettings() []map[string]any {
	names := []string{"harness", "provider", "harness.max_steps", "harness.step_timeout", "harness.output_limit", "worker_model", "light_model"}
	out := make([]map[string]any, 0, len(names))
	for _, name := range names {
		rule := store.SettingRules[name]
		if modelRule, ok := store.ModelRules[name]; ok {
			rule = modelRule
		}
		if harnessRule, ok := store.HarnessRules[name]; ok {
			rule = harnessRule
		}
		out = append(out, object("name value kind choices description", name, h.Store.Setting(name, ""), rule.Kind, rule.Choices, rule.Description))
	}
	return out
}

func (h *Handler) harnessJobTitle(ctx context.Context, jobID string) (string, error) {
	if strings.TrimSpace(jobID) == "" {
		return "", nil
	}
	var title string
	err := h.Store.DB.QueryRowContext(ctx, "SELECT title FROM jobs WHERE id = ?", jobID).Scan(&title)
	if errors.Is(err, sql.ErrNoRows) {
		return "", nil
	}
	return title, err
}

func (h *Handler) harnessJobTitles(ctx context.Context, sessions []store.HarnessSession) (map[string]string, error) {
	ids := make([]string, 0, len(sessions))
	seen := map[string]bool{}
	for _, session := range sessions {
		if session.JobID != "" && !seen[session.JobID] {
			seen[session.JobID] = true
			ids = append(ids, session.JobID)
		}
	}
	titles := map[string]string{}
	if len(ids) == 0 {
		return titles, nil
	}
	placeholders := strings.TrimSuffix(strings.Repeat("?,", len(ids)), ",")
	args := make([]any, len(ids))
	for i, id := range ids {
		args[i] = id
	}
	rows, err := h.Store.DB.QueryContext(ctx, "SELECT id, title FROM jobs WHERE id IN ("+placeholders+")", args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	for rows.Next() {
		var id, title string
		if err := rows.Scan(&id, &title); err != nil {
			return nil, err
		}
		titles[id] = title
	}
	return titles, rows.Err()
}

func (h *Handler) redactedProviderError(err error, secrets ...string) string {
	if err == nil {
		return ""
	}
	message := strings.Join(strings.Fields(err.Error()), " ")
	if message == "" {
		return ""
	}
	for _, secret := range secrets {
		secret = strings.TrimSpace(secret)
		if secret != "" {
			message = strings.ReplaceAll(message, secret, bus.PrivateValue)
		}
	}
	redact := bus.Redactor(h.Store)
	return redact(bus.Event{Detail: message}).Detail
}

func implementsDeviceLogin(p provider.Provider) bool {
	_, ok := p.(provider.DeviceLoginer)
	return ok
}

func implementsTokenLogin(p provider.Provider) bool {
	_, ok := p.(provider.TokenLoginer)
	return ok
}
