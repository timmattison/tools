package main

import (
	"strconv"
	"testing"
)

func TestPortGeneration(t *testing.T) {
	for i := 0; i < 1000000; i++ {
		inputString := strconv.Itoa(i)

		port := unprivilegedPortNumberFromString(inputString)

		if port < 1024 {
			t.Errorf("Generated port %d is out of valid range", port)
		}
	}
}
