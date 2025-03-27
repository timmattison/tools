package main

import (
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
)

func main() {
	useLatest := flag.Bool("latest", false, "Use --latest flag with npm or -L with pnpm")
	forceNpm := flag.Bool("npm", false, "Force using npm for all directories")
	forcePnpm := flag.Bool("pnpm", false, "Force using pnpm for all directories")
	flag.Parse()

	// Check for conflicting flags
	if *forceNpm && *forcePnpm {
		fmt.Println("Error: Cannot specify both --npm and --pnpm flags")
		os.Exit(1)
	}

	cwd, err := os.Getwd()
	if err != nil {
		fmt.Printf("Error getting current directory: %v\n", err)
		os.Exit(1)
	}

	// Check if there's a pnpm-lock.yaml in the root directory
	rootHasPnpmLock := false
	if _, err := os.Stat(filepath.Join(cwd, "pnpm-lock.yaml")); err == nil {
		rootHasPnpmLock = true
	}

	fmt.Println("Starting to scan from:", cwd)
	fmt.Println("Will update npm/pnpm packages in directories with package.json")

	if *forceNpm {
		fmt.Println("Forcing npm for all directories")
	} else if *forcePnpm {
		fmt.Println("Forcing pnpm for all directories")
	} else if rootHasPnpmLock {
		fmt.Println("Found pnpm-lock.yaml in root directory, preferring pnpm")
	}

	if *useLatest {
		fmt.Println("Using --latest flag to update to latest versions")
	}

	err = filepath.Walk(cwd, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			fmt.Printf("Error accessing %s: %v\n", path, err)
			return nil // continue walking
		}

		// Skip node_modules directories
		if info.IsDir() && info.Name() == "node_modules" {
			return filepath.SkipDir
		}

		// Check for package.json
		if !info.IsDir() && info.Name() == "package.json" {
			dirPath := filepath.Dir(path)

			var cmdArgs []string

			// Determine which package manager to use
			if *forceNpm {
				// Force npm
				if *useLatest {
					cmdArgs = []string{"npm", "update", "--latest"}
				} else {
					cmdArgs = []string{"npm", "update"}
				}
			} else if *forcePnpm {
				// Force pnpm
				if *useLatest {
					cmdArgs = []string{"pnpm", "up", "-L"}
				} else {
					cmdArgs = []string{"pnpm", "up"}
				}
			} else if rootHasPnpmLock {
				// Root has pnpm-lock.yaml, prefer pnpm
				if *useLatest {
					cmdArgs = []string{"pnpm", "up", "-L"}
				} else {
					cmdArgs = []string{"pnpm", "up"}
				}
			} else {
				// Check for lock files to determine which package manager to use
				pnpmLockPath := filepath.Join(dirPath, "pnpm-lock.yaml")
				npmLockPath := filepath.Join(dirPath, "package-lock.json")

				if _, err := os.Stat(pnpmLockPath); err == nil {
					if *useLatest {
						cmdArgs = []string{"pnpm", "up", "-L"}
					} else {
						cmdArgs = []string{"pnpm", "up"}
					}
				} else if _, err := os.Stat(npmLockPath); err == nil {
					if *useLatest {
						cmdArgs = []string{"npm", "update", "--latest"}
					} else {
						cmdArgs = []string{"npm", "update"}
					}
				} else {
					// Default to npm if no lock file is found
					if *useLatest {
						cmdArgs = []string{"npm", "update", "--latest"}
					} else {
						cmdArgs = []string{"npm", "update"}
					}
				}
			}

			fmt.Printf("Running '%s' in %s\n", formatCommand(cmdArgs), dirPath)

			// Change to the directory
			currentDir, _ := os.Getwd()
			err = os.Chdir(dirPath)
			if err != nil {
				fmt.Printf("Error changing to directory %s: %v\n", dirPath, err)
				return nil
			}

			// Execute the command
			cmd := exec.Command(cmdArgs[0], cmdArgs[1:]...)
			cmd.Stdout = os.Stdout
			cmd.Stderr = os.Stderr
			err = cmd.Run()
			if err != nil {
				fmt.Printf("Error executing command in %s: %v\n", dirPath, err)
			}

			// Change back to the original directory
			err = os.Chdir(currentDir)
			if err != nil {
				fmt.Printf("Error changing back to original directory: %v\n", err)
				return err
			}
		}

		return nil
	})

	if err != nil {
		fmt.Printf("Error walking directory tree: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Update complete!")
}

func formatCommand(args []string) string {
	cmd := ""
	for i, arg := range args {
		if i > 0 {
			cmd += " "
		}
		cmd += arg
	}
	return cmd
}
