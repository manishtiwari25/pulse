package hub

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"strconv"
	"strings"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Evolution is SQL-native, not a reconstruction of the old shell report.
// Missing imported records are null; absence of Phase 3's additive table is
// an empty live archive, not a failed request or invented historical result.
func (h *Handler) evolution(project string) (map[string]any, error) {
	records, err := h.Store.ListRecords(store.RecordFilter{Project: project, Kind: "evolution"})
	if err != nil {
		return nil, err
	}
	archive := object("rows", []map[string]any{})
	result := object("constitution evaluators archive loop", nil, nil, archive, nil)
	for _, record := range records {
		if record.Slug != "constitution" && record.Slug != "evaluators" && record.Slug != "archive" {
			continue
		}
		head, revision, err := h.Store.GetRecord(record.ID, 0)
		if err != nil {
			return nil, err
		}
		if record.Slug == "archive" {
			archive["record"] = additionRecord(*head)
			archive["body"] = revision.Body
		} else {
			seeded, from := seededRecord(head, revision)
			doc := object("record body seeded seeded_from", additionRecord(*head), revision.Body, seeded, from)
			doc["draft"], doc["draft_job"] = draftSummary(h.Store, head.ID, true), draftJobRef(h.Store, project)
			result[record.Slug] = doc
		}
	}
	var hasArchive int
	if err := h.Store.DB.QueryRow("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='evolution_archive'").Scan(&hasArchive); err != nil {
		return nil, err
	}
	if hasArchive != 0 {
		// fixtures_json may identify held-out fixtures. It is never part of
		// this projection, even when older archive writers populated it.
		rows, err := h.queryRows(context.Background(), `SELECT id,project,suggestion_post,hypothesis,branch,baseline_score,variant_score,
			tolerance,kept,sandbox,run_id,at,note FROM evolution_archive WHERE project=? ORDER BY at DESC,id DESC`, project)
		if err != nil {
			return nil, err
		}
		archive["rows"] = rows
	}
	heldout := h.Store.Setting(heldoutSetting, "")
	present := false
	if heldout != "" {
		info, err := os.Stat(heldout)
		switch {
		case err == nil:
			present = info.IsDir()
		case os.IsNotExist(err):
		default:
			// A PathError would reveal the private directory in the response.
			return nil, errors.New("the owner-private held-out directory could not be inspected")
		}
	}
	result["loop"] = object("loop limits tolerance stop heldout_present",
		h.Store.Setting("evolve.loop", ""), h.Store.Setting("evolve.limits", ""),
		h.Store.Setting("evolve.tolerance", ""), h.Store.Setting("stop", ""), present)
	// ADR-029: the loop's gates and its fixture names, as evolve status and
	// fixture list report them. A request or scorer is never projected: the
	// request is what an agent gets to read, the scorer is the owner's judge.
	gates, gatesErr := h.evolutionGates(project)
	result["gates"] = nil
	if gates != nil {
		result["gates"] = gates
	}
	if gatesErr != "" {
		result["gates_error"] = gatesErr
	}
	fixtures, err := h.evolutionFixtures(project)
	if err != nil {
		return nil, err
	}
	result["fixtures"] = fixtures
	if heldout != "" {
		redactEvolutionPath(result, heldout)
	}
	return result, nil
}

// seededRecord reports whether a record is still the text ADR-030 seeded -
// its first revision, marked seeded_from - and that mark
// (brain/TPL-evolution-constitution@r1, "" when the record was never seeded).
func seededRecord(head *store.Record, revision *store.Revision) (bool, string) {
	from := str(revision.Frontmatter["seeded_from"])
	return head.CurrentRev == 1 && from != "", from
}

// draftSummary is the draft waiting on a record (ADR-046) - {author, at,
// note, base_rev, job_id} and, when asked, body - or nil when none is filed.
func draftSummary(s *store.Store, id string, withBody bool) any {
	draft, err := s.GetDraft(id)
	if err != nil || draft == nil {
		return nil
	}
	out := object("author at note base_rev job_id", draft.Author, draft.At, draft.Note, draft.BaseRev, draft.JobID)
	if withBody {
		out["body"] = draft.Body
	}
	return out
}

// draftJobRef is {id, status} of the project's open drafting job, or nil.
func draftJobRef(s *store.Store, project string) any {
	ref, err := evolve.OpenDraftJob(s, project)
	if err != nil || ref == nil {
		return nil
	}
	return object("id status", ref.ID, ref.Status)
}

