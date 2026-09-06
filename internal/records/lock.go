package records

import (
	"errors"
	"os"
	"path/filepath"
	"sync"

	"github.com/manishtiwari25/pulse/internal/store"
)

var writeMu sync.Mutex

// PutRecord is an upsert, so a fresh ID must remain reserved between the
// numbering/WIP checks and the write, including across independent CLI processes.
// The OS releases the advisory lock even if a writer crashes.
func lockWrites(s *store.Store) (func(), error) {
	writeMu.Lock()
	file, err := os.OpenFile(filepath.Join(s.Home, "records.lock"), os.O_CREATE|os.O_RDWR, 0o600)
	if err == nil {
		err = lockFile(file)
	}
	if err != nil {
		if file != nil {
			err = errors.Join(err, file.Close())
		}
		writeMu.Unlock()
		return nil, err
	}
	return func() {
		file.Close()
		writeMu.Unlock()
	}, nil
}
