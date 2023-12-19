package main_model

import (
	"github.com/charmbracelet/bubbles/progress"
	tea "github.com/charmbracelet/bubbletea"
	"time"
)

func (m MainModel) Update(untypedMessage tea.Msg) (tea.Model, tea.Cmd) {
	var cmd tea.Cmd
	var cmds []tea.Cmd

	switch typedMessage := untypedMessage.(type) {
	case tea.WindowSizeMsg:
		m.WindowWidth = typedMessage.Width
		m.WindowHeight = typedMessage.Height
	case tea.KeyMsg:
		switch key := typedMessage.Type; {
		case key == tea.KeyCtrlC:
			m.Quitting = true
		case key == tea.KeySpace:
			m.Paused = !m.Paused
			cmds = append(cmds, ChangeTransferPauseState(m.Paused, m.PausedChannel))
		}
	case CopyErrorMsg:
		m.Err = typedMessage.err
		m.Quitting = true
	case CopyFinishedMsg:
		m.Quitting = true
	case CopyProgressMsg:
		m.DestinationPosition = typedMessage.totalWritten
	case PrepareTransferMsg:
		cmds = append(cmds, OpenSourceFile(m.SourceFilename), OpenDestinationFile(m.DestinationFilename))
	case OpenSourceFileMsg:
		if typedMessage.err != nil {
			m.Err = typedMessage.err
			m.Quitting = true
		} else {
			m.SourceFile = typedMessage.file
			m.SourceFileInfo = typedMessage.fileInfo
		}
	case OpenDestinationFileMsg:
		if typedMessage.err != nil {
			m.Err = typedMessage.err
			m.Quitting = true
		} else {
			m.DestinationFile = typedMessage.file
		}
	}

	if m.Quitting {
		cmds = append(cmds, CloseFile(m.SourceFile))
		cmds = append(cmds, CloseFile(m.DestinationFile))
		cmds = append(cmds, tea.Quit)
		return m, tea.Batch(cmds...)
	}

	var newModel tea.Model

	newModel, cmd = m.ProgressBar.Update(untypedMessage)
	cmds = append(cmds, cmd)

	if newProgressBar, ok := newModel.(progress.Model); ok {
		m.ProgressBar = newProgressBar
	}

	if m.SourceFile != nil && m.DestinationFile != nil && m.StartTime.IsZero() {
		m.StartTime = time.Now()
		cmds = append(cmds, StartTransfer(m.SourceFile, m.DestinationFile, m.PausedChannel))
	}

	return m, tea.Batch(cmds...)
}

func ChangeTransferPauseState(paused bool, channel chan bool) tea.Cmd {
	return func() tea.Msg {
		channel <- paused
		return nil
	}
}
