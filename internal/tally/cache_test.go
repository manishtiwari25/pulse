package tally

import (
	"testing"
	"time"
)

func TestComputeCachesResultForUnchangedStore(t *testing.T) {
	f := newFixture(t, true)
	th := Thresholds{Now: fixtureNow}

	r1, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}
	r2, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}
	if r1 != r2 {
		t.Fatal("expected cached result pointer to be identical")
	}
}

func TestComputeInvalidatesCacheWhenPostAdded(t *testing.T) {
	f := newFixture(t, true)
	th := Thresholds{Now: fixtureNow}

	r1, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}

	f.post(t, fpost{kind: "claim", area: "governance", thread: "new", agent: "r/c", date: fixtureNow, summary: "new claim", session: "s", expires: "2026-12-31T00:00:00Z"})

	r2, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}
	if r1 == r2 {
		t.Fatal("expected cache to invalidate after new post")
	}
}

func TestComputeInvalidatesCacheWhenSettingsChange(t *testing.T) {
	f := newFixture(t, true)
	th := Thresholds{Now: fixtureNow}

	r1, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}

	if _, err := f.st.DB.Exec("UPDATE settings SET value = '1' WHERE name = 'stop'"); err != nil {
		t.Fatal(err)
	}

	r2, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}
	if r1 == r2 {
		t.Fatal("expected cache to invalidate after settings change")
	}
}

func TestComputeCacheExpiresAfterTTL(t *testing.T) {
	f := newFixture(t, true)
	th := Thresholds{Now: fixtureNow}

	r1, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}

	// Force the cached entry past its TTL without changing the store.
	key := cacheKeyFor(f.st, "fixture", "", th)
	if cv, ok := tallyCache.get(key); ok {
		tallyCache.put(key, &cacheValue{result: cv.result, version: cv.version, at: time.Now().Add(-2 * cacheTTL)})
	}

	r2, err := Compute(f.st, "fixture", "", th)
	if err != nil {
		t.Fatal(err)
	}
	if r1 == r2 {
		t.Fatal("expected cache to expire after TTL")
	}
}
