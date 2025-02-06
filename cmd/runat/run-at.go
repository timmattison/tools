package main

import (
	"fmt"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/log"
	"os"
	"os/exec"
	"strings"
	"time"
)

var (
	timeStyle      = lipgloss.NewStyle().Bold(true)
	remainingStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("196")).Bold(true)
	commandStyle   = lipgloss.NewStyle().Bold(true)
	separatorStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("241")) // darker grey
	timezoneStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("241")) // darker grey
)

var program *tea.Program

type model struct {
	targetTime time.Time
	command    []string
	quitting   bool
}

// Custom fatal logger that ensures terminal cleanup
func fatalWithCleanup(msg string, args ...interface{}) {
	// Let bubbletea handle the cleanup by using tea.Quit
	if program != nil {
		program.Kill()
	}
	// Log the error and exit
	log.Fatal(msg, args...)
}

func (m model) Init() tea.Cmd {
	return tea.Tick(time.Second, func(t time.Time) tea.Msg {
		return t
	})
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		if msg.Type == tea.KeyCtrlC {
			m.quitting = true
			return m, tea.Quit
		}
	case time.Time:
		if msg.After(m.targetTime) || msg.Equal(m.targetTime) {
			cmd := exec.Command(m.command[0], m.command[1:]...)
			cmd.Stdin = os.Stdin
			cmd.Stdout = os.Stdout
			cmd.Stderr = os.Stderr

			if err := cmd.Run(); err != nil {
				fatalWithCleanup("Failed to run command", "error", err)
			}

			return m, tea.Quit
		}

		return m, tea.Tick(time.Second, func(t time.Time) tea.Msg {
			return t
		})
	}

	return m, nil
}

func (m model) View() string {
	if m.quitting {
		return "Cancelled\n"
	}

	now := time.Now()
	remaining := m.targetTime.Sub(now)

	hours := int(remaining.Hours())
	minutes := int(remaining.Minutes()) % 60
	seconds := int(remaining.Seconds()) % 60

	return fmt.Sprintf(
		"%s\n%s\n\n%s\n\n%s %s\n\n%s\n",
		"Current time: "+formatTime(now),
		"Target time:  "+formatTime(m.targetTime),
		"Time remaining: "+remainingStyle.Render(fmt.Sprintf("%02d:%02d:%02d", hours, minutes, seconds)),
		"Command:", commandStyle.Render(strings.Join(m.command, " ")),
		"Press CTRL-C to abort",
	)
}

func formatTime(t time.Time) string {
	datePart := t.Format("2006-01-02")
	timePart := t.Format("15:04:05")
	timezonePart := t.Format("Z07:00")

	return fmt.Sprintf("%s%s%s%s",
		timeStyle.Render(datePart),
		separatorStyle.Render("T"),
		timeStyle.Render(timePart),
		timezoneStyle.Render(timezonePart),
	)
}

func parseTimeString(timeStr string) (time.Time, error) {
	// First try parsing as RFC3339 (with timezone)
	if t, err := time.Parse(time.RFC3339, timeStr); err == nil {
		return t, nil
	}

	// Try parsing common formats without timezone (assume local)
	formats := []string{
		"2006-01-02T15:04:05",
		"2006-01-02 15:04:05",
		"2006-01-02 15:04",
		"15:04:05",
		"15:04",
	}

	now := time.Now()
	today := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.Local)

	for _, format := range formats {
		if t, err := time.ParseInLocation(format, timeStr, time.Local); err == nil {
			// For time-only formats, use today's date
			if format == "15:04:05" || format == "15:04" {
				t = time.Date(today.Year(), today.Month(), today.Day(),
					t.Hour(), t.Minute(), t.Second(), 0, time.Local)

				// If the time has already passed today, schedule for tomorrow
				if t.Before(now) {
					t = t.Add(24 * time.Hour)
				}
			}
			return t, nil
		}
	}

	return time.Time{}, fmt.Errorf("could not parse time: %s", timeStr)
}

func main() {
	// Set up cleanup for any panic situations
	defer func() {
		if r := recover(); r != nil {
			fmt.Print("\033[?25h") // Show cursor
			fmt.Print("\033[2J")   // Clear screen
			fmt.Print("\033[H")    // Move cursor to home position
			panic(r)               // Re-panic after cleanup
		}
	}()

	if len(os.Args) < 3 {
		fmt.Println("Usage: runat <timestamp> <command> [args...]")
		fmt.Println("Examples:")
		fmt.Println("  runat 2024-01-01T12:00:00Z echo hello world    # UTC time")
		fmt.Println("  runat 2024-01-01T12:00:00 echo hello world     # Local time")
		fmt.Println("  runat \"2024-01-01 12:00\" echo hello world      # Local time")
		fmt.Println("  runat 12:00 echo hello world                   # Today/tomorrow at 12:00 local time")
		os.Exit(1)
	}

	targetTime, err := parseTimeString(os.Args[1])
	if err != nil {
		fatalWithCleanup("Invalid timestamp format", "error", err)
	}

	if targetTime.Before(time.Now()) {
		fatalWithCleanup("Target time must be in the future")
	}

	command := os.Args[2:]

	p := tea.NewProgram(
		model{
			targetTime: targetTime,
			command:    command,
		},
		tea.WithAltScreen(), // This handles terminal cleanup
	)
	program = p

	if _, err := p.Run(); err != nil {
		fatalWithCleanup("Error running program", "error", err)
	}
}
