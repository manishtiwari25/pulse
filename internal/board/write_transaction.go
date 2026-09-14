package board

import "github.com/manishtiwari25/pulse/internal/store"

// WriteTransaction reuses the board's normal validation and event publication
// inside one immediate transaction. Store reads in the callback see a stable
// committed WAL snapshot while other writers wait; callbacks must not execute work.
type WriteTransaction struct{ w *writer }

func WithWriteTransaction(s *store.Store, fn func(*WriteTransaction) error) error {
	w, err := begin(s)
	if err != nil {
		return err
	}
	defer w.rollback()
	if err := fn(&WriteTransaction{w: w}); err != nil {
		return err
	}
	return w.commit()
}

func (tx *WriteTransaction) AddJob(in JobInput) (*Job, bool, error) {
	return addJob(tx.w, in)
}

func (tx *WriteTransaction) CreatePost(in PostInput) (*Post, error) {
	return createPost(tx.w, in)
}
