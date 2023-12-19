package main

import (
	"fmt"
	"github.com/charmbracelet/bubbles/progress"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/timmattison/tools/cmd/prcp/main-model"
	"golang.org/x/text/language"
	"golang.org/x/text/message"
	"os"
)

func getUserLocale() language.Tag {
	// Get the preferred locale from the environment variables
	locale := os.Getenv("LC_ALL")

	if locale == "" {
		locale = os.Getenv("LC_MESSAGES")
	}

	if locale == "" {
		locale = os.Getenv("LANG")
	}

	if locale == "" {
		// Default fallback if no environment variable is set
		locale = "en_US.UTF-8"
	}

	// Parse the locale code
	tag, err := language.Parse(locale)
	if err != nil {
		// Fallback to default language if parsing failed
		return language.English
	}

	return tag
}

func main() {
	if len(os.Args) != 3 {
		fmt.Println("Missing required arguments.")
		fmt.Println("Usage:")
		fmt.Println("  prcp <source file> <destination file>")
		os.Exit(1)
	}

	sourceFile := os.Args[1]
	destinationFile := os.Args[2]

	progressBar := progress.New(progress.WithScaledGradient("#FF7CCB", "#FDFF8C"))

	printer := message.NewPrinter(getUserLocale())

	pausedChannel := make(chan bool, 10)

	myModel := main_model.MainModel{
		SourceFilename:      sourceFile,
		DestinationFilename: destinationFile,
		ProgressBar:         progressBar,
		Printer:             printer,
		PausedChannel:       pausedChannel,
	}

	main_model.Prcp = tea.NewProgram(myModel)

	if _, err := main_model.Prcp.Run(); err != nil {
		fmt.Println(err)
		os.Exit(1)
	}
}
