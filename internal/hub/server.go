// Package hub serves the owner console and the versioned, loopback-only API.
package hub

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"mime"
	"net"
	"net/http"
	"net/url"
	"os/exec"
	"path"
	"runtime"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/manishtiwari25/pulse/internal/provider"
	"github.com/manishtiwari25/pulse/internal/store"
	webhub "github.com/manishtiwari25/pulse/web/hub"
)

type Handler struct {
	Store   *store.Store
	Source  TallySource
	Project string
	UI      fs.FS
	// Service is true when the user service runs this hub (it passes
	// --restart-on-change); a foreground hub hands over to the service it
	// installs (ADR-038).
	Service bool
	// Port is the TCP port this hub listens on; a foreground hub hands over
	// only when it holds the port the hub unit will bind.
	Port int
	// Handover is called once, after the service-install response is
	// written, when a foreground hub installed the hub service: it must stop
	// serving so the service's hub can bind the port, then load the unit.
	Handover func(loadHub func() error)

	providerMu    sync.Mutex
	providerCache map[string]provider.Provider
}

func New(s *store.Store, source TallySource) *Handler {
	if source == nil && s != nil {
		source = nativeTally{s}
	}
	ui, err := fs.Sub(webhub.Dist, "out")
	if err != nil {
		panic(err) // The embed directive makes absence a build error.
	}
	return &Handler{Store: s, Source: source, UI: ui}
}

func writeJSON(w http.ResponseWriter, status int, payload any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	encoder := json.NewEncoder(w)
	encoder.SetEscapeHTML(false)
	_ = encoder.Encode(payload) // A disconnected client cannot receive an error.
}

func writeError(w http.ResponseWriter, status int, what, why, fix string) {
	writeJSON(w, status, map[string]any{"error": what, "why": why, "fix": fix})
}

func readPayload(w http.ResponseWriter, r *http.Request) (map[string]any, bool) {
	if origin := r.Header.Get("Origin"); origin != "" {
		u, err := url.Parse(origin)
		if err != nil || u.Host != r.Host || (u.Scheme != "http" && u.Scheme != "https") {
			writeError(w, 403, "Cross-origin writes are not allowed", "The request did not originate at this hub.", "Open the hub on 127.0.0.1.")
			return nil, false
		}
	}
	if r.Header.Get("Sec-Fetch-Site") == "cross-site" {
		writeError(w, 403, "Cross-origin writes are not allowed", "The browser reports a cross-site request.", "Open the hub on 127.0.0.1.")
		return nil, false
	}
	media, _, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
	if err != nil || media != "application/json" {
		writeError(w, 415, "Content-Type must be application/json", "Only JSON writes are accepted.", "Send Content-Type: application/json.")
		return nil, false
	}
	data, err := io.ReadAll(io.LimitReader(r.Body, 16385))
	if err != nil {
		writeError(w, 400, "body must be JSON", err.Error(), "Send a JSON object.")
		return nil, false
	}
	if len(data) == 0 || len(data) > 16384 {
		writeError(w, 400, "JSON body must be 1..16384 bytes", "The request body is outside the allowed size.", "Send a smaller JSON object.")
		return nil, false
	}
	if !utf8.Valid(data) {
		writeError(w, 400, "body must be JSON", "The request is not valid UTF-8.", "Send a UTF-8 JSON object.")
		return nil, false
	}
	var decoded any
	decoder := json.NewDecoder(strings.NewReader(string(data)))
	decoder.UseNumber()
	if err := decoder.Decode(&decoded); err != nil {
		writeError(w, 400, "body must be JSON", "The request body is malformed JSON.", "Send a JSON object.")
		return nil, false
	}
	payload, ok := decoded.(map[string]any)
	if !ok {
		writeError(w, 400, "body must be a JSON object", "The body is not a JSON object.", "Send an object with the documented fields.")
		return nil, false
	}
	if decoder.Decode(new(any)) != io.EOF {
		writeError(w, 400, "body must be JSON", "The body contains data after the JSON object.", "Send one JSON object.")
		return nil, false
	}
	return payload, true
}

