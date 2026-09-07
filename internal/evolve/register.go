package evolve

import "github.com/manishtiwari25/pulse/internal/cli"

func init() {
	cli.Register(cli.Command{
		Name: "evolve", Group: "agents", Raw: true, NoStore: true,
		Summary: "measure, rehearse and run the voted self-improvement loop",
		Usage: "pulse evolve status|fixture|setup|measure|archive|cycle|score [flags]\n" +
			"  fixture add --project P --name N --request FILE|- --scorer CMD [--weight W] [--kind public|signal] [--answer-key PATH...]\n" +
			"  fixture list|enable|disable [NAME] [--project P] [--json]\n" +
			"  fixture seed [--project P] [--json]   (starter set; every registered project without --project)\n" +
			"  setup DIR | status [--project P] [--json] | archive [--project P] [--json]\n" +
			"  measure --project P [--ref REF] [--tree DIR] [--fixture NAME...] [--shadow] [--json]\n" +
			"  cycle --project P [--once] [--agent A] [--json]\n" +
			"  score NAME   (starter fixture scorer; run inside the measured clone, prints 1 or 0)",
		Run: runEvolve,
	})
	cli.Register(cli.Command{
		Name: "reflect", Group: "agents", Raw: true, NoStore: true, ReadOnly: true,
		Summary: "read-only working state, board, ballot, hygiene and evolution gates",
		Usage:   "pulse reflect [--project P] [--area A] [--agent R/M] [--json]",
		Run:     runReflect,
	})
	cli.Register(cli.Command{
		Name: "fleet", Group: "agents", Raw: true, NoStore: true,
		Summary: "prepare, inspect and clean isolated worker clones",
		Usage: "pulse fleet prepare --n N [--ref REV] [--project P] [--json]\n" +
			"  pulse fleet verify|clean ID [--json]\n  pulse fleet list [--project P] [--json]",
		Run: runFleet,
	})
	cli.Register(cli.Command{
		Name: "usage", Group: "inspect", Raw: true, NoStore: true, ReadOnly: true,
		Summary: "best-effort local runner token diagnostics; never a ledger",
		Usage:   "pulse usage [--runner copilot|claude|codex|opencode] [--days N] [--json]",
		Run:     runUsage,
	})
}
