// Package bus is the in-process event bus (D-E): every store event a Go
// process writes is published to subscribers over buffered channels, and
// other processes follow the same stream by tailing the events table.
//
// Writers call Emit instead of store.Event so the row and the publish
// happen together; Tail polls `PRAGMA data_version` so the hub or `pulse
// watch` see rows the daemon inserted from another connection.
package bus

import (
	"fmt"
	"path"
	"strings"
	"sync"
	"sync/atomic"
)

// Event is one row of the events table (seq is ID).
type Event struct {
	ID      int64  `json:"id"`
	At      string `json:"at"`
	Actor   string `json:"actor"`
	Action  string `json:"action"`
	Subject string `json:"subject"`
	Detail  string `json:"detail"`
}

// Project is the project named in the detail JSON ("" when the detail has
// none). Events carry no project column; writers put it in the detail.
func (e Event) Project() string {
	return detailField(e.Detail, "project")
}

// Thread is the "area/thread" the detail names, if any.
func (e Event) Thread() string {
	return detailField(e.Detail, "thread")
}

// Filter narrows a subscription; empty fields match everything.
type Filter struct {
	Actions []string // globs on the action, e.g. "post.*", "task.amended", "*"
	Project string   // detail.project (or the subject) equals this
	Subject string   // glob on the subject
	Area    string   // detail.thread starts with "area/"
	Thread  string   // detail.thread equals "area/thread" (or ends with "/thread")
}

// Match reports whether e passes f.
func (f Filter) Match(e Event) bool {
	if len(f.Actions) > 0 {
		ok := false
		for _, g := range f.Actions {
			if Glob(g, e.Action) {
				ok = true
				break
			}
		}
		if !ok {
			return false
		}
	}
	if f.Subject != "" && !Glob(f.Subject, e.Subject) {
		return false
	}
	if f.Project != "" && e.Project() != f.Project && e.Subject != f.Project {
		return false
	}
	if f.Area != "" || f.Thread != "" {
		thread := e.Thread()
		if thread == "" {
			return false
		}
		if f.Area != "" && !strings.HasPrefix(thread, f.Area+"/") && thread != f.Area {
			return false
		}
		if f.Thread != "" && thread != f.Thread && !strings.HasSuffix(thread, "/"+f.Thread) {
			return false
		}
	}
	return true
}

// Glob matches a dotted action (or a subject) against a pattern: "*" matches
// everything, otherwise path.Match semantics ("post.*", "task.*", "compute.approved").
func Glob(pattern, name string) bool {
	if pattern == "" || pattern == "*" {
		return true
	}
	ok, err := path.Match(pattern, name)
	return err == nil && ok
}

type subscriber struct {
	mu     sync.Mutex
	ch     chan Event
	filter Filter
	closed bool
}

// Bus fans events out to subscribers. Publish never blocks: a subscriber
// whose buffer is full loses its oldest event (counted in Dropped).
type Bus struct {
	mu      sync.Mutex
	subs    map[int]*subscriber
	next    int
	dropped atomic.Int64

	// seen remembers the last seenSize event ids PublishOnce delivered so a
	// Bridge and an Emit racing on the same row publish it exactly once.
	seen     map[string]struct{}
	seenRing []string
	seenPos  int
	deduped  atomic.Int64
}

// seenSize bounds the dedup memory; ids are monotonic so a small window is enough.
const seenSize = 4096

// New returns an empty bus.
func New() *Bus {
	return &Bus{subs: map[int]*subscriber{}, seen: map[string]struct{}{}, seenRing: make([]string, seenSize)}
}

var (
	defaultMu  sync.Mutex
	defaultBus *Bus
)

// Default is the process-wide bus Emit publishes to.
func Default() *Bus {
	defaultMu.Lock()
	defer defaultMu.Unlock()
	if defaultBus == nil {
		defaultBus = New()
	}
	return defaultBus
}

