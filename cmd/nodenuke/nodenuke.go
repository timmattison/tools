package main

import (
	"fmt"
	"os"
	"path/filepath"
)

var targetDirs = []string{"node_modules", ".next"}
var targetFiles = []string{"pnpm-lock.yaml", "package-lock.json"}

func main() {
	cwd, err := os.Getwd()
	if err != nil {
		fmt.Printf("Error getting current directory: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Starting to scan from:", cwd)
	fmt.Println("Will delete directories:", targetDirs)
	fmt.Println("Will delete files:", targetFiles)
	fmt.Println("Press Enter to continue or Ctrl+C to cancel...")
	fmt.Scanln()

	err = filepath.Walk(cwd, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			fmt.Printf("Error accessing %s: %v\n", path, err)
			return nil // continue walking
		}

		// Check for target directories
		if info.IsDir() {
			baseName := filepath.Base(path)
			for _, target := range targetDirs {
				if baseName == target {
					fmt.Printf("Removing directory: %s\n", path)
					err = os.RemoveAll(path)
					if err != nil {
						fmt.Printf("Error removing %s: %v\n", path, err)
					}
					return filepath.SkipDir // skip this directory after removing it
				}
			}
		}

		// Check for target files
		if !info.IsDir() {
			baseName := filepath.Base(path)
			for _, target := range targetFiles {
				if baseName == target {
					fmt.Printf("Removing file: %s\n", path)
					err = os.Remove(path)
					if err != nil {
						fmt.Printf("Error removing %s: %v\n", path, err)
					}
					break
				}
			}
		}

		return nil
	})

	if err != nil {
		fmt.Printf("Error walking directory tree: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Cleanup complete!")
}
