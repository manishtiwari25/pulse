package store

import (
	"encoding/json"
	"fmt"
	"strings"
)

// PostForOwner decodes the routing flag from post metadata, including textual
// booleans retained from frontmatter. Missing flags leave a post worker-facing.
func PostForOwner(extra string) (bool, error) {
	var metadata map[string]any
	if err := json.Unmarshal([]byte(extra), &metadata); err != nil {
		return false, fmt.Errorf("post extra: %w", err)
	}
	value, ok := metadata["for_owner"]
	if !ok {
		return false, nil
	}
	switch flag := value.(type) {
	case bool:
		return flag, nil
	case string:
		switch strings.ToLower(strings.TrimSpace(flag)) {
		case "true":
			return true, nil
		case "false":
			return false, nil
		}
	}
	return false, fmt.Errorf("post extra: for_owner must be true or false")
}
