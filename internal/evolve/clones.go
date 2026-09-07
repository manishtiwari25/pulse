package evolve

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

var defaultAnswerKeys = []string{
	".pulse",
	"docs/evolution/evaluators.md",
	"docs/evolution/archive.md",
	"docs/board/evolution",
	"docs/board/governance",
	"docs/memory/mistakes.md",
}

// ViolationError is a failed isolation or read-only constraint, not a low
// evaluator score. A runner must discard it rather than archive it as a pass.
type ViolationError struct{ Reason string }

func (e *ViolationError) Error() string { return "evolution constraint violation: " + e.Reason }

func projectAnswerKeys(s *store.Store, project string) ([]string, error) {
	fixtures, err := ListFixtures(s, project, false)
	if err != nil {
		return nil, err
	}
	seen := map[string]bool{}
	for _, key := range defaultAnswerKeys {
		seen[key] = true
	}
	for _, f := range fixtures {
		for _, key := range f.AnswerKeys {
			k, err := relativePath(key)
			if err != nil {
				return nil, &ViolationError{Reason: "an answer-key path escapes the clone"}
			}
			seen[k] = true
		}
	}
	out := make([]string, 0, len(seen))
	for key := range seen {
		out = append(out, key)
	}
	sort.Strings(out)
	return out, nil
}

func fleetRoot(s *store.Store, create bool) (string, error) {
	home, err := canonicalPath(s.Home)
	if err != nil {
		return "", err
	}
	root := filepath.Join(home, "fleet")
	resolved, err := canonicalPath(root)
	if err != nil || resolved != root {
		return "", inputError("Cannot use fleet directory", "the managed fleet directory must not be a symlink", "pulse --home DIR fleet list")
	}
	if err := outsideProjects(s, root); err != nil {
		return "", inputError("Cannot use fleet directory", "PULSE_HOME/fleet must be outside every registered project", "pulse --home DIR fleet prepare --project P --n 1")
	}
	if private := s.Setting("evolve.heldout_dir", ""); private != "" {
		dir, err := canonicalPath(private)
		if err != nil || within(dir, root) || within(root, dir) {
			return "", privateError("private fixtures must not overlap the disposable fleet area")
		}
	}
	if create {
		if err := os.MkdirAll(root, 0o700); err != nil {
			return "", err
		}
	}
	return root, nil
}

func resolveRevision(ctx context.Context, repo, ref string) (string, string, error) {
	root, err := canonicalPath(repo)
	if err != nil {
		return "", "", inputError("Cannot measure checkout", "the checkout path cannot be resolved", "pulse projects")
	}
	if ref == "" {
		ref = "HEAD"
	}
	commit, err := gitOutput(ctx, root, "rev-parse", "--verify", "--end-of-options", ref+"^{commit}")
	if err != nil || len(commit) < 40 || strings.ContainsAny(commit, "\n\r ") {
		if ctx.Err() != nil {
			return "", "", ctx.Err()
		}
		return "", "", inputError("Cannot resolve measurement revision", "the requested ref must name a local commit", "git rev-parse --verify HEAD")
	}
	return root, commit, nil
}

func strippedClone(ctx context.Context, repo, commit, dest string, keys []string) error {
	if within(dest, repo) {
		return &ViolationError{Reason: "a clone destination is inside its source repository"}
	}
	if _, err := os.Lstat(dest); !errors.Is(err, os.ErrNotExist) {
		return inputError("Cannot prepare clone", "the clone path must not already exist", "pulse fleet list")
	}
	if _, err := gitOutput(ctx, "", "clone", "--quiet", "--no-hardlinks", "--no-tags", "--no-checkout", "--", repo, dest); err != nil {
		return cloneFailure(ctx, "clone")
	}
	if _, err := gitOutput(ctx, dest, "checkout", "--quiet", "--detach", commit); err != nil {
		return cloneFailure(ctx, "detach the clone at the requested revision")
	}
	if _, err := gitOutput(ctx, dest, "remote", "remove", "origin"); err != nil {
		return cloneFailure(ctx, "remove the clone's remote")
	}
	tags, err := gitOutput(ctx, dest, "for-each-ref", "--format=%(refname)", "refs/tags/")
	if err != nil {
		return cloneFailure(ctx, "inspect clone tags")
	}
	for _, tag := range strings.Fields(tags) {
		if _, err := gitOutput(ctx, dest, "update-ref", "-d", tag); err != nil {
			return cloneFailure(ctx, "remove clone tags")
		}
	}
	root, err := os.OpenRoot(dest)
	if err != nil {
		return err
	}
	defer root.Close()
	for _, key := range keys {
		key, err := relativePath(key)
		if err != nil {
			return &ViolationError{Reason: "an answer-key path escapes the clone"}
		}
		if err := root.RemoveAll(filepath.FromSlash(key)); err != nil {
			return &ViolationError{Reason: "cannot strip an answer-key path"}
		}
	}
	if err := hideStripped(ctx, dest, keys); err != nil {
		return err
	}
	return checkClone(ctx, dest, keys)
}

