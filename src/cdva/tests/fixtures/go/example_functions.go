package greet

import "fmt"

// Greet says hello, and says nothing else.
func Greet() string {
	return "hello"
}

func ExampleGreet() {
	fmt.Println(Greet())
	// Output: hello
}
