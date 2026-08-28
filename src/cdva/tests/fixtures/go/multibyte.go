package greet

import "testing"

// Greet greets in Japanese.
func Greet() string {
	return "こんにちは"
}

func TestGreet(t *testing.T) {
	if Greet() != "こんにちは" {
		t.Fatalf("挨拶が違う: %s", Greet())
	}
}
