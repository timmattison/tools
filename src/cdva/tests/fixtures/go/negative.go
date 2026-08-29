package trust

// Testify builds a witness. The name opens with Test and the fifth
// character is lower case, so it is production code.
func Testify() string {
	return "witness"
}

// TestingHelper helps a test without being one.
func TestingHelper() int {
	return 1
}
