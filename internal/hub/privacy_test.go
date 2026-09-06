package hub

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
)

func TestHeldoutSettingsNeverLeaveAPI(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	server := httptest.NewServer(h)
	defer server.Close()
	paths := []string{"/private/old-heldout-canary", "/private/\u263a-\"quoted\"-heldout-canary"}
	var assertPrivate func(any)
	assertPrivate = func(value any) {
		t.Helper()
		switch value := value.(type) {
		case string:
			for _, path := range paths {
				if strings.Contains(value, path) {
					t.Fatalf("API exposed held-out path %q", path)
				}
			}
		case map[string]any:
			for key, item := range value {
				assertPrivate(key)
				assertPrivate(item)
			}
		case []any:
			for _, item := range value {
				assertPrivate(item)
			}
		}
	}
	set := func(path string) {
		t.Helper()
		status, payload := confirmedRequest(t, h, "settings", fmt.Sprintf(`{"name":%q,"value":%q}`, heldoutSetting, path))
		if status != 200 {
			t.Fatalf("private setting update: %d %#v", status, payload)
		}
		row := payload["setting"].(map[string]any)
		if row["secret"] != true || row["value"] != hiddenSettingValue(path) {
			t.Fatalf("POST echo is not masked: %#v", row)
		}
		assertPrivate(payload)
		if s.Setting(heldoutSetting, "") != path {
			t.Fatal("masking the response changed the persisted setting")
		}
	}
	var cursor int64
	if err := s.DB.QueryRow("SELECT COALESCE(MAX(seq),0) FROM events").Scan(&cursor); err != nil {
		t.Fatal(err)
	}
	set(paths[0])
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, "GET", fmt.Sprintf("%s/api/v1/events/stream?since=%d", server.URL, cursor), nil)
	if err != nil {
		t.Fatal(err)
	}
	response, err := server.Client().Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != 200 {
		t.Fatal(response.Status)
	}
	set(paths[1])
	reader := bufio.NewReader(response.Body)
	readEvent := func() additionEvent {
		t.Helper()
		for {
			line, err := reader.ReadString('\n')
			if err != nil {
				t.Fatalf("reading event stream: %v", err)
			}
			assertPrivate(line)
			if !strings.HasPrefix(line, "data: ") {
				continue
			}
			var event additionEvent
			if err := json.Unmarshal([]byte(strings.TrimPrefix(line, "data: ")), &event); err != nil {
				t.Fatal(err)
			}
			assertPrivate(event.Subject)
			assertPrivate(event.Detail)
			return event
		}
	}
	for settingEvents := 0; settingEvents < 2; {
		event := readEvent()
		if event.Subject == heldoutSetting {
			settingEvents++
			if event.Detail != privateValue {
				t.Fatalf("SSE event was not masked: %#v", event)
			}
		}
	}
	assertQuotedEvent := func(path string) {
		t.Helper()
		if err := s.Event("agent", "post", "location:"+path, map[string]any{"note": "Location: " + path}); err != nil {
			t.Fatal(err)
		}
		for {
			event := readEvent()
			if event.Action == "post" {
				if event.Subject != "location:"+privateValue {
					t.Fatalf("SSE exposed a current or historical private subject: %#v", event)
				}
				if detail, ok := event.Detail.(map[string]any); !ok || detail["note"] != "Location: "+privateValue {
					t.Fatalf("SSE exposed a JSON-encoded private path: %#v", event.Detail)
				}
				break
			}
		}
	}
	assertQuotedEvent(paths[0])
	assertQuotedEvent(paths[1])
	var detail string
	if err := s.DB.QueryRow("SELECT detail FROM events WHERE subject=? ORDER BY seq DESC LIMIT 1", heldoutSetting).Scan(&detail); err != nil {
		t.Fatal(err)
	}
	if detail != paths[1] {
		t.Fatal("API redaction rewrote the immutable event instead of its projection")
	}
	set("")
	for {
		event := readEvent()
		if event.Subject == heldoutSetting {
			if event.Detail != "" {
				t.Fatalf("SSE reported a cleared setting as configured: %#v", event)
			}
			break
		}
	}
	assertQuotedEvent(paths[1])
	for _, path := range []string{"/api/v1/settings", "/api/v1/events", "/api/v1/timeline"} {
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest("GET", path, nil))
		if rec.Code != 200 {
			t.Fatalf("%s returned %d: %s", path, rec.Code, rec.Body.String())
		}
		var payload any
		if err := json.Unmarshal(rec.Body.Bytes(), &payload); err != nil {
			t.Fatal(err)
		}
		assertPrivate(payload)
	}
}

func TestPrivateSettingEventShapes(t *testing.T) {
	for _, tc := range []struct {
		name, subject, detail string
	}{
		{"legacy scalar", heldoutSetting, "/private/canary"},
		{"quoted scalar", heldoutSetting, `"/private/canary"`},
		{"named payload", "", `{"name":"evolve.heldout_dir","value":"/private/canary","old_value":"/private/older"}`},
		{"keyed payload", "", `{"key":"evolve.heldout_dir","value":"/private/canary"}`},
		{"nested payload", heldoutSetting, `{"previous":{"path":"/private/canary"}}`},
	} {
		t.Run(tc.name, func(t *testing.T) {
			event := additionBusEvent(bus.Event{ID: 1, Action: "setting", Subject: tc.subject, Detail: tc.detail})
			if event.Detail != privateValue {
				t.Fatalf("private event detail is not masked: %#v", event)
			}
		})
	}
	public := additionBusEvent(bus.Event{ID: 2, Action: "setting", Subject: "workers", Detail: "3"})
	if public.Detail != float64(3) {
		t.Fatalf("public setting event changed: %#v", public)
	}
	cleared := additionBusEvent(bus.Event{ID: 3, Action: "setting", Subject: heldoutSetting, Detail: ""})
	if cleared.Detail != "" {
		t.Fatalf("cleared directory was reported as configured: %#v", cleared)
	}
}