// hideStripped marks the tracked files under the stripped answer-key paths
// skip-worktree in the clone's index. Git then treats their absence as nothing
// to report - a scorer's git status stays clean and git checkout -- . does not
// bring them back - while the measured commit itself is untouched. Without it
// a repository that tracks its evaluators or archive shows every stripped key
// as a deleted path, and every starter rule reads that as the harness's work.
func hideStripped(ctx context.Context, dest string, keys []string) error {
	if len(keys) == 0 {
		return nil
	}
	listed, err := gitRaw(ctx, dest, append([]string{"--literal-pathspecs", "ls-files", "-z", "--cached", "--"}, keys...)...)
	if err != nil {
		return cloneFailure(ctx, "list the stripped answer-key paths")
	}
	paths := []string{}
	for _, p := range strings.Split(listed, "\x00") {
		if p != "" {
			paths = append(paths, p)
		}
	}
	for len(paths) > 0 {
		n := min(len(paths), 64)
		if _, err := gitOutput(ctx, dest, append([]string{"update-index", "--skip-worktree", "--"}, paths[:n]...)...); err != nil {
			return cloneFailure(ctx, "hide the stripped answer-key paths")
		}
		paths = paths[n:]
	}
	return nil
}

func cloneFailure(ctx context.Context, step string) error {
	if ctx.Err() != nil {
		return ctx.Err()
	}
	return inputError("Cannot prepare isolated clone", "git could not "+step, "pulse fleet prepare --project P --n 1 --ref HEAD")
}

func checkClone(ctx context.Context, dest string, keys []string) error {
	info, err := os.Lstat(filepath.Join(dest, ".git"))
	if err != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return &ViolationError{Reason: "clone Git metadata is not a local directory"}
	}
	head, err := gitOutput(ctx, dest, "symbolic-ref", "--quiet", "HEAD")
	if err == nil || head != "" {
		return &ViolationError{Reason: "clone is no longer detached"}
	}
	// symbolic-ref's exit 1 means detached; a corrupt repository must not pass.
	if _, err := gitOutput(ctx, dest, "rev-parse", "--verify", "HEAD"); err != nil {
		return &ViolationError{Reason: "clone HEAD is unreadable"}
	}
	for _, args := range [][]string{{"remote"}, {"tag", "--list"}} {
		out, err := gitOutput(ctx, dest, args...)
		if err != nil || out != "" {
			return &ViolationError{Reason: "clone has a remote or tag, or cannot be inspected"}
		}
	}
	root, err := os.OpenRoot(dest)
	if err != nil {
		return err
	}
	defer root.Close()
	for _, key := range keys {
		_, err := root.Lstat(filepath.FromSlash(key))
		if !errors.Is(err, os.ErrNotExist) {
			return &ViolationError{Reason: "a stripped answer-key path is present or cannot be inspected"}
		}
	}
	return nil
}

// removeManaged removes one resolved child of Home/fleet, never a supplied
// filesystem path or a manifest's arbitrary deletion targets. Root prevents
// symlink traversal during cleanup; modes are repaired without following links.
func removeManaged(s *store.Store, id string) error {
	if !managedID.MatchString(id) {
		return inputError("Cannot clean fleet", "expected a fleet id, not a path", "pulse fleet list")
	}
	base, err := fleetRoot(s, false)
	if err != nil {
		return err
	}
	root, err := os.OpenRoot(base)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	defer root.Close()
	info, err := root.Lstat(id)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if info.IsDir() {
		child, err := root.OpenRoot(id)
		if err != nil {
			return err
		}
		walkErr := fs.WalkDir(child.FS(), ".", func(name string, entry fs.DirEntry, walkErr error) error {
			if walkErr != nil {
				return walkErr
			}
			if entry.IsDir() {
				return child.Chmod(name, 0o700)
			}
			return nil
		})
		closeErr := child.Close()
		if err := errors.Join(walkErr, closeErr); err != nil {
			return fmt.Errorf("cannot make managed clone removable: %w", err)
		}
	}
	return root.RemoveAll(id)
}
