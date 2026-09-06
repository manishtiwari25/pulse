//go:build !darwin && !linux

package records

import (
	"fmt"
	"os"
)

func lockFile(_ *os.File) error {
	return fmt.Errorf("record writes require an OS advisory lock; this build supports macOS and Linux")
}
