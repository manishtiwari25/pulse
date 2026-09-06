package bus

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

func tempStore(t *testing.T) *store.Store {
	t.Helper()
	home := t.TempDir()
	if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(store.Options{Home: home, Now: "2026-09-06T21:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func TestFanOutOnePublisherManySubscribers(t *testing.T) {
	b := New()
	const subs, events = 5, 200
	var wg sync.WaitGroup
	counts := make([]int, subs)
	cancels := make([]func(), subs)
	for i := 0; i < subs; i++ {
		ch, cancel := b.Subscribe(Filter{}, events)
		cancels[i] = cancel
		wg.Add(1)
		go func(i int, ch <-chan Event) {
			defer wg.Done()
			for range ch {
				counts[i]++
			}
		}(i, ch)
	}
	for i := 0; i < events; i++ {
		b.Publish(Event{ID: int64(i), Action: "post"})
	}
	for _, c := range cancels {
		c()
	}
	wg.Wait()
	for i, n := range counts {
		if n != events {
			t.Errorf("subscriber %d got %d events, want %d", i, n, events)
		}
	}
	if b.Dropped() != 0 {
		t.Errorf("dropped %d", b.Dropped())
	}
	if b.Subscribers() != 0 {
		t.Errorf("subscribers left: %d", b.Subscribers())
	}
}

func TestSlowSubscriberDropsOldestNeverBlocks(t *testing.T) {
	b := New()
	ch, cancel := b.Subscribe(Filter{}, 3)
	defer cancel()
	done := make(chan struct{})
	go func() {
		for i := 1; i <= 10; i++ {
			b.Publish(Event{ID: int64(i)})
		}
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("publisher blocked on a slow subscriber")
	}
	var got []int64
	for len(got) < 3 {
		got = append(got, (<-ch).ID)
	}
	if got[0] != 8 || got[1] != 9 || got[2] != 10 {
		t.Fatalf("kept %v, want the newest three [8 9 10]", got)
	}
	if b.Dropped() != 7 {
		t.Errorf("dropped %d, want 7", b.Dropped())
	}
}

func TestCancelStopsDeliveryAndIsIdempotent(t *testing.T) {
	b := New()
	ch, cancel := b.Subscribe(Filter{}, 4)
	b.Publish(Event{ID: 1})
	cancel()
	cancel()
	b.Publish(Event{ID: 2})
	var ids []int64
	for e := range ch {
		ids = append(ids, e.ID)
	}
	if len(ids) != 1 || ids[0] != 1 {
		t.Fatalf("got %v after cancel, want [1]", ids)
	}
}

func TestFilterMatch(t *testing.T) {
	post := Event{Action: "post", Subject: "20260906T210000Z-a", Detail: `{"kind": "finding", "project": "demo", "thread": "runner/compute"}`}
	cases := []struct {
		f    Filter
		want bool
	}{
		{Filter{}, true},
		{Filter{Actions: []string{"post"}}, true},
		{Filter{Actions: []string{"task.*"}}, false},
		{Filter{Actions: []string{"task.*", "post"}}, true},
		{Filter{Project: "demo"}, true},
		{Filter{Project: "other"}, false},
		{Filter{Area: "runner"}, true},
		{Filter{Area: "board"}, false},
		{Filter{Thread: "runner/compute"}, true},
		{Filter{Thread: "compute"}, true},
		{Filter{Thread: "runner/other"}, false},
		{Filter{Subject: "2026*"}, true},
		{Filter{Subject: "TASK-*"}, false},
	}
	for _, c := range cases {
		if got := c.f.Match(post); got != c.want {
			t.Errorf("%+v.Match = %v, want %v", c.f, got, c.want)
		}
	}
	if (Filter{Project: "demo"}).Match(Event{Action: "project.removed", Subject: "demo", Detail: "free text"}) != true {
		t.Error("subject equal to the project should match")
	}
	if !Glob("post.*", "post.amended") || Glob("post.*", "task.amended") || !Glob("*", "anything") {
		t.Error("glob")
	}
}

func TestEmitWritesRowAndPublishes(t *testing.T) {
	s := tempStore(t)
	b := New()
	ch, cancel := b.Subscribe(Filter{Actions: []string{"task.*"}}, 8)
	defer cancel()
	e, err := EmitOn(b, s, "agent-a", "task.created", "TASK-001", map[string]any{"project": "demo", "title": "x"})
	if err != nil {
		t.Fatal(err)
	}
	if e.ID == 0 || e.At != "2026-09-06T21:00:00Z" || e.Detail != `{"project": "demo", "title": "x"}` {
		t.Fatalf("event %+v", e)
	}
	got := <-ch
	if got != e {
		t.Fatalf("published %+v, emitted %+v", got, e)
	}
	rows, err := Since(s, 0, Filter{Actions: []string{"task.created"}}, 0)
	if err != nil || len(rows) != 1 || rows[0] != e {
		t.Fatalf("Since = %v, %v", rows, err)
	}
	if last, _ := LastID(s); last != e.ID {
		t.Fatalf("LastID %d, want %d", last, e.ID)
	}
	// the default bus path
	ch2, cancel2 := Default().Subscribe(Filter{Subject: "TASK-002"}, 1)
	defer cancel2()
	if _, err := Emit(s, "agent-a", "task.created", "TASK-002", nil); err != nil {
		t.Fatal(err)
	}
	if (<-ch2).Subject != "TASK-002" {
		t.Fatal("default bus did not deliver")
	}
}

func TestTailSeesRowsFromAnotherConnection(t *testing.T) {
	home := t.TempDir()
	if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
		t.Fatal(err)
	}
	reader, err := store.Open(store.Options{Home: home})
	if err != nil {
		t.Fatal(err)
	}
	defer reader.Close()
	writer, err := store.Open(store.Options{Home: home, Now: "2026-09-06T21:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	defer writer.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ch := Tail(ctx, reader, -1, Filter{Actions: []string{"post", "task.*"}}, 20*time.Millisecond)
	time.Sleep(50 * time.Millisecond) // let Tail take its baseline

	if err := writer.Event("x", "settings.set", "stop", ""); err != nil { // filtered out
		t.Fatal(err)
	}
	if _, err := EmitOn(nil, writer, "agent-b", "task.amended", "TASK-001", map[string]any{"project": "demo"}); err != nil {
		t.Fatal(err)
	}
	if err := writer.Event("agent-b", "post", "20260906T210000Z-agent-b", `{"kind": "finding", "project": "demo", "thread": "a/b"}`); err != nil {
		t.Fatal(err)
	}
	var got []Event
	deadline := time.After(3 * time.Second)
	for len(got) < 2 {
		select {
		case e := <-ch:
			got = append(got, e)
		case <-deadline:
			t.Fatalf("tail delivered %d events: %+v", len(got), got)
		}
	}
	if got[0].Action != "task.amended" || got[1].Action != "post" || got[1].ID != got[0].ID+1 {
		t.Fatalf("tail order: %+v", got)
	}
	cancel()
	select {
	case _, ok := <-ch:
		if ok {
			// drain anything already buffered, then expect close
			for range ch {
			}
		}
	case <-time.After(2 * time.Second):
		t.Fatal("tail did not close after cancel")
	}
}

func TestTailFromSince(t *testing.T) {
	s := tempStore(t)
	for i := 0; i < 3; i++ {
		if err := s.Event("a", "post", "p", ""); err != nil {
			t.Fatal(err)
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	ch := Tail(ctx, s, 1, Filter{}, 10*time.Millisecond)
	var ids []int64
	for len(ids) < 2 {
		select {
		case e := <-ch:
			ids = append(ids, e.ID)
		case <-ctx.Done():
			t.Fatalf("got %v", ids)
		}
	}
	// the store.Create seeding may have logged events first; we only assert order and count after seq 1
	if ids[0] >= ids[1] {
		t.Fatalf("order %v", ids)
	}
}

func TestMain(m *testing.M) {
	// never touch the live store
	os.Setenv("PULSE_HOME", filepath.Join(os.TempDir(), "pulse-bus-test-never-used"))
	os.Exit(m.Run())
}

func TestPublishOnceDedupsById(t *testing.T) {
	b := New()
	ch, cancel := b.Subscribe(Filter{}, 8)
	defer cancel()
	if !b.PublishOnce(Event{ID: 7, Action: "a"}) || b.PublishOnce(Event{ID: 7, Action: "a"}) {
		t.Fatal("second publish of id 7 should be declined")
	}
	if !b.PublishOnce(Event{Action: "synthetic"}) || !b.PublishOnce(Event{Action: "synthetic"}) {
		t.Fatal("id 0 events are always published")
	}
	if b.Deduped() != 1 {
		t.Fatalf("deduped=%d", b.Deduped())
	}
	got := 0
	for done := false; !done; {
		select {
		case <-ch:
			got++
		case <-time.After(200 * time.Millisecond):
			done = true
		}
	}
	if got != 3 {
		t.Fatalf("got %d events, want 3", got)
	}
	// the window forgets ids once seenSize newer ones passed
	for i := int64(100); i < 100+seenSize; i++ {
		b.PublishOnce(Event{ID: i})
	}
	if !b.PublishOnce(Event{ID: 7}) {
		t.Fatal("id 7 should have aged out of the dedup window")
	}
	// the same seq from a different store is a different row, not a duplicate
	if !b.PublishOnce(Event{ID: 7, Action: "b"}) {
		t.Fatal("distinct rows sharing an id must both be published")
	}
}

// The D-E gap: store.Event and store.SetSetting write rows without touching
// the bus; a Bridge delivers them to in-process subscribers, and an Emit on
// the same bus is not doubled.
func TestBridgeDeliversStoreWritesOnce(t *testing.T) {
	s := tempStore(t)
	b := New()
	ctx, stop := context.WithCancel(context.Background())
	defer stop()
	done := BridgeOn(ctx, b, s, 20*time.Millisecond)
	time.Sleep(60 * time.Millisecond) // let the bridge take its "from now" mark
	ch, cancel := b.Subscribe(Filter{}, 16)
	defer cancel()

	if err := s.Event("demo-lead", "post.created", "20260906T210000Z-demo-lead", map[string]any{"project": "demo"}); err != nil {
		t.Fatal(err)
	}
	if err := s.SetSetting(store.HumanAgent, "compute.auto_approve", "1"); err != nil {
		t.Fatal(err)
	}
	if _, err := EmitOn(b, s, "runner", "job.leased", "job-1", nil); err != nil {
		t.Fatal(err)
	}

	var got []Event
	deadline := time.After(3 * time.Second)
	for len(got) < 3 {
		select {
		case e := <-ch:
			got = append(got, e)
		case <-deadline:
			t.Fatalf("got %d events before deadline: %+v", len(got), got)
		}
	}
	actions := map[string]int{}
	for _, e := range got {
		actions[e.Action]++
		if e.ID == 0 {
			t.Fatalf("bridged event without id: %+v", e)
		}
	}
	if actions["post.created"] != 1 || actions["setting"] != 1 || actions["job.leased"] != 1 {
		t.Fatalf("actions=%v", actions)
	}
	// nothing else arrives: the Emit'd row must not be re-published by the bridge
	time.Sleep(120 * time.Millisecond)
	select {
	case e := <-ch:
		t.Fatalf("duplicate event %+v", e)
	default:
	}
	if b.Deduped() != 1 {
		t.Fatalf("deduped=%d, want 1 (the Emit'd row seen again by the bridge)", b.Deduped())
	}
	stop()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("bridge did not stop")
	}
}

// TestStoreHelpersReachDefaultBus: SetSetting logs its event privately in the
// store; the OnEvent observer installed at init must deliver it to Default()
// exactly once, and a later Emit of the same id (Bridge, PublishAfter) must
// not deliver it again.
func TestStoreHelpersReachDefaultBus(t *testing.T) {
	b := ResetDefault() // earlier tests fed Default() rows from other temp stores
	s := tempStore(t)
	ch, cancel := b.Subscribe(Filter{Actions: []string{"setting"}}, 8)
	defer cancel()
	if err := s.SetSetting("human", "workers", "2"); err != nil {
		t.Fatal(err)
	}
	var got Event
	select {
	case got = <-ch:
	case <-time.After(2 * time.Second):
		t.Fatal("setting event never reached the default bus")
	}
	if got.Action != "setting" || got.Subject != "workers" || got.Detail != "2" || got.ID == 0 {
		t.Fatalf("event %+v", got)
	}
	b.PublishOnce(got)
	select {
	case dup := <-ch:
		t.Fatalf("duplicate delivery %+v", dup)
	case <-time.After(200 * time.Millisecond):
	}
}

func TestRedactMasksPrivateSettings(t *testing.T) {
	s := tempStore(t)
	if err := s.SetSetting(store.HumanAgent, "evolve.heldout_dir", "/Users/owner/private-fixtures"); err != nil {
		t.Fatal(err)
	}
	if err := s.SetSetting(store.HumanAgent, "workers", "2"); err != nil {
		t.Fatal(err)
	}
	redact := Redactor(s)
	e := redact(Event{ID: 1, Action: "setting", Subject: "evolve.heldout_dir", Detail: "/Users/owner/private-fixtures"})
	if e.Detail != PrivateValue {
		t.Fatalf("private setting leaked: %+v", e)
	}
	e = redact(Event{ID: 2, Action: "setting", Subject: "workers", Detail: "2"})
	if e.Detail != "2" {
		t.Fatalf("ordinary setting altered: %+v", e)
	}
	e = redact(Event{ID: 3, Action: "evolve.cycle", Subject: "cycle-1", Detail: `{"fixtures": "/Users/owner/private-fixtures/run.json"}`})
	if strings.Contains(e.Detail, "owner/private") || !strings.Contains(e.Detail, PrivateValue) {
		t.Fatalf("quoted path leaked: %+v", e)
	}
	// without a store only the setting event is masked, and unset settings are not "" scrubbed
	e = Redactor(nil)(Event{ID: 4, Action: "setting", Subject: "evolve.heldout_dir", Detail: "x"})
	if e.Detail != PrivateValue {
		t.Fatalf("%+v", e)
	}
	if e := Redact(Event{ID: 5, Action: "setting", Subject: "evolve.heldout_dir", Detail: ""}); e.Detail != "" {
		t.Fatalf("clearing a private setting is not a value: %+v", e)
	}
}

// H's canary: a private path with Unicode and quotes, quoted inside a public
// post detail that store.Event serialised with PyJSON (\u263a, \"), and a path
// that has since been cleared from the configuration. Both must be masked,
// and the masked detail must still parse as JSON.
func TestRedactorScrubsEncodedAndHistoricalValues(t *testing.T) {
	s := tempStore(t)
	canary := `/private/☺-"quoted"-heldout-canary`
	old := `/private/<old>&former-heldout`
	for _, v := range []string{old, canary} {
		if err := s.SetSetting(store.HumanAgent, "evolve.heldout_dir", v); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Event("demo-lead", "post.created", "p1", map[string]any{"note": "Location: " + canary, "old": old}); err != nil {
		t.Fatal(err)
	}
	// clear the setting: the historical values must still be hidden
	if err := s.SetSetting(store.HumanAgent, "evolve.heldout_dir", ""); err != nil {
		t.Fatal(err)
	}
	events, err := Since(s, 0, Filter{Actions: []string{"post.created"}}, 0)
	if err != nil || len(events) != 1 {
		t.Fatalf("%v %d", err, len(events))
	}
	raw := events[0].Detail
	if !strings.Contains(raw, `\u263a`) || !strings.Contains(raw, `\"quoted\"`) {
		t.Fatalf("fixture did not exercise PyJSON escaping: %s", raw)
	}
	got := Redactor(s)(events[0])
	var doc map[string]string
	if err := json.Unmarshal([]byte(got.Detail), &doc); err != nil {
		t.Fatalf("masked detail is not JSON: %v\n%s", err, got.Detail)
	}
	if doc["note"] != "Location: "+PrivateValue || doc["old"] != PrivateValue {
		t.Fatalf("leak after decoding: %+v", doc)
	}
	// Go-encoded (HTML-escaped) and double-encoded forms are covered too
	goEnc := goEncoded(canary, true)
	e := Redactor(s)(Event{ID: 9, Action: "x", Detail: `{"a":"` + goEnc + `","b":"` + goEncoded(goEnc, false) + `"}`})
	if strings.Contains(e.Detail, "heldout-canary") {
		t.Fatalf("encoded leak: %s", e.Detail)
	}
	if err := json.Unmarshal([]byte(e.Detail), &doc); err != nil {
		t.Fatal(err)
	}
}
