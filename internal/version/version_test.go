package version

import (
	"strings"
	"testing"
)

func TestStringFormat(t *testing.T) {
	result := String("mytool")

	// Should contain tool name
	if !strings.HasPrefix(result, "mytool ") {
		t.Errorf("String() should start with tool name, got: %s", result)
	}

	// Should contain parentheses with hash and status
	if !strings.Contains(result, "(") || !strings.Contains(result, ")") {
		t.Errorf("String() should contain parentheses, got: %s", result)
	}

	// Should contain comma separator
	if !strings.Contains(result, ", ") {
		t.Errorf("String() should contain comma separator, got: %s", result)
	}
}

func TestShortFormat(t *testing.T) {
	result := Short()

	// Should NOT contain a tool name prefix (no space before version)
	if strings.HasPrefix(result, " ") {
		t.Errorf("Short() should not start with space, got: %s", result)
	}

	// Should contain parentheses with hash and status
	if !strings.Contains(result, "(") || !strings.Contains(result, ")") {
		t.Errorf("Short() should contain parentheses, got: %s", result)
	}

	// Should contain comma separator
	if !strings.Contains(result, ", ") {
		t.Errorf("Short() should contain comma separator, got: %s", result)
	}
}

func TestDefaultValues(t *testing.T) {
	// With default values (not set via ldflags), we should still get valid output
	result := String("test")

	// Should contain version
	if !strings.Contains(result, Version) {
		t.Errorf("String() should contain version %s, got: %s", Version, result)
	}

	// Should contain git hash (even if "unknown")
	if !strings.Contains(result, GitHash) {
		t.Errorf("String() should contain git hash %s, got: %s", GitHash, result)
	}

	// Should contain git dirty status (even if "unknown")
	if !strings.Contains(result, GitDirty) {
		t.Errorf("String() should contain git dirty status %s, got: %s", GitDirty, result)
	}
}

func TestStringWithDifferentToolNames(t *testing.T) {
	testCases := []string{"bm", "dirc", "my-tool", "tool_name", ""}

	for _, toolName := range testCases {
		result := String(toolName)
		if !strings.HasPrefix(result, toolName+" ") {
			t.Errorf("String(%q) should start with tool name, got: %s", toolName, result)
		}
	}
}

func TestGitDirtyValidValues(t *testing.T) {
	// GitDirty should be one of: "dirty", "clean", or "unknown"
	validValues := map[string]bool{
		"dirty":   true,
		"clean":   true,
		"unknown": true,
	}

	if !validValues[GitDirty] {
		t.Errorf("GitDirty should be 'dirty', 'clean', or 'unknown', got: %s", GitDirty)
	}
}
