package store

import "testing"

func TestPostForOwner(t *testing.T) {
	for _, tc := range []struct {
		name, extra string
		want        bool
		wantError   bool
	}{
		{"missing", `{}`, false, false},
		{"unrelated metadata", `{"mentions":["human"],"ticket":"owner-review"}`, false, false},
		{"true", `{"for_owner":true}`, true, false},
		{"false", `{"for_owner":false}`, false, false},
		{"frontmatter true", `{"for_owner":"true"}`, true, false},
		{"frontmatter false", `{"for_owner":"false"}`, false, false},
		{"normalized true", `{"for_owner":" TRUE "}`, true, false},
		{"normalized false", `{"for_owner":" FALSE "}`, false, false},
		{"number", `{"for_owner":1}`, false, true},
		{"zero", `{"for_owner":0}`, false, true},
		{"null flag", `{"for_owner":null}`, false, true},
		{"object", `{"for_owner":{}}`, false, true},
		{"array flag", `{"for_owner":[]}`, false, true},
		{"invalid text", `{"for_owner":"yes"}`, false, true},
		{"empty text", `{"for_owner":""}`, false, true},
		{"invalid json", `{"for_owner":`, false, true},
		{"array metadata", `[]`, false, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got, err := PostForOwner(tc.extra)
			if (err != nil) != tc.wantError || got != tc.want {
				t.Fatalf("PostForOwner(%s) = %v, %v; want %v, error=%v", tc.extra, got, err, tc.want, tc.wantError)
			}
		})
	}
}
