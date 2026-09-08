package inspect

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
)

func TestJobsRecoveryFieldAndText(t *testing.T) {
	s := temporaryStore(t)
	execSQL(t, s, `INSERT INTO jobs(id,project,kind,tier,title,brief,status,created_at,created_by,finished_at,result)
		VALUES ('job-held','demo','task',1,'Held job','','failed',?,'human',?,'failed'),
			('job-plain','demo','task',1,'Plain job','','failed',?,'human',?,'failed')`,
		parityNow, parityNow, parityNow, parityNow)
	if err := s.Event("daemon", board.JobRecoverAction, "job-held", map[string]any{"outcome": "replanned", "root": "job-held", "job": "job-copy"}); err != nil {
		t.Fatal(err)
	}
	if err := s.Event("daemon", board.JobRecoverAction, "job-held", map[string]any{"outcome": "held", "root": "job-held", "why": "owner must inspect"}); err != nil {
		t.Fatal(err)
	}

	jobs, err := Jobs(s, "", true)
	if err != nil {
		t.Fatal(err)
	}
	byID := map[string]Job{}
	for _, j := range jobs {
		byID[j.ID] = j
	}
	if byID["job-held"].Recovery != "held: owner must inspect" {
		t.Fatalf("held recovery %q", byID["job-held"].Recovery)
	}
	if byID["job-plain"].Recovery != "" {
		t.Fatalf("plain recovery %q", byID["job-plain"].Recovery)
	}

	text := invoke(t, s, false, "", "jobs", "--all")
	if text.code != 0 || !strings.Contains(text.out, "recovery: held: owner must inspect") {
		t.Fatalf("text recovery missing: %+v", text)
	}
	js := invoke(t, s, true, "", "jobs", "--all")
	if js.code != 0 {
		t.Fatalf("json jobs failed: %+v", js)
	}
	var decoded []Job
	if err := json.Unmarshal([]byte(js.out), &decoded); err != nil {
		t.Fatal(err)
	}
	byID = map[string]Job{}
	for _, j := range decoded {
		byID[j.ID] = j
	}
	if byID["job-held"].Recovery != "held: owner must inspect" || byID["job-plain"].Recovery != "" {
		t.Fatalf("json recovery held=%q plain=%q", byID["job-held"].Recovery, byID["job-plain"].Recovery)
	}
}

func TestRecoveryTextPhrases(t *testing.T) {
	cases := []struct {
		name, at, detail, want string
	}{
		{"requeued", "2026-09-06T19:00:00Z", `{"outcome":"requeued"}`, "ran again 2026-09-06T19:00:00Z"},
		{"replanned", "2026-09-06T19:00:00Z", `{"outcome":"replanned","job":"job-new"}`, "re-planned as job-new"},
		{"held", "2026-09-06T19:00:00Z", `{"outcome":"held","why":"needs owner"}`, "held: needs owner"},
		{"retried", "2026-09-06T19:00:00Z", `{"outcome":"retried"}`, "retried by hand 2026-09-06T19:00:00Z"},
		{"none", "2026-09-06T19:00:00Z", "", ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := recoveryText(tc.at, tc.detail); got != tc.want {
				t.Fatalf("recoveryText() = %q, want %q", got, tc.want)
			}
		})
	}
}
