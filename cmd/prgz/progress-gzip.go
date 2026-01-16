package main

import (
	"compress/gzip"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/charmbracelet/bubbles/progress"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
	"github.com/timmattison/tools/internal/version"
	"golang.org/x/text/language"
	"golang.org/x/text/message"
)

func sprintFloat(input float64) string {
	return getPrinter().Sprintf("%.2f", input)
}

func sprintInt(input int64) string {
	return getPrinter().Sprintf("%d", input)
}

func getPrinter() *message.Printer {
	var langEnv language.Tag
	var err error

	if langEnv, err = language.Parse(os.Getenv("LANG")); err != nil {
		langEnv = language.AmericanEnglish
	}

	return message.NewPrinter(langEnv)
}

type Model struct {
	progressBar            progress.Model
	counterWriter          *internal.ByteCounterWriter
	writtenBytes           int64
	totalSize              int64
	startTime              time.Time
	width                  int
	height                 int
	estimatedTimeRemaining time.Duration
	inputFile              *os.File
	outputFile             *os.File
	gzipWriter             *gzip.Writer
}

func OneTickPerSecond() tea.Cmd {
	return tea.Every(time.Second, func(t time.Time) tea.Msg {
		return tick()
	})
}

func (m Model) Init() tea.Cmd {
	return tea.Batch(
		OneTickPerSecond(),
		StartCompressing(m.totalSize, m.inputFile, m.outputFile, m.counterWriter, m.gzipWriter),
	)
}

func tick() tea.Msg {
	return time.Now()
}

func (m Model) Update(untypedMsg tea.Msg) (tea.Model, tea.Cmd) {
	var commands []tea.Cmd

	switch typedMessage := untypedMsg.(type) {
	case time.Time:
		m.writtenBytes = m.counterWriter.Count()

		if m.writtenBytes != 0 {
			elapsed := time.Since(m.startTime).Seconds()
			rate := float64(m.writtenBytes) / elapsed
			remaining := float64(m.totalSize-m.writtenBytes) / rate

			m.estimatedTimeRemaining = time.Duration(int64(remaining)) * time.Second
		} else {
			m.estimatedTimeRemaining = 0
		}

		commands = append(commands, OneTickPerSecond())
	case tea.WindowSizeMsg:
		m.width = typedMessage.Width
		m.height = typedMessage.Height
		m.progressBar.Width = typedMessage.Width - 10
	case tea.KeyMsg:
		if typedMessage.Type == tea.KeyCtrlC {
			return m, tea.Quit
		}
	case CompressionComplete:
		logFunc := log.Info
		logMessage := "Compression complete"

		if typedMessage.ReductionFlag == false {
			logFunc = log.Warn
			logMessage = "Compression complete, but the file size increased"
		}

		logFunc(logMessage, "originalSize", typedMessage.OriginalSize, "newSize", typedMessage.NewSize, "sizeChangePercent", typedMessage.SizeChangePercent, "duration", typedMessage.Duration, "bytesReadPerSecond", typedMessage.BytesReadPerSecond, "bytesWrittenPerSecond", typedMessage.BytesWrittenPerSecond)

		return m, tea.Quit
	case FatalError:
		log.Fatal(typedMessage.Message, "error", typedMessage.Error)
		return m, tea.Quit
	default:
		log.Warn("Unknown message", "msg", typedMessage, "type", fmt.Sprintf("%T", typedMessage))
		var data []byte
		var err error

		if data, err = json.Marshal(typedMessage); err != nil {
			log.Warn("Could not marshal the message", "error", err)
		} else {
			log.Warn("Marshalled message", "data", string(data))
		}
	}

	return m, tea.Batch(commands...)
}

func (m Model) View() string {
	var stringBuilder strings.Builder

	if m.writtenBytes == 0 {
		stringBuilder.WriteString("Waiting for data...")
	} else {
		stringBuilder.WriteString(fmt.Sprintf("Progress: %s %d/%d", m.progressBar.ViewAs(float64(m.writtenBytes)/float64(m.totalSize)), int(m.writtenBytes), int(m.totalSize)))
		stringBuilder.WriteString("\n")
		stringBuilder.WriteString(fmt.Sprintf("Estimated Time Remaining: %v", m.estimatedTimeRemaining))
	}

	return stringBuilder.String()
}

