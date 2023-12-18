package main

import (
	"crypto/sha256"
	"crypto/sha512"
	"fmt"
	"github.com/edsrzf/mmap-go"
	"io"
	"os"
	"path/filepath"
	"strings"
)

var hashChannel chan string

func hash(path string) (string, error) {
	file, err := os.Open(path)

	if err != nil {
		return "", err
	}

	defer file.Close()

	fileMmap, _ := mmap.Map(file, mmap.RDONLY, 0)
	defer fileMmap.Unmap()

	h := sha512.New()

	if _, err = io.Copy(h, file); err != nil {
		return "", err
	}

	sum := h.Sum(nil)

	return fmt.Sprintf("%x", sum), nil
}

func hashString(input string) string {
	h := sha256.New()

	h.Write([]byte(input))

	return fmt.Sprintf("%x", h.Sum(nil))
}

func visit(path string, info os.DirEntry, err error) error {
	if err != nil {
		fmt.Printf("error visiting path %s: %v\n", path, err)
		return err
	}

	if info.IsDir() {
		// Ignore it
	} else {
		var hashed string

		hashed, err = hash(path)

		if err != nil {
			panic(err)
		}

		name := filepath.Base(path)

		fmt.Printf("%s %s\n", name, hashed)
		hashChannel <- hashed
	}

	return nil
}

// A very interesting quicksort from https://gist.github.com/ncw/5419af0e255d2fb62b98
func quicksort(in, out chan string) {
	defer close(out)
	pivot, ok := <-in

	// Finish recursion if no input - is sorted
	if !ok {
		return
	}

	// Divide and conquer the problem

	leftIn := make(chan string)
	leftOut := make(chan string)

	go quicksort(leftIn, leftOut)

	rightIn := make(chan string)
	rightOut := make(chan string)

	go quicksort(rightIn, rightOut)

	// Feed the two halves
	go func() {
		for i := range in {
			if i < pivot {
				leftIn <- i
			} else {
				rightIn <- i
			}
		}

		close(leftIn)
		close(rightIn)
	}()

	// Join the sorted streams
	for i := range leftOut {
		out <- i
	}

	out <- pivot

	for i := range rightOut {
		out <- i
	}
}

func main() {
	if len(os.Args) != 2 {
		fmt.Println("Missing required arguments.")
		fmt.Println("Usage:")
		fmt.Println("  dirhash <directory>")
		os.Exit(1)
	}

	directory := os.Args[1]

	hashChannel = make(chan string, 1000)

	if err := filepath.WalkDir(directory, visit); err != nil {
		panic(err)
	}

	close(hashChannel)

	sortedHashChannel := make(chan string)

	go quicksort(hashChannel, sortedHashChannel)

	var finalString strings.Builder

	for sortedHash := range sortedHashChannel {
		finalString.WriteString(sortedHash)
	}

	fmt.Println(hashString(finalString.String()))
}
