package copilot

import (
	"errors"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/provider"
)

func TestExportLoginOnlyReturnsStoredPulseLogin(t *testing.T) {
	p := newTestProvider(t, t.TempDir(), map[string]string{"COPILOT_GITHUB_TOKEN": "gho_borrowed0000000000000000000000"}).(*copilot)
	if _, _, err := p.ExportLogin(); err == nil {
		t.Fatal("borrowed environment token was exportable")
	} else {
		var perr *provider.Error
		if !errors.As(err, &perr) || perr.Code != "no_stored_login" {
			t.Fatalf("missing stored login error = %#v", err)
		}
	}

	if err := p.writeStored(storedLogin{
		GitHubToken: "gho_stored00000000000000000000000000",
		User:        "octocat",
		Source:      "device login",
		SavedAt:     time.Now().UTC().Format(time.RFC3339),
	}); err != nil {
		t.Fatal(err)
	}
	tok, login, err := p.ExportLogin()
	if err != nil {
		t.Fatal(err)
	}
	if tok != "gho_stored00000000000000000000000000" {
		t.Fatalf("token = %q", tok)
	}
	if !login.LoggedIn || login.User != "octocat" || login.Source != "stored" {
		t.Fatalf("login = %+v", login)
	}
}