func main() {
	inputFilename := flag.String("input", "", "Input filename")
	outputFilename := flag.String("output", "", "Output filename (optional, defaults to input filename with .gz appended)")
	showVersion := flag.Bool("version", false, "Show version information")

	flag.Parse()

	if *showVersion {
		fmt.Println(version.String("prgz"))
		os.Exit(0)
	}

	if *inputFilename == "" {
		flag.Usage()
		os.Exit(1)
	}

	var inputFile *os.File
	var err error

	if inputFile, err = os.Open(*inputFilename); err != nil {
		log.Fatal("Could not open the input file", "error", err, "filename", *inputFilename)
	}

	defer inputFile.Close()

	var fileInfo os.FileInfo

	if fileInfo, err = inputFile.Stat(); err != nil {
		log.Fatal("Could not get the input file info", "error", err)
	}

	totalSize := fileInfo.Size()

	var outputFile *os.File

	if *outputFilename == "" {
		temp := *inputFilename + ".gz"
		outputFilename = &temp
	}

	if outputFile, err = os.Create(*outputFilename); err != nil {
		log.Fatal("Could not create the output file", "error", err, "filename", *outputFilename)
	}

	defer outputFile.Close()

	gzipWriter := gzip.NewWriter(outputFile)

	defer gzipWriter.Close()

	counterWriter := &internal.ByteCounterWriter{Writer: gzipWriter}

	prog := progress.New(progress.WithScaledGradient("#FF7CCB", "#FDFF8C"))

	initialModel := Model{progressBar: prog,
		counterWriter: counterWriter,
		writtenBytes:  0,
		totalSize:     totalSize,
		startTime:     time.Now(),
		inputFile:     inputFile,
		outputFile:    outputFile,
		gzipWriter:    gzipWriter,
	}

	p := tea.NewProgram(initialModel)

	if _, err = p.Run(); err != nil {
		log.Fatal("Something went wrong", "error", err)
	}
}

type FatalError struct {
	Message string
	Error   error
}

type CompressionComplete struct {
	OriginalSize          string
	NewSize               string
	ReductionFlag         bool
	SizeChangePercent     string
	Duration              time.Duration
	BytesReadPerSecond    string
	BytesWrittenPerSecond string
}

func StartCompressing(totalSize int64, inputFile *os.File, outputFile *os.File, counterWriter *internal.ByteCounterWriter, gzipWriter *gzip.Writer) tea.Cmd {
	return func() tea.Msg {
		startTime := time.Now()

		compressionFailed := false

		defer func() {
			if compressionFailed {
				os.Remove(outputFile.Name())
			}
		}()

		var err error

		if _, err = io.Copy(counterWriter, inputFile); err != nil {
			compressionFailed = true

			return FatalError{
				Message: "Could not compress the input file",
				Error:   err,
			}
		}

		compressionDuration := time.Since(startTime)

		var outputFileInfo os.FileInfo

		if outputFileInfo, err = outputFile.Stat(); err != nil {
			return FatalError{
				Message: "Could not get the output file info",
				Error:   err,
			}
		}

		newSize := outputFileInfo.Size()

		sizeDifference := totalSize - newSize

		gzipWriter.Close()
		outputFile.Close()

		reductionPercentage := sprintFloat((1 - float64(newSize)/float64(totalSize)) * 100)
		bytesReadPerSecond := sprintFloat(float64(totalSize) / compressionDuration.Seconds())
		bytesWrittenPerSecond := sprintFloat(float64(newSize) / compressionDuration.Seconds())

		return CompressionComplete{
			OriginalSize:          sprintInt(totalSize),
			NewSize:               sprintInt(newSize),
			ReductionFlag:         sizeDifference > 0,
			SizeChangePercent:     reductionPercentage,
			Duration:              compressionDuration,
			BytesReadPerSecond:    bytesReadPerSecond,
			BytesWrittenPerSecond: bytesWrittenPerSecond,
		}
	}
}
