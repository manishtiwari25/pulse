//go:build !unix

package evolve

import (
	"os"
	"os/exec"
)

func containProcess(cmd *exec.Cmd) func() {
	return func() {
		if cmd.Process != nil {
			_ = cmd.Process.Kill()
		}
	}
}

func multipleLinks(info os.FileInfo) bool { return false }

func stopSignals() []os.Signal { return []os.Signal{os.Interrupt} }
