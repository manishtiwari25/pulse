package evolve

import (
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

const maxFixtureBytes = 1 << 20
const templateMarker = "PULSE_HELDOUT_TEMPLATE"

type privateFixture struct {
	request string
	scorer  string
}

// HeldoutState is deliberately counts-only, including diagnostics.
type HeldoutState struct {
	Count   int    `json:"count"`
	Pending int    `json:"pending"`
	Problem string `json:"problem,omitempty"`
}

type SetupResult struct {
	HeldoutState
	Created int `json:"created"`
}

func canonicalPath(value string) (string, error) {
	if value == "~" || strings.HasPrefix(value, "~/") {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", err
		}
		if value == "~" {
			value = home
		} else {
			value = filepath.Join(home, strings.TrimPrefix(value, "~/"))
		}
	}
	abs, err := filepath.Abs(value)
	if err != nil {
		return "", err
	}
	probe := abs
	var tail []string
	for {
		_, err := os.Lstat(probe)
		if err == nil {
			real, err := filepath.EvalSymlinks(probe)
			if err != nil {
				return "", err
			}
			for i := len(tail) - 1; i >= 0; i-- {
				real = filepath.Join(real, tail[i])
			}
			return real, nil
		}
		if !errors.Is(err, os.ErrNotExist) {
			return "", err
		}
		parent := filepath.Dir(probe)
		if parent == probe {
			return "", err
		}
		tail = append(tail, filepath.Base(probe))
		probe = parent
	}
}

func within(child, parent string) bool {
	rel, err := filepath.Rel(parent, child)
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) && !filepath.IsAbs(rel)
}

func outsideProjects(s *store.Store, target string) error {
	projects, err := s.Projects(false)
	if err != nil {
		return err
	}
	for _, p := range projects {
		if p.Path == "" {
			continue
		}
		root, err := canonicalPath(p.Path)
		if err != nil {
			return errors.New("cannot validate all registered project boundaries")
		}
		if within(target, root) {
			return errors.New("the directory is inside a registered project")
		}
	}
	return nil
}

func privateError(why string) error {
	return inputError("Private fixture set is not ready", why, "pulse evolve setup DIR")
}

func privateDir(s *store.Store, value string) (string, error) {
	if strings.TrimSpace(value) == "" {
		return "", privateError("no owner-private directory is configured")
	}
	if store.SecretShaped(value) {
		return "", privateError("the directory identifier must not contain credentials")
	}
	dir, err := canonicalPath(value)
	if err != nil {
		return "", privateError("cannot resolve the owner-private directory")
	}
	if err := outsideProjects(s, dir); err != nil {
		return "", privateError("the directory must be outside every registered project")
	}
	fleet, err := canonicalPath(filepath.Join(s.Home, "fleet"))
	if err != nil || within(dir, fleet) || within(fleet, dir) {
		return "", privateError("the directory must not overlap the disposable fleet area")
	}
	return dir, nil
}

func privateRegular(root *os.Root, name string, executable bool) ([]byte, error) {
	info, err := root.Lstat(name)
	if err != nil || !info.Mode().IsRegular() || info.Mode().Perm()&0o077 != 0 ||
		(executable && info.Mode().Perm()&0o100 == 0) || info.Size() > maxFixtureBytes || multipleLinks(info) {
		return nil, privateError("each pair needs regular owner-only request and executable scorer files")
	}
	f, err := root.Open(name)
	if err != nil {
		return nil, privateError("cannot read a private fixture file")
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, maxFixtureBytes+1))
	if err != nil || len(data) > maxFixtureBytes {
		return nil, privateError("cannot read a bounded private fixture")
	}
	return data, nil
}

func loadPrivate(s *store.Store) ([]privateFixture, HeldoutState, error) {
	return loadPrivateAt(s, s.Setting("evolve.heldout_dir", ""))
}

