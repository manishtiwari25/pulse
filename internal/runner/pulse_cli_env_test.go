package runner

import "testing"

// TestPulseOnPath: a recipe-only PULSE_CLI must reach the remote hands env (PULSE_* passes handsEnvRe),
// so the worker on the host honours the recipe's binary instead of only its own environment.
func TestPulseOnPath(t *testing.T) {
	if got := recipePulseCLI(nil); got != "" {
		t.Fatalf("nil recipe: got %q, want empty", got)
	}
	r, problems := parseRecipe(recipeWith(nil, nil, nil, [][2]string{{"PULSE_CLI", "/opt/pulse/bin/pulse "}}, ""))
	if len(problems) != 0 {
		t.Fatalf("unexpected problems: %v", problems)
	}
	if got := recipePulseCLI(r); got != "/opt/pulse/bin/pulse" {
		t.Fatalf("recipe PULSE_CLI: got %q", got)
	}
	if !handsEnvRe.MatchString("PULSE_CLI=" + recipePulseCLI(r)) {
		t.Fatal("hands env filter rejects PULSE_CLI; the recipe value would never reach the host")
	}
	plain, _ := parseRecipe(recipeWith(nil, nil, nil, [][2]string{{"OTHER", "x"}}, ""))
	if got := recipePulseCLI(plain); got != "" {
		t.Fatalf("recipe without PULSE_CLI: got %q", got)
	}
}
