package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/charmbracelet/log"
	"golang.design/x/clipboard"
)

func main() {
	// Define flags
	var help bool
	flag.BoolVar(&help, "help", false, "Show help message")
	var pasteMode bool
	flag.BoolVar(&pasteMode, "paste", false, "Paste cd command for directory in clipboard")

	// Custom usage message
	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, "Usage: dirc [options]\n\n")
		fmt.Fprintf(os.Stderr, "A tool that can either:\n")
		fmt.Fprintf(os.Stderr, "1. Output a 'cd' command to the directory path stored in the clipboard (default)\n")
		fmt.Fprintf(os.Stderr, "2. Copy the current working directory to the clipboard (-copy mode)\n\n")
		fmt.Fprintf(os.Stderr, "NOTE: This tool cannot directly change your shell's directory.\n")
		fmt.Fprintf(os.Stderr, "To use it effectively, you need to evaluate its output in your shell:\n\n")
		fmt.Fprintf(os.Stderr, "  Bash/Zsh: eval $(dirc -paste)\n")
		fmt.Fprintf(os.Stderr, "  Fish:      eval (dirc -paste)\n\n")
		fmt.Fprintf(os.Stderr, "TIP: Add this alias to your shell config:\n")
		fmt.Fprintf(os.Stderr, "  Bash/Zsh: alias dirp='eval $(dirc -paste)'\n")
		fmt.Fprintf(os.Stderr, "  Fish:      alias dirp='eval (dirc -paste)'\n\n")
		fmt.Fprintf(os.Stderr, "Options:\n")
		flag.PrintDefaults()
	}

	// Parse flags
	flag.Parse()

	// Show help if requested
	if help {
		flag.Usage()
		os.Exit(0)
	}

	// Set up logging
	logger := log.NewWithOptions(os.Stderr, log.Options{
		Level:           log.InfoLevel,
		ReportTimestamp: false,
	})

	// Initialize clipboard
	err := clipboard.Init()

	if err != nil {
		logger.Fatal("Failed to initialize clipboard", "error", err)
	}

	// Handle copy mode
	if !pasteMode {
		// Get current directory
		currentDir, err := os.Getwd()
		if err != nil {
			logger.Fatal("Failed to get current directory", "error", err)
		}

		// Get absolute path
		absPath, err := filepath.Abs(currentDir)
		if err != nil {
			logger.Fatal("Failed to resolve absolute path", "error", err)
		}

		// Copy to clipboard
		clipboard.Write(clipboard.FmtText, []byte(absPath))

		fmt.Println("Copied to clipboard:", absPath)

		return
	}

	// Handle paste mode

	// Read from clipboard
	clipContent := clipboard.Read(clipboard.FmtText)
	if len(clipContent) == 0 {
		logger.Fatal("Clipboard is empty")
	}

	// Convert to string and trim whitespace
	dirPath := strings.TrimSpace(string(clipContent))
	if dirPath == "" {
		logger.Fatal("Clipboard contains only whitespace")
	}

	logger.Debug("Read from clipboard", "content", dirPath)

	// Check if the path exists and is a directory
	fileInfo, err := os.Stat(dirPath)

	if err != nil {
		logger.Fatal("Invalid directory path in clipboard", "path", dirPath, "error", err)
	}

	if !fileInfo.IsDir() {
		logger.Fatal("Path in clipboard is not a directory", "path", dirPath)
	}

	// Get absolute path
	absPath, err := filepath.Abs(dirPath)

	if err != nil {
		logger.Fatal("Failed to resolve absolute path", "error", err)
	}

	// Escape single quotes in the path for shell safety
	escapedPath := strings.ReplaceAll(absPath, "'", "'\\''")
	fmt.Printf("cd '%s'\n", escapedPath)
}
