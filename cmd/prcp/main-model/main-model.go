package main_model

import (
	"github.com/charmbracelet/bubbles/progress"
	tea "github.com/charmbracelet/bubbletea"
	"golang.org/x/text/message"
	"io"
	"os"
	"time"
)

var Prcp *tea.Program

type MainModel struct {
	WindowWidth         int
	WindowHeight        int
	Printer             *message.Printer
	PausedChannel       chan bool
	ProgressBar         progress.Model
	Quitting            bool
	SourceFilename      string
	SourceFile          *os.File
	SourceFileInfo      os.FileInfo
	DestinationFilename string
	DestinationFile     *os.File
	DestinationPosition int64
	StartTime           time.Time
	Paused              bool
	Err                 error
}

type PrepareTransferMsg struct{}

type OpenSourceFileMsg struct {
	file     *os.File
	fileInfo os.FileInfo
	err      error
}

type OpenDestinationFileMsg struct {
	file *os.File
	err  error
}

func PrepareTransfer() tea.Msg {
	return PrepareTransferMsg{}
}

func OpenSourceFile(sourceFilename string) tea.Cmd {
	return func() tea.Msg {
		file, err := os.Open(sourceFilename)

		if err != nil {
			return OpenSourceFileMsg{file: file, fileInfo: nil, err: err}
		}

		fileInfo, err := file.Stat()

		return OpenSourceFileMsg{file: file, fileInfo: fileInfo, err: err}
	}
}

func OpenDestinationFile(destinationFilename string) tea.Cmd {
	return func() tea.Msg {
		file, err := os.Create(destinationFilename)
		return OpenDestinationFileMsg{file, err}
	}
}

func CloseFile(file *os.File) tea.Cmd {
	return func() tea.Msg {
		if file == nil {
			return nil
		}

		return file.Close()
	}
}

type CopyProgressMsg struct {
	totalWritten int64
}

type CopyErrorMsg struct {
	err error
}

type CopyFinishedMsg struct{}

func StartTransfer(sourceFile *os.File, destinationFile *os.File, pausedChannel chan bool) tea.Cmd {
	return func() tea.Msg {
		buf := make([]byte, 16*1024*1024)

		var totalWritten int64
		var paused bool

		for {
			n, err := sourceFile.Read(buf)

			if n > 0 {
				// Write the read chunk to the destination file.
				var written int
				written, err = destinationFile.Write(buf[:n])

				if err != nil {
					return err
				}

				totalWritten += int64(written)

				Prcp.Send(CopyProgressMsg{totalWritten: totalWritten})
			}

			// Check to see if there's a pause request
			select {
			case paused = <-pausedChannel:
			default:
			}

			// If we're paused, wait until we're unpaused
			for paused {
				select {
				case paused = <-pausedChannel:
				default:
					time.Sleep(100 * time.Millisecond)
				}
			}

			if err == io.EOF {
				break
			}

			if err != nil {
				return CopyErrorMsg{err}
			}
		}

		return CopyFinishedMsg{}
	}
}
func (m MainModel) Init() tea.Cmd {
	return tea.Batch(PrepareTransfer, m.ProgressBar.Init())
}
