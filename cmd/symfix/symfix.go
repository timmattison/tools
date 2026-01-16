package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal/version"
)

func main() {
	// Define flags
	var dirPath string
	flag.StringVar(&dirPath, "dir", ".", "Directory to scan for broken symlinks (default: current directory)")
	var verbose bool
	flag.BoolVar(&verbose, "verbose", false, "Enable verbose output")
	var help bool
	flag.BoolVar(&help, "help", false, "Show help message")
	var prependToFix string
	flag.StringVar(&prependToFix, "prepend-to-fix", "", "String to prepend to broken symlink targets to attempt fixing them")
	var removeToFix string
	flag.StringVar(&removeToFix, "remove-to-fix", "", "String to remove from the beginning of broken symlink targets to attempt fixing them")
	var showVersion bool
	flag.BoolVar(&showVersion, "version", false, "Show version information")
	flag.BoolVar(&showVersion, "V", false, "Show version information (shorthand)")

	// Custom usage message
	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, "Usage: symfix [options]\n\n")
		fmt.Fprintf(os.Stderr, "Recursively scans directories for broken symlinks and optionally fixes them.\n\n")
		fmt.Fprintf(os.Stderr, "When fixing symlinks, targets are resolved relative to the symlink's location.\n\n")
		fmt.Fprintf(os.Stderr, "Options:\n")
		flag.PrintDefaults()
		fmt.Fprintf(os.Stderr, "\nExamples:\n")
		fmt.Fprintf(os.Stderr, "  symfix -dir /path/to/scan                   # Just scan for broken symlinks\n")
		fmt.Fprintf(os.Stderr, "  symfix -prepend-to-fix ../                  # Prepend '../' to broken symlink targets\n")
		fmt.Fprintf(os.Stderr, "  symfix -remove-to-fix /old/path/prefix/     # Remove '/old/path/prefix/' from the beginning of targets\n")
		fmt.Fprintf(os.Stderr, "  symfix -dir /path/to/scan -prepend-to-fix .. # Scan in specified directory and fix symlinks\n")
	}

	// Parse flags
	flag.Parse()

	// Show version if requested
	if showVersion {
		fmt.Println(version.String("symfix"))
		os.Exit(0)
	}

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

	if verbose {
		logger.SetLevel(log.DebugLevel)
	}

	// Resolve the directory path to an absolute path
	absPath, err := filepath.Abs(dirPath)
	if err != nil {
		logger.Fatal("Failed to resolve absolute path", "error", err)
	}

	// Check if the directory exists
	info, err := os.Stat(absPath)
	if err != nil {
		logger.Fatal("Directory does not exist", "path", absPath, "error", err)
	}

	if !info.IsDir() {
		logger.Fatal("Path is not a directory", "path", absPath)
	}

	logger.Info("Scanning for broken symlinks", "directory", absPath)

	// Counters for broken and fixed symlinks
	brokenCount := 0
	fixedCount := 0

	// Walk the directory tree
	err = filepath.WalkDir(absPath, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			logger.Warn("Error accessing path", "path", path, "error", err)
			return nil // Continue walking
		}

		// Skip the entry if it's not a symlink
		info, err := d.Info()
		if err != nil {
			logger.Debug("Error getting file info", "path", path, "error", err)
			return nil // Continue walking
		}

		// Check if it's a symlink
		if info.Mode()&os.ModeSymlink != 0 {
			logger.Debug("Found symlink", "path", path)

			// Try to evaluate the symlink
			_, err := os.Stat(path)
			if err != nil {
				// Get the target of the symlink
				target, err := os.Readlink(path)
				if err != nil {
					logger.Warn("Error reading symlink", "path", path, "error", err)
					return nil
				}

				// Report the broken symlink
				fmt.Printf("Broken symlink: %s -> %s\n", path, target)
				brokenCount++

				// Try to fix the symlink if options are provided
				fixed := false

				// Try prepending to the target
				if prependToFix != "" {
					newTarget := prependToFix + target
					logger.Debug("Attempting to fix by prepending", "path", path, "original", target, "new", newTarget)

					// Get the directory containing the symlink
					symlinkDir := filepath.Dir(path)

					// Resolve the new target relative to the symlink's directory
					resolvedTarget := filepath.Join(symlinkDir, newTarget)
					logger.Debug("Resolving target relative to symlink directory", "symlinkDir", symlinkDir, "resolvedTarget", resolvedTarget)

					// Check if the new target exists
					_, err := os.Stat(resolvedTarget)
					if err == nil {
						// Update the symlink
						err = os.Remove(path)
						if err != nil {
							logger.Warn("Failed to remove old symlink", "path", path, "error", err)
						} else {
							err = os.Symlink(newTarget, path)
							if err != nil {
								logger.Warn("Failed to create new symlink", "path", path, "target", newTarget, "error", err)
							} else {
								fmt.Printf("Fixed symlink by prepending: %s -> %s\n", path, newTarget)
								fixed = true
								fixedCount++
							}
						}
					} else {
						logger.Debug("Prepended target does not exist", "path", path, "target", newTarget)
					}
				}

				// Try removing prefix from the target if not already fixed
				if !fixed && removeToFix != "" && strings.HasPrefix(target, removeToFix) {
					newTarget := strings.TrimPrefix(target, removeToFix)
					logger.Debug("Attempting to fix by removing prefix", "path", path, "original", target, "new", newTarget)

					// Get the directory containing the symlink
					symlinkDir := filepath.Dir(path)

					// Resolve the new target relative to the symlink's directory
					resolvedTarget := filepath.Join(symlinkDir, newTarget)
					logger.Debug("Resolving target relative to symlink directory", "symlinkDir", symlinkDir, "resolvedTarget", resolvedTarget)

					// Check if the new target exists
					_, err := os.Stat(resolvedTarget)
					if err == nil {
						// Update the symlink
						err = os.Remove(path)
						if err != nil {
							logger.Warn("Failed to remove old symlink", "path", path, "error", err)
						} else {
							err = os.Symlink(newTarget, path)
							if err != nil {
								logger.Warn("Failed to create new symlink", "path", path, "target", newTarget, "error", err)
							} else {
								fmt.Printf("Fixed symlink by removing prefix: %s -> %s\n", path, newTarget)
								fixedCount++
							}
						}
					} else {
						logger.Debug("Target with removed prefix does not exist", "path", path, "target", newTarget)
					}
				}
			}
		}

		return nil
	})

	if err != nil {
		logger.Fatal("Error walking directory", "error", err)
	}

	// Print summary
	if brokenCount == 0 {
		fmt.Println("No broken symlinks found.")
	} else {
		fmt.Printf("Found %d broken symlink(s).\n", brokenCount)
		if fixedCount > 0 {
			fmt.Printf("Fixed %d symlink(s).\n", fixedCount)
		} else if prependToFix != "" || removeToFix != "" {
			fmt.Println("No symlinks could be fixed with the provided options.")
		}
	}
}
