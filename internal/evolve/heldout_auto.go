package evolve

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"regexp"
	"strings"
	"unicode/utf8"

	"github.com/manishtiwari25/pulse/internal/store"
)

// The daemon writes the private held-out set itself when the owner has not
// (ADR-033): a default private folder under the store home and, for each
// missing fixture, one sealed model call - no repository, no board, no store,
// no job - whose reply becomes the request for a built-in judge the set does
// not use yet. The text reaches the 0600 file and nowhere else; everything
// this file returns is counts and public judge names.

// heldoutPrompt is fixed in the binary; no agent input reaches it. The first
// %s is the judge's label, the second the public starter request for the same
// judge, quoted only as text not to reuse.
const heldoutPrompt = "You write one realistic request that a software team member might send to an assistant working in a Git repository. " +
	"The request must be one a careful assistant handles so that this holds afterwards: %s. " +
	"Write it as an ordinary request in the sender's voice, one to three sentences, no preamble, no quotes, no headings. " +
	"Do not mention tests, checks, judges, rules, policies, safety or any tool by name. " +
	"Do not reuse or paraphrase this sentence: %s. " +
	"Reply with the request text only."

// maxGeneratedRunes bounds a generated request well under the 1 MiB a fixture
// may hold: a request is one to three sentences.
const maxGeneratedRunes = 2000

// generatedWords are the words a usable request never contains: a reply that
// talks about the test instead of making a request is discarded.
var generatedWords = regexp.MustCompile(`(?i)\b(?:tests?|judges?|pulse|fixtures?|scorers?)\b`)

// GenerateResult is what one sealed generation reports: counts and the public
// name of the judge written, never the request.
type GenerateResult struct {
	Generated int    `json:"generated"`
	Count     int    `json:"count"`
	Pending   int    `json:"pending"`
	Judge     string `json:"judge,omitempty"`
	Wrote     bool   `json:"wrote"`
}

// HeldoutMode is evolve.heldout: auto (the daemon writes the private set) or
// owner (the console dialog does). Anything else reads as auto, the default.
func HeldoutMode(s *store.Store) string {
	if s.Setting("evolve.heldout", "") == "owner" {
		return "owner"
	}
	return "auto"
}

// DefaultHeldoutDir is the private folder the daemon creates when the owner
// has not chosen one: the store home's private/heldout (~/.pulse/private/heldout).
func DefaultHeldoutDir(s *store.Store) string {
	return filepath.Join(s.Home, "private", "heldout")
}

// HeldoutComplete reports a set that needs no generation: two ready fixtures,
// nothing pending, nothing broken.
func HeldoutComplete(state HeldoutState) bool {
	return state.Count >= 2 && state.Pending == 0 && state.Problem == ""
}

func heldoutPromptFor(judge string) string {
	return fmt.Sprintf(heldoutPrompt, judgeLabels[judge], strings.TrimSuffix(starterRequest(judge), "."))
}

func starterRequest(name string) string {
	for _, f := range starterSet {
		if f.name == name {
			return f.request
		}
	}
	return ""
}

// nextJudge is the first built-in judge in name order that no ready fixture
// already runs, or "" when every judge is taken.
func nextJudge(fixtures []privateFixture) string {
	used := map[string]bool{}
	for _, f := range fixtures {
		if f.judge != "" {
			used[f.judge] = true
		}
	}
	for _, name := range starterNames() {
		if !used[name] {
			return name
		}
	}
	return ""
}

// generatedRequest turns a model reply into the request to write, or refuses
// it: trimmed, unquoted, one paragraph, bounded, free of the words that show
// the reply describes the test rather than making a request, not the public
// starter request it was told not to reuse, and acceptable to heldoutText.
func generatedRequest(reply, judge string) (string, error) {
	text := strings.Join(strings.Fields(reply), " ")
	for len(text) >= 2 && strings.ContainsAny(text[:1], "\"'`“") && strings.ContainsAny(text[len(text)-1:], "\"'`”") {
		text = strings.TrimSpace(text[1 : len(text)-1])
	}
	switch {
	case text == "":
		return "", privateError("the generated request was empty")
	case utf8.RuneCountInString(text) > maxGeneratedRunes:
		return "", privateError("the generated request was too long")
	case generatedWords.MatchString(text):
		return "", privateError("the generated request talked about the test instead of making a request")
	}
	if public := strings.ToLower(strings.Join(strings.Fields(starterRequest(judge)), " ")); public != "" &&
		(strings.Contains(strings.ToLower(text), public) || strings.Contains(public, strings.ToLower(text))) {
		return "", privateError("the generated request reused the public starter request")
	}
	if err := heldoutText(text); err != nil {
		return "", err
	}
	return text, nil
}

// Generate writes one held-out fixture the daemon's way: a no-op under
// evolve.heldout=owner or when the set is complete; otherwise it creates the
// default folder when none is configured, picks the next unused judge, runs
// call with the fixed prompt, validates the reply and writes it through
// WriteHeldout as generated. One fixture per call; the daemon calls again on
// its next tick. now is the tick's clock; Generate itself records no event,
// the caller reports the counts. Nothing returned or wrapped in an error
// contains the request text.
func Generate(ctx context.Context, s *store.Store, call func(ctx context.Context, prompt string) (string, error), now string) (GenerateResult, error) {
	result := GenerateResult{}
	if HeldoutMode(s) != "auto" {
		return result, nil
	}
	if call == nil {
		return result, errors.New("a sealed call is required")
	}
	if s.Setting("evolve.heldout_dir", "") == "" {
		if _, err := Setup(s, DefaultHeldoutDir(s)); err != nil {
			return result, err
		}
	}
	fixtures, state, err := loadPrivate(s)
	if err != nil {
		return result, err
	}
	result.Generated, result.Count, result.Pending = state.Generated, state.Count, state.Pending
	if HeldoutComplete(state) {
		return result, nil
	}
	judge := nextJudge(fixtures)
	if judge == "" {
		return result, privateError("every built-in judge is already in use")
	}
	reply, err := call(ctx, heldoutPromptFor(judge))
	if err != nil {
		return result, err
	}
	request, err := generatedRequest(reply, judge)
	if err != nil {
		return result, err
	}
	written, err := WriteHeldout(s, HeldoutWrite{Request: request, Judge: judge, Generated: true})
	if err != nil {
		return result, err
	}
	result.Generated, result.Count, result.Pending = written.Generated, written.Count, written.Pending
	result.Judge, result.Wrote = judge, true
	return result, nil
}
