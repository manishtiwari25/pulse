// Package webhub contains the committed Next.js static export.
package webhub

import "embed"

//go:embed all:out
var Dist embed.FS
