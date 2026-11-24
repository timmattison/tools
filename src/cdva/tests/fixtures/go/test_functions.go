package math

import "testing"

// Add returns the sum of its two arguments.
func Add(a, b int) int {
	return a + b
}

func TestAdd(t *testing.T) {
	if Add(1, 2) != 3 {
		t.Fatal("addition is broken")
	}
}
