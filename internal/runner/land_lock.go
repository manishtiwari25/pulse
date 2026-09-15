package runner

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

// .git/index.lock in the owner's checkout (ADR-024 D1: the daemon only fast-forwards that checkout, and defers
// otherwise). Git holds the file for the seconds of a write, so a lock that outlives every git process is the
// wreck of a killed command - a timeout's SIGKILL leaves it behind - and it holds every later landing with
// "Unable to create index.lock: File exists" while each tick redoes the whole merge and check only to fail at
// the last step. Age alone never decides (the fleet's lesson): the lock is set aside only under the landing's own
// per-checkout mutex, on a clean tree, when it is older than that mutex's own staleness, when no process has
// it open, and when the file rechecked by identity is the one inspected. It is quarantined beside the index,
// never deleted, so nothing git wrote is lost.
const indexLockMinutes = landLockMinutes

// abandonedLock is an index lock the landing set aside.
type abandonedLock struct {
	From string        `json:"from"`
	To   string        `json:"to"`
	Age  time.Duration `json:"-"`
	Size int64         `json:"size"`
}

func (a abandonedLock) detail() map[string]any {
	return map[string]any{"from": a.From, "to": a.To, "age": a.Age.Round(time.Second).String(), "size": a.Size}
}

// indexLockPath is where git keeps the index lock for this checkout; a linked worktree's .git is a file, so git
// is asked first.
func indexLockPath(root string) string {
	if res := gitTimeout(root, 10*time.Second, "rev-parse", "--git-path", "index.lock"); res.ok() {
		if p := strings.TrimSpace(res.Stdout); p != "" {
			if !filepath.IsAbs(p) {
				p = filepath.Join(root, p)
			}
			return p
		}
	}
	return filepath.Join(root, ".git", "index.lock")
}

// lockHolders names the processes that have path open: /proc where it exists, lsof elsewhere. known is false
// when this machine cannot say, and then nothing is touched.
var lockHolders = func(path string) (pids []int, known bool) {
	if entries, err := os.ReadDir("/proc"); err == nil {
		for _, e := range entries {
			pid, err := strconv.Atoi(e.Name())
			if err != nil {
				continue
			}
			fds, err := os.ReadDir(filepath.Join("/proc", e.Name(), "fd"))
			if err != nil {
				continue
			}
			for _, fd := range fds {
				if target, err := os.Readlink(filepath.Join("/proc", e.Name(), "fd", fd.Name())); err == nil && target == path {
					pids = append(pids, pid)
					break
				}
			}
		}
		return pids, true
	}
	if _, err := exec.LookPath("lsof"); err != nil {
		return nil, false
	}
	out, err := exec.Command("lsof", "-t", "--", path).Output()
	if err != nil {
		if ee, ok := err.(*exec.ExitError); ok && ee.ExitCode() == 1 && len(out) == 0 {
			return nil, true // lsof exits 1 when nothing has the file open
		}
		return nil, false
	}
	for _, f := range strings.Fields(string(out)) {
		if pid, err := strconv.Atoi(f); err == nil {
			pids = append(pids, pid)
		}
	}
	return pids, true
}

// freeIndexLock clears the way for the landing's write into root, which must already hold the landing mutex and
// be clean. "" means the checkout is free; otherwise the note says why the landing is deferred. moved names the
// abandoned lock that was set aside, if any.
func freeIndexLock(root string, now time.Time) (note string, moved *abandonedLock) {
	path := indexLockPath(root)
	fi, err := os.Lstat(path)
	if err != nil {
		return "", nil
	}
	rel := filepath.Join(".git", "index.lock")
	age := now.Sub(fi.ModTime())
	if age < indexLockMinutes*time.Minute {
		return fmt.Sprintf("the checkout of %s is mid-write (%s is %s old); nothing landed, try again", root, rel, age.Round(time.Second)), nil
	}
	pids, known := lockHolders(path)
	if !known {
		return fmt.Sprintf("the checkout of %s holds a %s-old %s and this machine cannot say whose it is; remove it by hand once no git runs there",
			root, age.Round(time.Minute), rel), nil
	}
	if len(pids) > 0 {
		return fmt.Sprintf("the checkout of %s has %s open in process %d; nothing landed, try again", root, rel, pids[0]), nil
	}
	again, err := os.Lstat(path)
	if err != nil {
		return "", nil // gone on its own
	}
	if !os.SameFile(fi, again) || !again.ModTime().Equal(fi.ModTime()) || again.Size() != fi.Size() {
		return fmt.Sprintf("%s in the checkout of %s changed while it was inspected; nothing landed, try again", rel, root), nil
	}
	to := filepath.Join(filepath.Dir(path), "pulse-quarantine-index.lock-"+now.UTC().Format("20060102T150405Z"))
	if err := os.Rename(path, to); err != nil {
		return fmt.Sprintf("the checkout of %s holds an abandoned %s that could not be set aside (%s); remove it by hand", root, rel, errText(err, "rename failed")), nil
	}
	return "", &abandonedLock{From: path, To: to, Age: age, Size: fi.Size()}
}

// lockedCheckout tells whether a git failure was the index lock, so the landing names the lock instead of
// blaming a moved branch.
func lockedCheckout(stderr string) bool {
	return strings.Contains(stderr, "index.lock")
}
