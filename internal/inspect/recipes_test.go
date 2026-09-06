package inspect

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func TestRecipePlacementParity(t *testing.T) {
	s := temporaryStore(t)
	root := t.TempDir()
	fixtureCommand(t, root, "", "git", "init", "--quiet")
	execSQL(t, s, `INSERT INTO projects(slug,name,path,default_branch,added_at) VALUES ('recipe-project','Recipe',?,'main',?)`, root, parityNow)
	execSQL(t, s, `INSERT INTO executors(name,status,created_at) VALUES ('remote','active',?)`, parityNow)
	for _, tc := range []struct{ name, text, local, remote string }{
		{"empty", "---\nrecipe: 1\n---\n", "", ""},
		{"services", "---\nrecipe: 1\nservices:\n  - postgres\n---\n", "services", "services"},
		{"tools", "---\nrecipe: 1\ntools:\n  - node@22\n---\n", "", "hands outdated"},
		{"env", "---\nrecipe: 1\nenv:\n  CI: '1'\n---\n", "", "hands outdated"},
		{"bad-inline-list", "---\nrecipe: 1\nservices: [postgres]\n---\n", "", ""},
		{"duplicate-key", "---\nrecipe: 1\nservices:\n  - postgres\nservices: []\n---\n", "", ""},
		{"unknown-key", "---\nrecipe: 1\nother: 1\nservices:\n  - postgres\n---\n", "", ""},
		{"bad-version", "---\nrecipe: 2\nservices:\n  - postgres\n---\n", "", ""},
		{"secret-env-name", "---\nrecipe: 1\nenv:\n  TOKEN: refused\nservices:\n  - postgres\n---\n", "", ""},
		{"lowercase-env", "---\nrecipe: 1\nenv:\n  ci: '1'\nservices:\n  - postgres\n---\n", "", ""},
		{"bad-quote", "---\nrecipe: 1\nservices:\n  - 'postgres\n---\n", "", ""},
		{"tool-plugin-name", "---\nrecipe: 1\ntools:\n  - aqua:node@22\n---\n", "", "hands outdated"},
		{"too-many-tools", "---\nrecipe: 1\ntools:\n" + strings.Repeat("  - node@22\n", 13) + "services:\n  - postgres\n---\n", "", ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			blob := fixtureCommand(t, root, tc.text, "git", "hash-object", "-w", "--stdin")
			context := fixtureCommand(t, root, "100644 blob "+blob+"\tenvironment.md\n", "git", "mktree")
			docs := fixtureCommand(t, root, "040000 tree "+context+"\tcontext\n", "git", "mktree")
			tree := fixtureCommand(t, root, "040000 tree "+docs+"\tdocs\n", "git", "mktree")
			commit := fixtureCommand(t, root, tc.name+"\n", "git", "commit-tree", tree)
			fixtureCommand(t, root, "", "git", "update-ref", "refs/heads/main", commit)
			for _, check := range []struct{ executor, want string }{{"local", tc.local}, {"remote", tc.remote}} {
				blocks, err := RecipeBlocks(s, check.executor)
				if err != nil || blocks["recipe-project"] != check.want {
					t.Fatalf("%s blocks = %v (%v), want %q", check.executor, blocks, err, check.want)
				}
			}
			assertParity(t, invoke(t, s, false, "", "status"), oracle(t, s.Home, true, "status"))
		})
	}
}

func TestSQLRecipePlacement(t *testing.T) {
	s := temporaryStore(t)
	execSQL(t, s, `INSERT INTO projects(slug,name,path,added_at) VALUES ('sql-project','SQL','/does-not-exist',?)`, parityNow)
	record := store.Record{ID: "CTX-environment", Project: "sql-project", Kind: "context", Slug: "environment",
		Title: "Environment", CreatedAt: parityNow, UpdatedAt: parityNow, Origin: "store"}
	if _, err := s.PutRecord(record, map[string]any{"recipe": "1", "services": []string{"postgres"}},
		"An environment in the store, not a Markdown file.", "human", ""); err != nil {
		t.Fatal(err)
	}
	blocks, err := RecipeBlocks(s, "local")
	if err != nil || blocks["sql-project"] != "services" {
		t.Fatalf("SQL recipe not used: %v, %v", blocks, err)
	}
	line, err := ExecutorsLine(s)
	if err != nil || line != "local active (0/1 busy); needs services for sql-project" {
		t.Fatalf("SQL recipe not in status: %q, %v", line, err)
	}
}
