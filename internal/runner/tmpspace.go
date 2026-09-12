package runner

import (
	"fmt"
	"os"
	"syscall"
)

// minTmpSpace is the free space the worker's TMPDIR must have before launch:
// `go build`/`go test` write caches and binaries there and fail mid-run with
// ENOSPC when it is full, after the budget has been spent.
const minTmpSpace = 512 << 20

// workerTmpDir is the TMPDIR the worker inherits: the TMPDIR entry of env
// (base process env overlaid with extra) or os.TempDir's default.
func workerTmpDir(env []string) string {
	dir := ""
	for _, kv := range env {
		if len(kv) > len("TMPDIR=") && kv[:len("TMPDIR=")] == "TMPDIR=" {
			dir = kv[len("TMPDIR="):]
		}
	}
	if dir == "" {
		return os.TempDir()
	}
	return dir
}

// freeSpace reports the bytes available to unprivileged writers on the
// filesystem holding dir.
var freeSpace = func(dir string) (uint64, error) {
	var st syscall.Statfs_t
	if err := syscall.Statfs(dir, &st); err != nil {
		return 0, err
	}
	return uint64(st.Bavail) * uint64(st.Bsize), nil
}

// preflightTmpSpace fails before the worker starts when dir has less than min
// bytes free, so a full disk is a setup error (job back to the queue, nothing
// spent) instead of an ENOSPC halfway through `go test`. A dir that cannot be
// stat'ed is also a setup error: the worker could not write there either.
func preflightTmpSpace(dir string, min uint64) error {
	free, err := freeSpace(dir)
	if err != nil {
		return fail(1, "setup-error: worker TMPDIR %s is unusable (%v); the worker was not started so no work is lost", dir, err)
	}
	if free < min {
		return fail(1, "setup-error: worker TMPDIR %s has %s free, below the %s the worker needs; the worker was not started so no work is lost", dir, humanBytes(free), humanBytes(min))
	}
	return nil
}

func humanBytes(n uint64) string {
	switch {
	case n >= 1<<30:
		return fmt.Sprintf("%.1f GiB", float64(n)/float64(1<<30))
	case n >= 1<<20:
		return fmt.Sprintf("%.1f MiB", float64(n)/float64(1<<20))
	}
	return fmt.Sprintf("%d B", n)
}