func (h *Handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("Referrer-Policy", "no-referrer")
	w.Header().Set("X-Frame-Options", "DENY")
	// Static Next.js exports contain inline hydration scripts and styles.
	w.Header().Set("Content-Security-Policy", "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'")
	p := r.URL.Path
	if strings.HasPrefix(p, "/api/v1/") {
		route := strings.TrimPrefix(p, "/api/v1/")
		if route == "running" {
			h.additionRunning(w, r)
			return
		}
		if h.additions(w, r, route) {
			return
		}
		if strings.HasPrefix(route, "projects/") {
			slug, rest, ok := strings.Cut(strings.TrimPrefix(route, "projects/"), "/")
			if ok {
				h.projectAPI(w, r, slug, rest)
				return
			}
		}
		if r.Method == http.MethodPost && (route == "decide" || route == "settings") {
			h.writeAPI(w, r, route)
			return
		}
		if !readMethod(w, r) {
			return
		}
		if h.Project != "" {
			h.projectAPI(w, r, h.Project, route)
		} else {
			h.orgAPI(w, r, route)
		}
		return
	}
	legacy := strings.TrimPrefix(p, "/")
	legacy = strings.TrimPrefix(legacy, "p/")
	if slug, route, ok := strings.Cut(legacy, "/api/v1/"); ok {
		h.projectAPI(w, r, slug, route)
		return
	}
	if strings.Contains(p, "/api/") || strings.HasPrefix(p, "/api/") {
		writeError(w, 404, "not found", "No such API route.", "Use /api/v1/.")
		return
	}
	if !readMethod(w, r) {
		return
	}
	if strings.HasSuffix(p, "/") && h.Store != nil {
		slug := strings.Trim(legacy, "/")
		if !strings.Contains(slug, "/") && slug != "" {
			project, err := h.Store.Project(slug)
			if err != nil {
				writeError(w, 503, "The registry is unavailable", err.Error(), "Restart pulse hub.")
				return
			}
			if project != nil || slug == store.CommonsSlug {
				http.Redirect(w, r, "/project/?slug="+url.QueryEscape(slug), http.StatusFound)
				return
			}
		}
	}
	h.static(w, r)
}

func readMethod(w http.ResponseWriter, r *http.Request) bool {
	if r.Method == http.MethodGet || r.Method == http.MethodHead {
		return true
	}
	w.Header().Set("Allow", "GET, HEAD")
	writeError(w, 405, "Method not allowed", "This route is read-only.", "Use GET.")
	return false
}

func (h *Handler) static(w http.ResponseWriter, r *http.Request) {
	name := strings.TrimPrefix(path.Clean("/"+r.URL.Path), "/")
	if name == "." || name == "" {
		name = "index.html"
	} else if strings.HasSuffix(r.URL.Path, "/") {
		name += "/index.html"
	}
	data, err := fs.ReadFile(h.UI, name)
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		if info, statErr := fs.Stat(h.UI, name); statErr == nil && info.IsDir() {
			data, err = fs.ReadFile(h.UI, name+"/index.html")
			if err == nil {
				target := url.URL{Path: "/" + name + "/", RawQuery: r.URL.RawQuery, ForceQuery: r.URL.ForceQuery}
				http.Redirect(w, r, target.String(), http.StatusPermanentRedirect)
				return
			}
		}
		if !errors.Is(err, fs.ErrNotExist) {
			writeError(w, 503, "The UI could not be read", err.Error(), "Rebuild PULSE with the static UI export.")
			return
		}
	}
	if err != nil {
		name = "404.html"
		data, err = fs.ReadFile(h.UI, name)
		if errors.Is(err, fs.ErrNotExist) {
			name = "index.html"
			data, err = fs.ReadFile(h.UI, name)
		}
	}
	if err != nil {
		writeError(w, 503, "The hub UI is not built", err.Error(), "Run npm run build in web/hub, then rebuild pulse.")
		return
	}
	if strings.HasPrefix(name, "_next/") {
		w.Header().Set("Cache-Control", "public, max-age=31536000, immutable")
	} else {
		w.Header().Set("Cache-Control", "no-cache")
	}
	w.Header().Set("Content-Type", mime.TypeByExtension(path.Ext(name)))
	http.ServeContent(w, r, name, time.Time{}, strings.NewReader(string(data)))
}

// Serve owns only the HTTP listener. The caller owns the store and cancellation.
func Serve(ctx context.Context, listener net.Listener, h http.Handler) error {
	host, _, err := net.SplitHostPort(listener.Addr().String())
	if err != nil || host != "127.0.0.1" {
		return fmt.Errorf("hub refuses non-loopback listeners; use --host 127.0.0.1 (remote serving is not supported)")
	}
	server := &http.Server{
		Handler: h, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 30 * time.Second,
		IdleTimeout: 60 * time.Second, MaxHeaderBytes: 1 << 20,
		BaseContext: func(net.Listener) context.Context { return ctx },
	}
	stopped := make(chan error, 1)
	go func() { stopped <- server.Serve(listener) }()
	select {
	case err := <-stopped:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return err
	case <-ctx.Done():
		shutdown, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		if err := server.Shutdown(shutdown); err != nil {
			_ = server.Close()
			return err
		}
		err := <-stopped
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return err
	}
}

func openBrowser(address string) error {
	command := "xdg-open"
	if runtime.GOOS == "darwin" {
		command = "open"
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	return exec.CommandContext(ctx, command, address).Run()
}
