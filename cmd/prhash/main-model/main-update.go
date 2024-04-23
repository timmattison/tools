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
			m.AbnormalExit = true
		case key == tea.KeySpace:
			m.Paused = !m.Paused
			cmds = append(cmds, ChangeHashPauseState(m.Paused, m.PausedChannel))
		}
	case HashErrorMsg:
		m.Err = typedMessage.err
		m.AbnormalExit = true
	case HashFinishedMsg:
		m.Done = true
		m.HashValue = typedMessage.hashValue
	case HashProgressMsg:
		m.Position = typedMessage.totalProcessed
	case PrepareHashMsg:
		cmds = append(cmds, OpenInputFile(m.InputFilename))
	case OpenInputFileMsg:
		if typedMessage.err != nil {
			m.Err = typedMessage.err
			m.AbnormalExit = true
		} else {
			m.InputFile = typedMessage.file
			m.InputFileInfo = typedMessage.fileInfo
		}
	}

	if m.AbnormalExit || m.Done {
		cmds = append(cmds, CloseFile(m.InputFile))
		cmds = append(cmds, tea.Quit)
		return m, tea.Batch(cmds...)
	}

	var newModel tea.Model

	newModel, cmd = m.ProgressBar.Update(untypedMessage)
	cmds = append(cmds, cmd)

	if newProgressBar, ok := newModel.(progress.Model); ok {
		m.ProgressBar = newProgressBar
	}

	if m.InputFile != nil && m.StartTime.IsZero() {
		m.StartTime = time.Now()
		cmds = append(cmds, StartHash(m.Hasher, m.InputFile, m.PausedChannel))
	}

	return m, tea.Batch(cmds...)
}

func ChangeHashPauseState(paused bool, channel chan bool) tea.Cmd {
	return func() tea.Msg {
		channel <- paused
		return nil
	}
}
