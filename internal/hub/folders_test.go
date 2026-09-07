package hub

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

// foldersRequest is additionsRequest with the loopback Host a real browser
// sends; httptest's default Host is example.com, which the route refuses.
func foldersRequest(t *testing.T, h http.Handler, method, path, body string) (int, map[string]any) {
	t.Helper()
	req := httptest.NewRequest(method, path, nil)
	req.Host = "127.0.0.1:8765"
	if method == http.MethodPost {
		req.Header.Set("Content-Type", "application/json")
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	var result map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &result); err != nil {
		t.Fatalf("%s %s: status %d, non-JSON response %q: %v", method, path, rec.Code, rec.Body, err)
	}
	return rec.Code, result
}

func folderNames(result map[string]any) []string {
	names := []string{}
	for _, e := range result["entries"].([]any) {
		names = append(names, e.(map[string]any)["name"].(string))
	}
	return names
}

func TestFoldersListing(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	repo := lifecycleRepo(t)
	if status, result := additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(repo)+`,"name":"My App"}`); status != 200 {
		t.Fatalf("register: %d %#v", status, result)
	}

	root := t.TempDir()
	if real, err := filepath.EvalSymlinks(root); err == nil {
		root = real
	}
	for _, d := range []string{"Zeta", "alpha", ".hidden", "beta/.git"} {
		if err := os.MkdirAll(filepath.Join(root, d), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(root, "notes.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(repo, filepath.Join(root, "linked")); err != nil {
		t.Fatal(err)
	}

	status, result := foldersRequest(t, h, "GET", "/api/v1/folders?path="+root, "")
	if status != 200 {
		t.Fatalf("list: %d %#v", status, result)
	}
	if got := folderNames(result); len(got) != 4 || got[0] != "alpha" || got[1] != "beta" || got[2] != "linked" || got[3] != "Zeta" {
		t.Fatalf("entries (folders only, no hidden, case-insensitive order): %v", got)
	}
	if result["path"] != root || result["parent"] != filepath.Dir(root) || result["checkout"] != false || result["registered"] != "" {
		t.Fatalf("listing header: %#v", result)
	}
	for _, e := range result["entries"].([]any) {
		entry := e.(map[string]any)
		switch entry["name"] {
		case "beta":
			if entry["checkout"] != true || entry["registered"] != "" {
				t.Fatalf("beta is an unregistered checkout: %#v", entry)
			}
		case "linked":
			if entry["checkout"] != true || entry["registered"] != "" || entry["path"] != filepath.Join(root, "linked") {
				t.Fatalf("a symlink to a checkout lists as a folder: %#v", entry)
			}
		case "alpha", "Zeta":
			if entry["checkout"] != false {
				t.Fatalf("plain folder: %#v", entry)
			}
		}
	}

	// The registered checkout itself, and its parent as a quick root.
	status, result = foldersRequest(t, h, "GET", "/api/v1/folders?path="+repo, "")
	if status != 200 || result["checkout"] != true || result["registered"] != "my-app" {
		t.Fatalf("registered checkout: %d %#v", status, result)
	}
	roots := result["roots"].([]any)
	home, _ := os.UserHomeDir()
	if len(roots) < 2 || roots[0].(map[string]any)["path"] != home || roots[len(roots)-1].(map[string]any)["path"] != filepath.Dir(repo) {
		t.Fatalf("roots are home then the project parents: %#v", roots)
	}
	status, result = foldersRequest(t, h, "GET", "/api/v1/folders?path="+filepath.Dir(repo), "")
	if status != 200 {
		t.Fatalf("parent: %d %#v", status, result)
	}
	found := false
	for _, e := range result["entries"].([]any) {
		if entry := e.(map[string]any); entry["path"] == repo {
			found = entry["registered"] == "my-app" && entry["checkout"] == true
		}
	}
	if !found {
		t.Fatalf("the registered project is marked in its parent: %#v", result["entries"])
	}

	// No path starts where the projects live.
	status, result = foldersRequest(t, h, "GET", "/api/v1/folders", "")
	if status != 200 || result["path"] != filepath.Dir(repo) {
		t.Fatalf("default start: %d %#v", status, result)
	}
}

func TestFoldersRefusals(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "file.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}

	cases := []struct {
		path string
		code int
		what string
	}{
		{"?path=" + filepath.Join(root, "missing"), 404, "No folder " + filepath.Join(root, "missing")},
		{"?path=" + filepath.Join(root, "file.txt"), 400, "Not a folder: " + filepath.Join(root, "file.txt")},
		{"?path=relative/dir", 400, "Not an absolute path"},
	}
	for _, c := range cases {
		status, result := foldersRequest(t, h, "GET", "/api/v1/folders"+c.path, "")
		if status != c.code || result["error"] != c.what {
			t.Fatalf("%s: %d %#v", c.path, status, result)
		}
	}

	if status, result := foldersRequest(t, h, "POST", "/api/v1/folders", `{}`); status != 405 {
		t.Fatalf("POST: %d %#v", status, result)
	}
	if status, result := foldersRequest(t, h, "GET", "/api/v1/folders/x", ""); status != 404 {
		t.Fatalf("sub-route: %d %#v", status, result)
	}

	// A hostname that resolves to this machine but is not loopback by name.
	req := httptest.NewRequest(http.MethodGet, "/api/v1/folders?path="+root, nil)
	req.Host = "evil.example:8765"
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if rec.Code != 403 {
		t.Fatalf("rebound host: %d %s", rec.Code, rec.Body)
	}
	for _, host := range []string{"127.0.0.1:8765", "localhost:8765", "[::1]:8765", "LOCALHOST"} {
		req := httptest.NewRequest(http.MethodGet, "/api/v1/folders?path="+root, nil)
		req.Host = host
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		if rec.Code != 200 {
			t.Fatalf("host %s: %d %s", host, rec.Code, rec.Body)
		}
	}
}