// evolutionGates is evolve status as a generic document so the private-path
// redaction can walk it; an error becomes a message, never a failed payload.
func (h *Handler) evolutionGates(project string) (map[string]any, string) {
	status, err := evolve.Status(h.Store, project)
	if err != nil {
		var ce *cli.Error
		if errors.As(err, &ce) {
			return nil, ce.What
		}
		return nil, err.Error()
	}
	encoded, err := json.Marshal(status)
	if err != nil {
		return nil, err.Error()
	}
	gates := map[string]any{}
	if err := json.Unmarshal(encoded, &gates); err != nil {
		return nil, err.Error()
	}
	return gates, ""
}

func (h *Handler) evolutionFixtures(project string) ([]map[string]any, error) {
	rows := []map[string]any{}
	var hasFixtures int
	if err := h.Store.DB.QueryRow("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='evolution_fixtures'").Scan(&hasFixtures); err != nil {
		return nil, err
	}
	if hasFixtures == 0 {
		return rows, nil
	}
	fixtures, err := evolve.ListFixtures(h.Store, project, false)
	if err != nil {
		return nil, err
	}
	for _, f := range fixtures {
		rows = append(rows, object("name kind enabled weight created_by at starter",
			f.Name, f.Kind, f.Enabled, f.Weight, f.CreatedBy, f.At, evolve.IsStarter(f.Name)))
	}
	return rows, nil
}

func redactEvolutionPath(value any, private string) {
	switch value := value.(type) {
	case map[string]any:
		for key, child := range value {
			if text, ok := child.(string); ok {
				value[key] = strings.ReplaceAll(text, private, privateValue)
			} else {
				redactEvolutionPath(child, private)
			}
		}
	case []map[string]any:
		for _, child := range value {
			redactEvolutionPath(child, private)
		}
	case []string:
		for i, text := range value {
			value[i] = strings.ReplaceAll(text, private, privateValue)
		}
	}
}

// The board overview and org cards retain the keys the original UI reads,
// while their values derive from the same SQL-native evolution document.
func evolutionSummary(evolution map[string]any) map[string]any {
	loop := evolution["loop"].(map[string]any)
	archive := evolution["archive"].(map[string]any)
	rows := archive["rows"].([]map[string]any)
	gates, _ := evolution["gates"].(map[string]any)
	baselines := 0
	for _, row := range rows {
		if evolve.IsBaselineRow(str(row["hypothesis"]), str(row["suggestion_post"])) {
			baselines++
		}
	}
	present := evolution["constitution"] != nil || evolution["evaluators"] != nil || archive["record"] != nil || len(rows) > 0
	loopName := "off"
	if loop["loop"] == "1" {
		loopName = "on"
	}
	summary := object("evolution_present stop_present missing_files loop limits limits_note baseline count_baselines count_variants count_incidents product_classes_ok product_evaluators_ok product_verdict hygiene_git last",
		strconv.Itoa(flagInt(present)), loop["stop"], []string{}, loopName, loop["limits"], "", "", strconv.Itoa(baselines), strconv.Itoa(len(rows)-baselines), "0",
		"0", "0", "", "", "5")
	if present {
		for _, slug := range []string{"constitution", "evaluators"} {
			if (gates != nil && gates[slug] != true) || (gates == nil && evolution[slug] == nil) {
				summary["missing_files"] = append(summary["missing_files"].([]string), "EVO-"+slug)
			}
		}
	}
	if gates == nil {
		note := "evolution gates unavailable"
		if detail := str(evolution["gates_error"]); detail != "" {
			note += ": " + detail
		}
		summary["limits_note"], summary["product_verdict"] = note, note
		return summary
	}
	summary["limits_note"] = str(gates["limit_problem"])
	if gates["baseline"] != nil {
		summary["baseline"] = str(gates["baseline_id"])
	}
	scope, _ := gates["product_scope"].(map[string]any)
	allowed, known := scope["allowed"].(bool)
	if !known {
		summary["product_verdict"] = "product scope unavailable"
		return summary
	}
	// These legacy flags describe configured product scope, not permission
	// for a particular experiment. Level D alone does not grant that scope.
	summary["product_classes_ok"] = strconv.Itoa(flagInt(allowed && gates["constitution"] == true))
	summary["product_evaluators_ok"] = strconv.Itoa(flagInt(allowed && gates["evaluators"] == true))
	summary["product_verdict"] = str(scope["reason"])
	return summary
}
