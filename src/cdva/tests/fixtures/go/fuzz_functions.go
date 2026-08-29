package parse

import "testing"

// Parse reads a word out of a string.
func Parse(s string) string {
	return s
}

func FuzzParse(f *testing.F) {
	f.Fuzz(func(t *testing.T, s string) {
		Parse(s)
	})
}
