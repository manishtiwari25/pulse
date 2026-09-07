package evolve

import "testing"

// The template and the brain head the table "Editable scope by stage"; the
// constraint check must read it, or every experiment target is refused.
func TestEditablePathAcceptsQualifiedHeading(t *testing.T) {
	table := "| File class | Paths | Stage | Autonomy |\n| --- | --- | --- | --- |\n" +
		"| product | `src/` | 4 | standing |\n| memory | `docs/memory/` | 2 | standing |\n"
	for _, heading := range []string{"## Editable scope", "## Editable scope by stage", "## editable scope by stage"} {
		body := "# C\n\n" + heading + "\n\n" + table + "\n## Budget\n\n| Limit | Value |\n| --- | --- |\n| `x/` | 4 |\n"
		if !editablePath(body, "src/main.go", true) {
			t.Fatalf("%q: product path not editable", heading)
		}
		if !editablePath(body, "docs/memory/patterns.md", false) {
			t.Fatalf("%q: operating path not editable", heading)
		}
		if editablePath(body, "x/y", false) {
			t.Fatalf("%q: a later section's table leaked into the scope", heading)
		}
	}
	for _, heading := range []string{"## Editable scopes", "## Not the editable scope", "### Editable scope"} {
		if editablePath("# C\n\n"+heading+"\n\n"+table, "src/main.go", true) {
			t.Fatalf("%q: unrelated heading accepted", heading)
		}
	}
}
