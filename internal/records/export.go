package records

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

type ExportOptions struct {
	Kind    string
	Project string
	Force   bool
}

// Export writes a human-requested Markdown snapshot. os.Root prevents a stored
// path or a symlink below DIR from escaping the chosen export directory.
func Export(s *store.Store, dir string, opts ExportOptions) (int, error) {
	if strings.TrimSpace(dir) == "" {
		return 0, inputError("Cannot export records", "an output directory is required", "pulse record export /tmp/pulse-records")
	}
	target, err := resolvedTarget(dir)
	if err != nil {
		return 0, err
	}
	if !opts.Force {
		checkout, err := insideCheckout(target)
		if err != nil {
			return 0, err
		}
		if checkout {
			return 0, inputError("Refusing Markdown export inside a git checkout", "SQL is the system of record; export outside the repository or explicitly use --force", "pulse record export /tmp/pulse-records")
		}
	}
	rows, err := List(s, store.RecordFilter{Project: opts.Project, Kind: opts.Kind})
	if err != nil {
		return 0, err
	}
	type output struct{ name, text string }
	outputs := []output{}
	seen := map[string]string{}
	// Complete collision/path preflight before creating any files.
	for _, rec := range rows {
		name := rec.Path
		if name == "" {
			name = rec.Kind + "/" + rec.ID + ".md"
		}
		name = filepath.FromSlash(name)
		if !filepath.IsLocal(name) || strings.Contains(name, "\\") {
			return 0, fmt.Errorf("cannot export %s: unsafe record path %q", rec.ID, name)
		}
		name = filepath.Clean(name)
		if first, exists := seen[name]; exists {
			return 0, inputError("Cannot export colliding paths", first+" and "+rec.ID+" both use "+name, "pulse record export "+dir+" --project "+rec.Project)
		}
		seen[name] = rec.ID
		_, rev, err := s.GetRecord(rec.ID, 0)
		if err != nil {
			return 0, err
		}
		if !opts.Force {
			checkout, err := insideCheckout(filepath.Dir(filepath.Join(target, name)))
			if err != nil {
				return 0, err
			}
			if checkout {
				return 0, inputError("Refusing Markdown export inside a git checkout", "a nested checkout contains "+name, "pulse record export /tmp/pulse-records")
			}
		}
		outputs = append(outputs, output{name: name, text: RenderMarkdown(&rec, rev)})
	}
	if err := os.MkdirAll(target, 0o755); err != nil {
		return 0, err
	}
	root, err := os.OpenRoot(target)
	if err != nil {
		return 0, err
	}
	defer root.Close()
	written := 0
	for _, file := range outputs {
		if err := root.MkdirAll(filepath.Dir(file.name), 0o755); err != nil {
			return written, err
		}
		if err := root.WriteFile(file.name, []byte(file.text), 0o644); err != nil {
			return written, fmt.Errorf("export %s: %w", file.name, err)
		}
		written++
	}
	return written, nil
}

// Resolve existing ancestors as well as DIR itself; a symlink to a checkout
// must not bypass the no-Markdown-in-the-system export default.
func resolvedTarget(dir string) (string, error) {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return "", err
	}
	existing := abs
	var tail []string
	for {
		resolved, err := filepath.EvalSymlinks(existing)
		if err == nil {
			for i := len(tail) - 1; i >= 0; i-- {
				resolved = filepath.Join(resolved, tail[i])
			}
			return resolved, nil
		}
		if !errors.Is(err, os.ErrNotExist) {
			return "", err
		}
		parent := filepath.Dir(existing)
		if parent == existing {
			return "", err
		}
		tail = append(tail, filepath.Base(existing))
		existing = parent
	}
}

func insideCheckout(target string) (bool, error) {
	resolved, err := resolvedTarget(target)
	if err != nil {
		return false, err
	}
	for dir := resolved; ; dir = filepath.Dir(dir) {
		if _, err := os.Stat(filepath.Join(dir, ".git")); err == nil {
			return true, nil
		} else if !errors.Is(err, os.ErrNotExist) {
			return false, err
		}
		if filepath.Dir(dir) == dir {
			return false, nil
		}
	}
}
