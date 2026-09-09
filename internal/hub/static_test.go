package hub

import (
	"bytes"
	"io/fs"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"testing/fstest"
)

func TestStaticExportSlashlessPages(t *testing.T) {
	h := New(nil, nil)
	const query = "?slug=demo-app&tab=board&filter=a%2Fb&empty="
	for _, route := range []string{"/project", "/tasks", "/votes", "/records", "/compute", "/settings", "/agents"} {
		wantBody, err := fs.ReadFile(h.UI, strings.TrimPrefix(route, "/")+"/index.html")
		if err != nil {
			t.Fatal(err)
		}
		for _, method := range []string{http.MethodGet, http.MethodHead} {
			t.Run(method+" "+route, func(t *testing.T) {
				rec := httptest.NewRecorder()
				h.ServeHTTP(rec, httptest.NewRequest(method, route+query, nil))
				wantLocation := route + "/" + query
				if rec.Code != http.StatusPermanentRedirect || rec.Header().Get("Location") != wantLocation {
					t.Fatalf("status %d, location %q; want 308, %q", rec.Code, rec.Header().Get("Location"), wantLocation)
				}
				if method == http.MethodHead && rec.Body.Len() != 0 {
					t.Fatal("HEAD redirect returned a body")
				}
				page := httptest.NewRecorder()
				h.ServeHTTP(page, httptest.NewRequest(method, wantLocation, nil))
				if page.Code != http.StatusOK || !strings.HasPrefix(page.Header().Get("Content-Type"), "text/html") {
					t.Fatalf("canonical page: status %d, content type %q", page.Code, page.Header().Get("Content-Type"))
				}
				if method == http.MethodHead {
					if page.Body.Len() != 0 {
						t.Fatal("HEAD page returned a body")
					}
				} else if !bytes.Equal(page.Body.Bytes(), wantBody) {
					t.Fatal("canonical page did not serve its exported HTML")
				}
			})
		}
	}
}

func TestStaticExportDirectoryRedirectURLs(t *testing.T) {
	h := New(nil, nil)
	h.UI = fstest.MapFS{
		"nested/tasks/index.html": {Data: []byte("tasks")},
		"page name/index.html":    {Data: []byte("page")},
		"404.html":                {Data: []byte("not-found")},
	}
	for _, tc := range []struct {
		request, location string
	}{
		{"/nested/tasks", "/nested/tasks/"},
		{"/nested/tasks?", "/nested/tasks/?"},
		{"/page%20name?filter=a%2Fb", "/page%20name/?filter=a%2Fb"},
		{"//nested/tasks", "/nested/tasks/"},
	} {
		t.Run(tc.request, func(t *testing.T) {
			rec := httptest.NewRecorder()
			h.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, tc.request, nil))
			if rec.Code != http.StatusPermanentRedirect || rec.Header().Get("Location") != tc.location {
				t.Fatalf("status %d, location %q; want 308, %q", rec.Code, rec.Header().Get("Location"), tc.location)
			}
		})
	}
}

func TestStaticExportDirectoryWithoutIndex(t *testing.T) {
	h := New(nil, nil)
	h.UI = fstest.MapFS{
		"_next/static/main.js": {Data: []byte("asset")},
		"404.html":             {Data: []byte("not-found")},
	}
	for _, route := range []string{"/_next/static", "/_next/static/"} {
		t.Run(route, func(t *testing.T) {
			rec := httptest.NewRecorder()
			h.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, route, nil))
			if rec.Code != http.StatusOK || rec.Body.String() != "not-found" || rec.Header().Get("Location") != "" {
				t.Fatalf("directory without index changed fallback: %d %q", rec.Code, rec.Body.String())
			}
		})
	}
}

type unreadableStaticIndexFS struct{ fs.FS }

func (f unreadableStaticIndexFS) Open(name string) (fs.File, error) {
	if name == "tasks/index.html" {
		return nil, &fs.PathError{Op: "open", Path: name, Err: fs.ErrPermission}
	}
	return f.FS.Open(name)
}

func TestStaticExportUnreadableDirectoryIndex(t *testing.T) {
	h := New(nil, nil)
	h.UI = unreadableStaticIndexFS{fstest.MapFS{
		"tasks/index.html": {Data: []byte("tasks")},
		"404.html":         {Data: []byte("not-found")},
	}}
	for _, route := range []string{"/tasks", "/tasks/"} {
		t.Run(route, func(t *testing.T) {
			rec := httptest.NewRecorder()
			h.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, route, nil))
			if rec.Code != http.StatusServiceUnavailable || !strings.Contains(rec.Body.String(), "The UI could not be read") {
				t.Fatalf("index read error was hidden: %d %q", rec.Code, rec.Body.String())
			}
		})
	}
}
