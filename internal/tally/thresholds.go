package tally

import (
	"regexp"
	"strconv"

	"github.com/manishtiwari25/pulse/internal/store"
)

// ProtectedDefault is legacy/pulse.py PROTECTED_DEFAULT: the constitution
// sections no vote may amend when the constitution names none.
const ProtectedDefault = "Hard stops, Kill switch, Amendments, frontmatter"

// Thresholds are the tally's parameters (legacy/pulse.py thresholds()): flags
// win, then the constitution's Vote rules rows, then settings, then defaults.
// Zero int fields and empty strings in an override mean "not given".
type Thresholds struct {
	Quorum        int    `json:"quorum"`
	ExpiryDays    int    `json:"expiry_days"`
	StaleDays     int    `json:"stale_days"`
	AmendQuorum   int    `json:"amend_quorum"`
	AmendMajority string `json:"amend_majority"`
	Now           string `json:"now"` // fixed clock; "" means the store's clock

	// Derived from the constitution record and the settings; never overridden.
	AmendNum            int    `json:"amend_num"`
	AmendDen            int    `json:"amend_den"`
	Protected           string `json:"protected"`
	ConstitutionPresent bool   `json:"constitution_present"`
	StopPresent         bool   `json:"stop_present"`
	Constitution        string `json:"-"` // the constitution text the amendment checks run against
}

var (
	amendQuorumRE   = regexp.MustCompile(`[0-9]+`)
	amendMajorityRE = regexp.MustCompile(`[0-9]+/[0-9]+`)
)

// LoadThresholds resolves the thresholds for a project: override fields that
// are set win, then the "Vote rules" table of the project's constitution
// record (Amendment quorum, Amendment supermajority, Not amendable by vote),
// then the settings, then the built-in defaults. project "" or "commons" has
// no constitution.
//
// Differences from the Python: the constitution text comes from the
// EVO-constitution record (D-H) when one exists (else the project's
// docs/evolution/constitution.md, as before), and StopPresent is the `stop`
// setting (the kill switch, D-M) as well as the legacy docs/evolution/STOP file.
func LoadThresholds(s *store.Store, project string, override Thresholds) Thresholds {
	th := override
	text := ""
	if project != "" && project != store.CommonsSlug {
		text = ConstitutionText(s, project)
	}
	protected := ""
	if text != "" {
		if th.AmendQuorum == 0 {
			if cell, ok := tableRow(text, "Amendment quorum"); ok {
				if m := amendQuorumRE.FindString(cell); m != "" {
					th.AmendQuorum, _ = strconv.Atoi(m)
				}
			}
		}
		if th.AmendMajority == "" {
			if cell, ok := tableRow(text, "Amendment supermajority"); ok {
				th.AmendMajority = amendMajorityRE.FindString(cell)
			}
		}
		protected, _ = tableRow(text, "Not amendable by vote")
	}
	if th.Quorum == 0 {
		th.Quorum = s.SettingInt("quorum")
	}
	if th.ExpiryDays == 0 {
		th.ExpiryDays = s.SettingInt("expiry_days")
	}
	if th.StaleDays == 0 {
		th.StaleDays = s.SettingInt("stale_days")
	}
	if th.AmendQuorum == 0 {
		th.AmendQuorum = s.SettingInt("amend_quorum")
	}
	if th.AmendMajority == "" {
		th.AmendMajority = s.Setting("amend_majority", "")
	}
	if protected == "" {
		protected = ProtectedDefault
	}
	th.Protected = protected
	th.ConstitutionPresent = text != ""
	th.Constitution = text
	th.StopPresent = stopPresent(s, project)
	num, den, err := store.Majority(th.AmendMajority)
	if err != nil {
		// an unparsable fraction falls back to the default the way setting_int does
		num, den, _ = store.Majority(store.DefaultSettings["amend_majority"])
		th.AmendMajority = store.DefaultSettings["amend_majority"]
	}
	th.AmendNum, th.AmendDen = num, den
	return th
}

// ParseMajority parses a fraction such as "2/3" (legacy/pulse.py majority()).
func ParseMajority(value string) (int, int, error) {
	return store.Majority(value)
}
