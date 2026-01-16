// Package version provides build-time version information for Go tools.
//
// Usage:
//
//	import "github.com/timmattison/tools/internal/version"
//
//	func main() {
//	    var showVersion = flag.Bool("version", false, "Show version information")
//	    flag.Parse()
//
//	    if *showVersion {
//	        fmt.Println(version.String("toolname"))
//	        os.Exit(0)
//	    }
//	}
//
// Build with ldflags to set version info:
//
//	go build -ldflags "-X github.com/timmattison/tools/internal/version.GitHash=$(git rev-parse --short=7 HEAD) \
//	                   -X github.com/timmattison/tools/internal/version.GitDirty=$(if git diff --quiet 2>/dev/null; then echo clean; else echo dirty; fi) \
//	                   -X github.com/timmattison/tools/internal/version.Version=0.1.0"
package version

import "fmt"

// These variables are set at build time via ldflags.
var (
	// Version is the semantic version (e.g., "0.1.0")
	Version = "0.1.0"
	// GitHash is the short git commit hash (e.g., "abc1234")
	GitHash = "unknown"
	// GitDirty is "dirty", "clean", or "unknown"
	GitDirty = "unknown"
)

// String returns a formatted version string for the given tool name.
// Format: "toolname 0.1.0 (abc1234, clean)"
func String(toolName string) string {
	return fmt.Sprintf("%s %s (%s, %s)", toolName, Version, GitHash, GitDirty)
}

// Short returns just the version info without tool name.
// Format: "0.1.0 (abc1234, clean)"
func Short() string {
	return fmt.Sprintf("%s (%s, %s)", Version, GitHash, GitDirty)
}
