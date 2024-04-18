package main_model

import (
	"encoding/hex"
	"github.com/charmbracelet/bubbles/progress"
	tea "github.com/charmbracelet/bubbletea"
	"golang.org/x/text/message"
	"hash"
	"io"
	"os"
	"time"
)

var Prhash *tea.Program

type MainModel struct {
	WindowWidth   int
	WindowHeight  int
	Printer       *message.Printer
	PausedChannel chan bool
	ProgressBar   progress.Model
	Quitting      bool
	InputFilename string
	InputFile     *os.File
	InputFileInfo os.FileInfo
	HashType      string
	Hasher        hash.Hash
	Position      int64
	StartTime     time.Time
	Paused        bool
	HashValue     string
	Err           error
}

type PrepareHashMsg struct{}

type OpenInputFileMsg struct {
	file     *os.File
	fileInfo os.FileInfo
	err      error
}

func PrepareHash() tea.Msg {
	return PrepareHashMsg{}
}

func OpenInputFile(inputFilename string) tea.Cmd {
	return func() tea.Msg {
		file, err := os.Open(inputFilename)

		if err != nil {
			return OpenInputFileMsg{file: file, fileInfo: nil, err: err}
		}

		fileInfo, err := file.Stat()

		return OpenInputFileMsg{file: file, fileInfo: fileInfo, err: err}
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

type HashProgressMsg struct {
	totalProcessed int64
}

type HashErrorMsg struct {
	err error
}

type HashFinishedMsg struct {
	hashValue string
}

func StartHash(hasher hash.Hash, inputFile *os.File, pausedChannel chan bool) tea.Cmd {
	return func() tea.Msg {
		buf := make([]byte, 16*1024*1024)

		var totalProcessed int64
		var paused bool

		for {
			n, err := inputFile.Read(buf)

			if n > 0 {
				// Write the read chunk to hasher
				var written int

				written, err = hasher.Write(buf[:n])

				if err != nil {
					return err
				}

				totalProcessed += int64(written)

				Prhash.Send(HashProgressMsg{totalProcessed: totalProcessed})
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
				return HashErrorMsg{err}
			}
		}

		return HashFinishedMsg{
			hashValue: hex.EncodeToString(hasher.Sum(nil)),
		}
	}
}
func (m MainModel) Init() tea.Cmd {
	return tea.Batch(PrepareHash, m.ProgressBar.Init())
}
