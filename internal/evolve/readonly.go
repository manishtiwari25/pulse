package evolve

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path"
	"strings"
)

type treeEntry struct {
	name string
	mode fs.FileMode
}

// freezeTree removes write permission before scoring, including Git metadata.
// A content/mode fingerprint rejects scorers which restore their own write
// permissions. This is a read-only contract check, not an OS sandbox.
func freezeTree(root *os.Root) (func() error, error) {
	entries := []treeEntry{}
	err := fs.WalkDir(root.FS(), ".", func(name string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		info, err := root.Lstat(name)
		if err != nil {
			return err
		}
		if info.Mode()&os.ModeSymlink != 0 {
			target, err := root.Readlink(name)
			if err != nil || path.IsAbs(target) || strings.Contains(target, "\\") {
				return &ViolationError{Reason: "scored tree has an escaping symlink"}
			}
			joined := path.Join(path.Dir(name), target)
			if joined == ".." || strings.HasPrefix(joined, "../") {
				return &ViolationError{Reason: "scored tree has an escaping symlink"}
			}
			if _, err := root.Stat(name); err != nil && !errors.Is(err, os.ErrNotExist) {
				return &ViolationError{Reason: "scored tree has an unresolvable symlink"}
			}
			return nil
		}
		if !info.IsDir() && (!info.Mode().IsRegular() || multipleLinks(info)) {
			return &ViolationError{Reason: "scored tree has a special file or shared hardlink"}
		}
		entries = append(entries, treeEntry{name, info.Mode()})
		return nil
	})
	if err != nil {
		return nil, err
	}
	restore := func() error {
		var errs []error
		for _, entry := range entries {
			info, err := root.Lstat(entry.name)
			if errors.Is(err, os.ErrNotExist) || (err == nil && info.Mode()&os.ModeSymlink != 0) {
				continue
			}
			if err != nil {
				errs = append(errs, err)
				continue
			}
			if !multipleLinks(info) {
				errs = append(errs, root.Chmod(entry.name, entry.mode.Perm()))
			}
		}
		return errors.Join(errs...)
	}
	for i := len(entries) - 1; i >= 0; i-- {
		e := entries[i]
		if err := root.Chmod(e.name, e.mode.Perm()&^0o222); err != nil {
			return nil, errors.Join(err, restore())
		}
	}
	return restore, nil
}

func treeFingerprint(root *os.Root) (string, error) {
	hash := sha256.New()
	err := fs.WalkDir(root.FS(), ".", func(name string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		info, err := root.Lstat(name)
		if err != nil {
			return err
		}
		fmt.Fprintf(hash, "%s\x00%d\x00", name, info.Mode())
		switch {
		case info.Mode().IsRegular():
			if multipleLinks(info) {
				return &ViolationError{Reason: "scorer created a shared hardlink"}
			}
			file, err := root.Open(name)
			if err != nil {
				return err
			}
			digest := sha256.New()
			_, err = io.Copy(digest, file)
			if err := errors.Join(err, file.Close()); err != nil {
				return err
			}
			_, _ = hash.Write(digest.Sum(nil))
		case info.Mode()&os.ModeSymlink != 0:
			target, err := root.Readlink(name)
			if err != nil {
				return err
			}
			fmt.Fprint(hash, target)
		case !info.IsDir():
			return &ViolationError{Reason: "scored tree contains a special file"}
		}
		return nil
	})
	return hex.EncodeToString(hash.Sum(nil)), err
}
