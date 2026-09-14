package tally

import (
	"context"
	"errors"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// SuggestionVoteCounts returns the canonical latest non-self vote counts for
// one project-bound suggestion, using the same post ordering and self-vote
// rules as Compute.
func SuggestionVoteCounts(s *store.Store, project, suggestionID string) (up, down int, found bool, err error) {
	return SuggestionVoteCountsContext(context.Background(), s, project, suggestionID)
}

func SuggestionVoteCountsContext(ctx context.Context, s *store.Store, project, suggestionID string) (up, down int, found bool, err error) {
	const maxVoteRows = 10000
	ctx, cancel := context.WithTimeout(ctx, time.Second)
	defer cancel()
	rows, err := s.DB.QueryContext(ctx, `SELECT id, kind, date, agent, session, on_post, vote FROM posts
		WHERE project = ? AND (id = ? OR (kind = 'vote' AND on_post = ?))
		ORDER BY area, thread, id LIMIT ?`, project, suggestionID, suggestionID, maxVoteRows+1)
	if err != nil {
		return 0, 0, false, err
	}
	defer rows.Close()
	var suggestion *Post
	votes := newOmap[voteKey, svoteInfo]()
	n := 0
	for rows.Next() {
		n++
		if n > maxVoteRows {
			return 0, 0, false, errors.New("suggestion vote evidence exceeds the bounded scan")
		}
		var r Post
		if err := rows.Scan(&r.ID, &r.Kind, &r.Date, &r.Agent, &r.Session, &r.OnPost, &r.Vote); err != nil {
			return 0, 0, false, err
		}
		if r.Kind == "suggestion" && r.ID == suggestionID {
			copy := r
			suggestion = &copy
		}
		if r.Kind == "vote" && r.OnPost != "" {
			vk := voteKey{r.OnPost, r.Agent}
			if v, ok := votes.get(vk); !ok || r.Date >= v.date {
				votes.set(vk, svoteInfo{date: r.Date, vote: r.Vote, id: r.ID, session: r.Session})
			}
		}
		if err := ctx.Err(); err != nil {
			return 0, 0, false, err
		}
	}
	if err := rows.Err(); err != nil {
		return 0, 0, false, err
	}
	if err := ctx.Err(); err != nil {
		return 0, 0, false, err
	}
	if suggestion == nil {
		return 0, 0, false, nil
	}
	for _, vk := range votes.keys {
		v := votes.m[vk]
		if vk.on != suggestionID || vk.who == suggestion.Agent || sameSession(v.session, suggestion.Session) {
			continue
		}
		if v.vote == "down" {
			down++
		} else {
			up++
		}
	}
	return up, down, true, nil
}
