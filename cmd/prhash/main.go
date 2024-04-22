package main

import (
	"crypto/md5"
	"crypto/sha1"
	"crypto/sha256"
	"crypto/sha512"
	"fmt"
	"github.com/charmbracelet/bubbles/progress"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/timmattison/tools/cmd/prhash/main-model"
	"github.com/timmattison/tools/internal"
	"hash"
	"os"
	"slices"
)

var validHashTypes = []string{"md5", "sha1", "sha256", "sha512"}

var hashImplementations = []func() hash.Hash{
	md5.New,
	sha1.New,
	sha256.New,
	sha512.New,
}

func main() {
	if len(os.Args) != 3 {
		fmt.Println("Missing required arguments.")
		fmt.Println("Usage:")
		fmt.Println("  prhash <hash type> <input file>")
		fmt.Println()

		printValidHashTypes()

		os.Exit(1)
	}

	hashType := os.Args[1]
	inputFilename := os.Args[2]

	var hasherIndex int

	if hasherIndex = slices.Index(validHashTypes, hashType); hasherIndex == -1 {
		fmt.Println("Invalid hash type.")

		printValidHashTypes()

		os.Exit(1)
	}

	progressBar := progress.New(progress.WithScaledGradient("#FF7CCB", "#FDFF8C"))

	printer := internal.GetLocalePrinter()

	pausedChannel := make(chan bool)

	myModel := main_model.MainModel{
		InputFilename: inputFilename,
		HashType:      hashType,
		Hasher:        hashImplementations[hasherIndex](),
		ProgressBar:   progressBar,
		Printer:       printer,
		PausedChannel: pausedChannel,
	}

	main_model.Prhash = tea.NewProgram(myModel)

	if _, err := main_model.Prhash.Run(); err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
}

func printValidHashTypes() {
	fmt.Println("Valid hash types are:")

	for _, validHashType := range validHashTypes {
		fmt.Println("  " + validHashType)
	}
}
