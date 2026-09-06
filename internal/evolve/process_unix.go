//go:build unix

package evolve

import (
	"errors"
	"os"
	"os/exec"
	"syscall"
)

func containProcess(cmd *exec.Cmd) func() {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	kill := func() error {
		if cmd.Process == nil {
			return nil
		}
		err := syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
		if errors.Is(err, syscall.ESRCH) {
			return os.ErrProcessDone
		}
		return err
	}
	cmd.Cancel = kill
	return func() { _ = kill() }
}

func multipleLinks(info os.FileInfo) bool {
	if info == nil || !info.Mode().IsRegular() {
		return false
	}
	st, ok := info.Sys().(*syscall.Stat_t)
	return ok && st.Nlink > 1
}

func stopSignals() []os.Signal { return []os.Signal{os.Interrupt, syscall.SIGTERM} }
