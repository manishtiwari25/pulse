package tally

import (
	"container/list"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"reflect"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// tallyCache memoizes Compute results so the daemon does not re-parse every
// post in a project on every loop. Posts are immutable, so the cache key
// includes a cheap version derived from post/job counts and statuses.
// The cache is bounded so a long-running daemon does not retain unbounded
// results as thresholds and projects drift.
const cacheTTL = 10 * time.Second
const cacheMaxEntries = 32

type cacheKey struct {
	storePtr uintptr
	project  string
	area     string
	thHash   string
}

type cacheValue struct {
	result  *Result
	version string
	at      time.Time
}

type lruCache struct {
	mu       sync.Mutex
	entries  map[cacheKey]*list.Element
	order    list.List // front = most recently used
	max      int
}

type lruEntry struct {
	key   cacheKey
	value *cacheValue
}

var tallyCache = &lruCache{entries: map[cacheKey]*list.Element{}, max: cacheMaxEntries}

func (c *lruCache) get(key cacheKey) (*cacheValue, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if el, ok := c.entries[key]; ok {
		cv := el.Value.(*lruEntry).value
		if time.Since(cv.at) < cacheTTL {
			c.order.MoveToFront(el)
			return cv, true
		}
		c.removeElement(el)
	}
	return nil, false
}

func (c *lruCache) put(key cacheKey, value *cacheValue) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if el, ok := c.entries[key]; ok {
		c.order.MoveToFront(el)
		el.Value.(*lruEntry).value = value
		return
	}
	for len(c.entries) >= c.max && c.order.Len() > 0 {
		c.removeElement(c.order.Back())
	}
	el := c.order.PushFront(&lruEntry{key: key, value: value})
	c.entries[key] = el
}

func (c *lruCache) removeElement(el *list.Element) {
	if el == nil {
		return
	}
	c.order.Remove(el)
	delete(c.entries, el.Value.(*lruEntry).key)
}

func cacheKeyFor(s *store.Store, project, area string, th Thresholds) cacheKey {
	// Truncate the clock to the minute so the cache survives per-second ticks
	// without multiplying entries; a tally only cares about the date boundary.
	if len(th.Now) >= 16 {
		th.Now = th.Now[:16] + ":00Z"
	}
	b, _ := json.Marshal(th)
	h := sha256.Sum256(b)
	return cacheKey{
		storePtr: reflect.ValueOf(s).Pointer(),
		project:  project,
		area:     area,
		thHash:   fmt.Sprintf("%x", h[:8]),
	}
}

func cacheVersion(s *store.Store, project, area string) (string, error) {
	postQ := "SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM posts WHERE project = ?"
	args := []any{project}
	if area != "" {
		postQ += " AND area = ?"
		args = append(args, area)
	}
	var postsCount, postsMax int64
	if err := s.DB.QueryRow(postQ, args...).Scan(&postsCount, &postsMax); err != nil {
		return "", err
	}

	var jobsCount, jobsMax, jobsStatus int64
	if err := s.DB.QueryRow(`
		SELECT COUNT(*), COALESCE(MAX(rowid), 0), COALESCE(SUM(CASE status
			WHEN 'done' THEN 1 WHEN 'failed' THEN 2 WHEN 'cancelled' THEN 3
			WHEN 'queued' THEN 4 WHEN 'leased' THEN 5 WHEN 'blocked' THEN 6
			ELSE 0 END), 0)
		FROM jobs WHERE source LIKE 'suggestion:%'`).Scan(&jobsCount, &jobsMax, &jobsStatus); err != nil {
		return "", err
	}

	recordsCount, recordsUpdated := tableVersion(s, "records", "project", project, "updated_at")
	revsCount, revsAt := recordRevisionVersion(s, project)
	settingsHash := settingsHash(s)

	repo := ""
	if p, err := s.Project(project); err == nil && p != nil {
		repo = p.Path
	}

	return fmt.Sprintf("p:%d:%d:j:%d:%d:%d:rec:%d:%s:rev:%d:%s:sh:%s:r:%s",
		postsCount, postsMax, jobsCount, jobsMax, jobsStatus,
		recordsCount, recordsUpdated, revsCount, revsAt,
		settingsHash, repo), nil
}

func tableVersion(s *store.Store, table, whereCol, whereVal, maxCol string) (count int64, maxVal string) {
	var exists int
	_ = s.DB.QueryRow("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?", table).Scan(&exists)
	if exists == 0 {
		return 0, ""
	}
	q := "SELECT COUNT(*), COALESCE(MAX(" + maxCol + "), '') FROM " + table
	args := []any{}
	if whereCol != "" {
		q += " WHERE " + whereCol + " = ?"
		args = append(args, whereVal)
	}
	_ = s.DB.QueryRow(q, args...).Scan(&count, &maxVal)
	return count, maxVal
}

func settingsHash(s *store.Store) string {
	var exists int
	_ = s.DB.QueryRow("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'settings'").Scan(&exists)
	if exists == 0 {
		return ""
	}
	rows, err := s.DB.Query("SELECT name, value FROM settings ORDER BY name")
	if err != nil {
		return ""
	}
	defer rows.Close()
	h := sha256.New()
	for rows.Next() {
		var name, value string
		if err := rows.Scan(&name, &value); err != nil {
			return ""
		}
		h.Write([]byte(name))
		h.Write([]byte("="))
		h.Write([]byte(value))
		h.Write([]byte("\n"))
	}
	return fmt.Sprintf("%x", h.Sum(nil)[:8])
}

func recordRevisionVersion(s *store.Store, project string) (count int64, maxAt string) {
	var exists int
	_ = s.DB.QueryRow("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'record_revisions'").Scan(&exists)
	if exists == 0 {
		return 0, ""
	}
	q := `SELECT COUNT(*), COALESCE(MAX(at), '') FROM record_revisions
	      WHERE record_id IN (SELECT id FROM records WHERE project = ?)`
	_ = s.DB.QueryRow(q, project).Scan(&count, &maxAt)
	return count, maxAt
}

func cachedCompute(s *store.Store, project, area string, th Thresholds, compute func() (*Result, error)) (*Result, error) {
	key := cacheKeyFor(s, project, area, th)
	version, err := cacheVersion(s, project, area)
	if err != nil {
		return nil, err
	}

	if cv, ok := tallyCache.get(key); ok {
		if cv.version == version {
			return cv.result, nil
		}
	}

	res, err := compute()
	if err != nil {
		return nil, err
	}
	tallyCache.put(key, &cacheValue{result: res, version: version, at: time.Now()})
	return res, nil
}
