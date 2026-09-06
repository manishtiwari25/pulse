package evolve

import "fmt"

func formatDelta(verdict string, delta, tolerance float64) string {
	return fmt.Sprintf("%s: variant %+.4f vs baseline (tolerance %.4f)", verdict, delta, tolerance)
}