func loadPrivateAt(s *store.Store, value string) ([]privateFixture, HeldoutState, error) {
	state := HeldoutState{}
	if value == "" {
		return nil, state, nil
	}
	dir, err := privateDir(s, value)
	if err != nil {
		return nil, state, err
	}
	info, err := os.Stat(dir)
	if err != nil || !info.IsDir() || info.Mode().Perm()&0o077 != 0 {
		return nil, state, privateError("the owner-private directory must exist with mode 0700")
	}
	root, err := os.OpenRoot(dir)
	if err != nil {
		return nil, state, privateError("cannot open the owner-private directory")
	}
	defer root.Close()
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, state, privateError("cannot inspect the private fixture set")
	}
	requests, scorers := map[string]bool{}, map[string]bool{}
	for _, ent := range entries {
		name := ent.Name()
		if strings.HasSuffix(name, ".request") {
			requests[strings.TrimSuffix(name, ".request")] = true
		}
		if strings.HasSuffix(name, ".score") {
			scorers[strings.TrimSuffix(name, ".score")] = true
		}
	}
	if len(requests) != len(scorers) {
		return nil, state, privateError("every private request must have exactly one scorer")
	}
	out := []privateFixture{}
	for _, ent := range entries {
		if !strings.HasSuffix(ent.Name(), ".request") {
			continue
		}
		stem := strings.TrimSuffix(ent.Name(), ".request")
		if stem == "" || !scorers[stem] {
			return nil, state, privateError("every private request must have exactly one scorer")
		}
		request, err := privateRegular(root, ent.Name(), false)
		if err != nil {
			return nil, state, err
		}
		scorer, err := privateRegular(root, stem+".score", true)
		if err != nil {
			return nil, state, err
		}
		if store.SecretShaped(string(request)) || store.SecretShaped(string(scorer)) {
			return nil, state, privateError("private requests and scorers must not contain credentials")
		}
		if strings.TrimSpace(string(request)) == "" || strings.TrimSpace(string(scorer)) == "" ||
			strings.ContainsRune(string(request), 0) || strings.ContainsRune(string(scorer), 0) {
			return nil, state, privateError("private requests and scorers must contain nonempty text")
		}
		if strings.Contains(string(request), templateMarker) || strings.Contains(string(scorer), templateMarker) {
			state.Pending++
			continue
		}
		out = append(out, privateFixture{request: string(request), scorer: filepath.Join(dir, stem+".score")})
		state.Count++
	}
	return out, state, nil
}

// HeldoutCount does not expose a private name, path, request, or scorer.
func HeldoutCount(s *store.Store) HeldoutState {
	_, state, err := loadPrivate(s)
	if err != nil {
		state.Problem = "private fixture validation failed; owner: pulse evolve setup DIR"
	}
	return state
}

// Setup creates two non-overwriting private templates and records only the
// directory setting. It never enables the loop or writes a baseline.
func Setup(s *store.Store, value string) (*SetupResult, error) {
	dir, err := privateDir(s, value)
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, privateError("cannot create the owner-private directory")
	}
	info, err := os.Stat(dir)
	if err != nil || !info.IsDir() || info.Mode().Perm()&0o077 != 0 {
		return nil, privateError("use an owner-private directory with mode 0700")
	}
	root, err := os.OpenRoot(dir)
	if err != nil {
		return nil, privateError("cannot open the owner-private directory")
	}
	defer root.Close()
	result := &SetupResult{}
	for i := 1; i <= 2; i++ {
		for _, item := range []struct {
			suffix, body string
			mode         os.FileMode
		}{
			{".request", templateMarker + "\nReplace this template with a secret-free request.\n", 0o600},
			{".score", "#!/bin/sh\n# " + templateMarker + "\n# Replace this template with a read-only scorer printing a number in [0,1].\nexit 2\n", 0o700},
		} {
			name := fmt.Sprintf("H%02d%s", i, item.suffix)
			f, err := root.OpenFile(name, os.O_WRONLY|os.O_CREATE|os.O_EXCL, item.mode)
			if errors.Is(err, os.ErrExist) {
				if _, err := privateRegular(root, name, item.suffix == ".score"); err != nil {
					return nil, err
				}
				continue
			}
			if err != nil {
				return nil, privateError("cannot create a private template; existing files were not overwritten")
			}
			_, writeErr := io.WriteString(f, item.body)
			if err := errors.Join(writeErr, f.Close()); err != nil {
				return nil, privateError("cannot write a private template")
			}
			result.Created++
		}
	}
	_, result.HeldoutState, err = loadPrivateAt(s, dir)
	if err != nil {
		return nil, err
	}
	// The canonical helper owns this human-attributed setting event. Validate
	// first, so a malformed set does not replace a working configuration.
	if err := s.SetSetting(store.HumanAgent, "evolve.heldout_dir", dir); err != nil {
		return nil, privateError("cannot save the owner-private directory setting")
	}
	return result, nil
}
