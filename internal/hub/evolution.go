package hub

import (
	"context"
	"errors"
	"os"
	"strconv"
	"strings"

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
			result[record.Slug] = object("record body", additionRecord(*head), revision.Body)
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
	if heldout != "" {
		redactEvolutionPath(result, heldout)
	}
	return result, nil
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
	present := evolution["constitution"] != nil || evolution["evaluators"] != nil || archive["record"] != nil || len(rows) > 0
	loopName := "off"
	if loop["loop"] == "1" {
		loopName = "on"
	}
	summary := object("evolution_present stop_present missing_files loop limits limits_note baseline count_baselines count_variants count_incidents product_classes_ok product_evaluators_ok product_verdict hygiene_git last",
		strconv.Itoa(flagInt(present)), loop["stop"], []string{}, loopName, loop["limits"], "", "", "0", strconv.Itoa(len(rows)), "0",
		"0", strconv.Itoa(flagInt(evolution["evaluators"] != nil)), "", "", "5")
	if present {
		for _, slug := range []string{"constitution", "evaluators"} {
			if evolution[slug] == nil {
				summary["missing_files"] = append(summary["missing_files"].([]string), "EVO-"+slug)
			}
		}
	}
	return summary
}