// ResetDefault replaces the process-wide bus with a fresh one and returns it.
// Tests call it when they build a new store: a process serves one store, but
// a test binary opens many whose deterministic fixtures repeat the same event
// rows, which PublishOnce on a shared bus would rightly treat as duplicates.
func ResetDefault() *Bus {
	defaultMu.Lock()
	defer defaultMu.Unlock()
	defaultBus = New()
	return defaultBus
}

// Subscribe returns a channel that receives matching events and a cancel
// function that closes it. buffer <= 0 means 64. Slow subscribers drop their
// oldest buffered event rather than slowing a publisher.
func (b *Bus) Subscribe(filter Filter, buffer int) (<-chan Event, func()) {
	if buffer <= 0 {
		buffer = 64
	}
	s := &subscriber{ch: make(chan Event, buffer), filter: filter}
	b.mu.Lock()
	id := b.next
	b.next++
	b.subs[id] = s
	b.mu.Unlock()
	var once sync.Once
	cancel := func() {
		once.Do(func() {
			b.mu.Lock()
			delete(b.subs, id)
			b.mu.Unlock()
			s.mu.Lock()
			s.closed = true
			close(s.ch)
			s.mu.Unlock()
		})
	}
	return s.ch, cancel
}

// Publish delivers e to every subscriber whose filter matches.
func (b *Bus) Publish(e Event) {
	b.mu.Lock()
	subs := make([]*subscriber, 0, len(b.subs))
	for _, s := range b.subs {
		subs = append(subs, s)
	}
	b.mu.Unlock()
	for _, s := range subs {
		if !s.filter.Match(e) {
			continue
		}
		s.mu.Lock()
		if !s.closed {
			select {
			case s.ch <- e:
			default:
				// full: make room by dropping the oldest, then retry once
				select {
				case <-s.ch:
					b.dropped.Add(1)
				default:
				}
				select {
				case s.ch <- e:
				default:
					b.dropped.Add(1)
				}
			}
		}
		s.mu.Unlock()
	}
}

// Dropped counts events lost to full subscriber buffers since the bus was made.
func (b *Bus) Dropped() int64 {
	return b.dropped.Load()
}

// PublishOnce publishes e unless an event with the same non-zero ID was
// already published this way; it reports whether e was delivered. Emit,
// PublishAfter and Bridge all use it, so a row written by any path reaches
// in-process subscribers exactly once. Events with ID 0 are always published.
func (b *Bus) PublishOnce(e Event) bool {
	if e.ID != 0 {
		// the key carries the row's content too, so ids that restart (a
		// re-created store, several stores in one test binary) never mask
		// a different event
		key := fmt.Sprintf("%d|%s|%s|%s", e.ID, e.At, e.Action, e.Subject)
		b.mu.Lock()
		if _, dup := b.seen[key]; dup {
			b.mu.Unlock()
			b.deduped.Add(1)
			return false
		}
		if old := b.seenRing[b.seenPos]; old != "" {
			delete(b.seen, old)
		}
		b.seenRing[b.seenPos] = key
		b.seenPos = (b.seenPos + 1) % seenSize
		b.seen[key] = struct{}{}
		b.mu.Unlock()
	}
	b.Publish(e)
	return true
}

// Deduped counts events PublishOnce declined because they were already published.
func (b *Bus) Deduped() int64 {
	return b.deduped.Load()
}

// Subscribers is the number of live subscriptions.
func (b *Bus) Subscribers() int {
	b.mu.Lock()
	defer b.mu.Unlock()
	return len(b.subs)
}

// detailField pulls one string field out of a JSON object detail without
// failing on the free-text details the Python tool wrote.
func detailField(detail, key string) string {
	if !strings.HasPrefix(strings.TrimSpace(detail), "{") {
		return ""
	}
	var m map[string]any
	if err := unmarshal(detail, &m); err != nil {
		return ""
	}
	if v, ok := m[key].(string); ok {
		return v
	}
	return ""
}
