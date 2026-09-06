package runner

import (
	"fmt"
	"io"
	"os"
	"runtime"
	"sort"
	"strings"
	"sync"
	"time"
)

// checkoutProbeTimeout bounds how long one daemon loop waits to learn whether a project's checkout answers at
// all. A stat that does not return in time is not an error the loop can act on - on macOS it is a consent
// dialog for the Desktop/Documents/Downloads folder blocking the syscall until the owner answers, elsewhere
// an unmounted volume - so the project is held this loop and the probe is left running: when it finally
// answers, the next loop sees the checkout again. Everything that would touch the path (landing, the
// constitution file fallback, leads, planning, leasing) skips a held project instead of blocking the loop
// or failing jobs against it.
const checkoutProbeTimeout = 5 * time.Second

// checkoutProbes remembers the stats still in flight so a stalled path costs one goroutine, not one per loop.
type checkoutProbes struct {
	pending map[string]chan error
	noted   map[string]string
}

// ready splits projects into those whose path answered os.Stat as a directory within timeout and the held
// ones with their reason (one line each, only the first time a reason is seen so the log does not repeat
// every loop). The hold is also published for takeJob through holdCheckouts.
func (p *checkoutProbes) ready(projects []row, timeout time.Duration) (ok []row, held map[string]string, lines []string) {
	if p.pending == nil {
		p.pending = map[string]chan error{}
		p.noted = map[string]string{}
	}
	held = map[string]string{}
	deadline := time.Now().Add(timeout)
	for _, proj := range projects {
		path, slug := proj.str("path"), proj.str("slug")
		ch, has := p.pending[path]
		if !has {
			ch = make(chan error, 1)
			p.pending[path] = ch
			go func(path string, ch chan error) { ch <- readDirOnce(path) }(path, ch)
		}
		wait := time.Until(deadline)
		if wait < 0 {
			wait = 0
		}
		timer := time.NewTimer(wait)
		select {
		case err := <-ch:
			timer.Stop()
			delete(p.pending, path)
			if err == nil {
				ok = append(ok, proj)
				delete(p.noted, slug)
				continue
			}
			held[slug] = fmt.Sprintf("cannot read %s: %s", path, err)
		case <-timer.C:
			held[slug] = fmt.Sprintf("%s has not answered for %s%s", path, timeout, macOSHint(path))
		}
	}
	slugs := make([]string, 0, len(held))
	for slug := range held {
		slugs = append(slugs, slug)
	}
	sort.Strings(slugs)
	for _, slug := range slugs {
		if p.noted[slug] != held[slug] {
			p.noted[slug] = held[slug]
			lines = append(lines, fmt.Sprintf("holding %s this loop: %s", slug, held[slug]))
		}
	}
	holdCheckouts(held)
	return ok, held, lines
}

// readDirOnce opens the checkout and lists one entry: a stat alone answers for a guarded macOS folder while
// the consent dialog is still open, listing it does not.
func readDirOnce(path string) error {
	fi, err := os.Stat(path)
	if err != nil {
		return err
	}
	if !fi.IsDir() {
		return fmt.Errorf("%s is not a directory", path)
	}
	d, err := os.Open(path)
	if err != nil {
		return err
	}
	defer d.Close()
	if _, err := d.Readdirnames(1); err != nil && err != io.EOF {
		return err
	}
	return nil
}

func macOSHint(path string) string {
	if runtime.GOOS != "darwin" {
		return ""
	}
	for _, f := range protectedFolders {
		if strings.Contains(path, "/"+f+"/") {
			return " (a macOS dialog is asking whether pulse may access your " + f + " folder: answer Allow)"
		}
	}
	return ""
}

var checkoutHold = struct {
	sync.Mutex
	m map[string]string
}{m: map[string]string{}}

// holdCheckouts replaces the set of projects whose checkout the daemon could not read this loop.
func holdCheckouts(held map[string]string) {
	checkoutHold.Lock()
	defer checkoutHold.Unlock()
	checkoutHold.m = map[string]string{}
	for k, v := range held {
		checkoutHold.m[k] = v
	}
}

// heldCheckouts is a copy of that set; empty outside the daemon.
func heldCheckouts() map[string]string {
	checkoutHold.Lock()
	defer checkoutHold.Unlock()
	out := make(map[string]string, len(checkoutHold.m))
	for k, v := range checkoutHold.m {
		out[k] = v
	}
	return out
}
