package main

import (
	"fmt"
	"github.com/charmbracelet/bubbles/progress"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/timmattison/tools/cmd/prcp/main-model"
	"github.com/timmattison/tools/internal"
	"os"
)

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

	printer := internal.GetLocalePrinter()

	pausedChannel := make(chan bool)

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
