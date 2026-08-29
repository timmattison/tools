package cache

import "testing"

// Warm fills the cache before a run.
func Warm() int {
	return 1
}

func BenchmarkWarm(b *testing.B) {
	for i := 0; i < b.N; i++ {
		Warm()
	}
}
