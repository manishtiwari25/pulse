package hub

import (
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// The folder picker behind Add project (WU-023 under ADR-027): the browser
// cannot learn a real path from its own file dialog, but the hub runs on the
// owner's machine, so it lists folders and the dialog browses them. Only
// directories, never hidden entries, and only for a loopback Host.

const folderLimit = 1000

type folderEntry struct {
	Name       string `json:"name"`
	Path       string `json:"path"`
	Checkout   bool   `json:"checkout"`
	Registered string `json:"registered"`
}

type folderRoot struct {
	Label string `json:"label"`
	Path  string `json:"path"`
}

type folderListing struct {
	Path       string        `json:"path"`
	Parent     string        `json:"parent"`
	Home       string        `json:"home"`
	Roots      []folderRoot  `json:"roots"`
	Checkout   bool          `json:"checkout"`
	Registered string        `json:"registered"`
	Inside     string        `json:"inside"`
	Entries    []folderEntry `json:"entries"`
	Truncated  bool          `json:"truncated"`
}

// loopbackHost reports whether the request was addressed to this machine by
// name; a page that rebinds its own hostname to 127.0.0.1 is refused.
func loopbackHost(r *http.Request) bool {
	host := r.Host
	if h, _, err := net.SplitHostPort(host); err == nil {
		host = h
	}
	host = strings.Trim(host, "[]")
	if strings.EqualFold(host, "localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func (h *Handler) additionFolders(w http.ResponseWriter, r *http.Request, parts []string) {
	if len(parts) != 1 {
		additionNotFound(w)
		return
	}
	if !additionMethod(w, r, http.MethodGet) || !h.additionAvailable(w, r, "projects") {
		return
	}
	if !loopbackHost(r) {
		writeError(w, http.StatusForbidden, "Folders are listed only on loopback", "The request named a host other than this machine.", "Open the hub on 127.0.0.1.")
		return
	}
	projects, err := h.Store.Projects(false)
	if err != nil {
		additionFailure(w, err)
		return
	}
	home, _ := os.UserHomeDir()
	registered := map[string]string{}
	roots := []folderRoot{}
	if home != "" {
		roots = append(roots, folderRoot{Label: "Home", Path: home})
	}
	seen := map[string]bool{}
	start := home
	for _, p := range projects {
		if p.Path == "" || p.Slug == store.CommonsSlug {
			continue
		}
		registered[filepath.Clean(p.Path)] = p.Slug
		parent := filepath.Dir(filepath.Clean(p.Path))
		if parent == filepath.Dir(parent) {
			continue
		}
		if start == home {
			start = parent
		}
		if parent != home && !seen[parent] {
			seen[parent] = true
			roots = append(roots, folderRoot{Label: filepath.Base(parent), Path: parent})
		}
	}

	path := strings.TrimSpace(r.URL.Query().Get("path"))
	switch {
	case path == "":
		path = start
	case path == "~" || strings.HasPrefix(path, "~/"):
		path = home + path[1:]
	}
	if !filepath.IsAbs(path) {
		writeError(w, http.StatusBadRequest, "Not an absolute path", "Folders are listed from an absolute path.", "Start the path with / or ~.")
		return
	}
	path = filepath.Clean(path)
	st, err := os.Stat(path)
	if err != nil {
		if os.IsNotExist(err) {
			writeError(w, http.StatusNotFound, "No folder "+path, "Nothing exists at that path on this machine.", "Pick a folder that exists or go up one level.")
			return
		}
		additionFailure(w, cli.Errorf(2, "Cannot read folder "+path, err.Error(), "Pick a folder this hub may read."))
		return
	}
	if !st.IsDir() {
		writeError(w, http.StatusBadRequest, "Not a folder: "+path, "That path is a file.", "Pick the folder that contains it.")
		return
	}
	dirents, err := os.ReadDir(path)
	if err != nil {
		additionFailure(w, cli.Errorf(2, "Cannot read folder "+path, err.Error(), "Pick a folder this hub may read."))
		return
	}

	listing := folderListing{Path: path, Home: home, Roots: roots, Checkout: isCheckout(path), Registered: registered[path], Inside: insideProject(registered, path), Entries: []folderEntry{}}
	if parent := filepath.Dir(path); parent != path {
		listing.Parent = parent
	}
	for _, d := range dirents {
		name := d.Name()
		if strings.HasPrefix(name, ".") {
			continue
		}
		full := filepath.Join(path, name)
		if !d.IsDir() {
			// A symlink to a folder counts as a folder; anything else is skipped.
			if d.Type()&os.ModeSymlink == 0 {
				continue
			}
			if st, err := os.Stat(full); err != nil || !st.IsDir() {
				continue
			}
		}
		listing.Entries = append(listing.Entries, folderEntry{Name: name, Path: full, Checkout: isCheckout(full), Registered: registered[full]})
	}
	sort.Slice(listing.Entries, func(i, j int) bool {
		a, b := strings.ToLower(listing.Entries[i].Name), strings.ToLower(listing.Entries[j].Name)
		if a == b {
			return listing.Entries[i].Name < listing.Entries[j].Name
		}
		return a < b
	})
	if len(listing.Entries) > folderLimit {
		listing.Entries, listing.Truncated = listing.Entries[:folderLimit], true
	}
	writeJSON(w, http.StatusOK, listing)
}

// insideProject is the registered project whose checkout contains path - a
// held-out folder must not sit there (ADR-032). The checkout itself is
// Registered, not Inside; nested checkouts resolve to the innermost.
func insideProject(registered map[string]string, path string) string {
	inside, depth := "", 0
	for root, slug := range registered {
		if root != path && strings.HasPrefix(path, root+string(filepath.Separator)) && len(root) > depth {
			inside, depth = slug, len(root)
		}
	}
	return inside
}

// isCheckout reports a .git entry (a directory, or the file a worktree keeps).
func isCheckout(dir string) bool {
	_, err := os.Lstat(filepath.Join(dir, ".git"))
	return err == nil
}
