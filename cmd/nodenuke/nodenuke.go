package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"

	"github.com/timmattison/tools/internal"
)

var targetDirs = []string{"node_modules", ".next"}
var targetFiles = []string{"pnpm-lock.yaml", "package-lock.json"}

func main() {
	noRoot := flag.Bool("no-root", false, "Don't go to the git repository root before running")
	flag.Parse()

	cwd, err := os.Getwd()
	if err != nil {
		fmt.Printf("Error getting current directory: %v\n", err)
		os.Exit(1)
	}

	// If we're in a git repo and --no-root wasn't specified, go to the repo root
	if !*noRoot {
		gitDir, err := internal.GetRepoBase()
		if err == nil {
			// Found a git repo, change to its root directory
			repoRoot := filepath.Dir(gitDir)
			fmt.Printf("Found git repository, changing to root: %s\n", repoRoot)
			err = os.Chdir(repoRoot)
			if err != nil {
				fmt.Printf("Error changing to repository root: %v\n", err)
				os.Exit(1)
			}
			cwd = repoRoot
		} else if err != os.ErrNotExist {
			// Only report errors other than "not found"
			fmt.Printf("Error checking for git repository: %v\n", err)
		}
	}

	fmt.Println("Starting to scan from:", cwd)
	fmt.Println("Will delete directories:", targetDirs)
	fmt.Println("Will delete files:", targetFiles)

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
