package main

import (
	"flag"
	"fmt"
	"github.com/charmbracelet/log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

func getGitDir(path string) (string, error) {
	cmd := exec.Command("git", "-C", path, "rev-parse", "--git-dir")
	output, err := cmd.Output()
	if err != nil {
		return "", err
	}

	gitDir := strings.TrimSpace(string(output))

	// If it's not an absolute path, make it absolute relative to the input path
	if !filepath.IsAbs(gitDir) {
		gitDir = filepath.Join(path, gitDir)
	}

	return filepath.Clean(gitDir), nil
}

func main() {
	var durationFlag = flag.Duration("duration", 24*time.Hour, "how far back to look for commits (e.g. 24h, 168h for a week)")
	var ignoreFailures = flag.Bool("ignore-failures", false, "suppress output about directories that couldn't be accessed")
	var summaryOnly = flag.Bool("summary-only", false, "only show repository names and commit counts")

	flag.Parse()

	var paths []string
	args := flag.Args()

	if len(args) > 0 {
		paths = append(paths, args...)
	} else {
		paths = append(paths, ".")
	}

	// Convert all paths to absolute paths
	var absPaths []string
	for _, path := range paths {
		absPath, err := filepath.Abs(path)
		if err != nil {
			if !*ignoreFailures {
				log.Fatal("Could not resolve absolute path", "path", path, "error", err)
			}
			continue
		}
		absPaths = append(absPaths, absPath)
	}

	// Get user's email from git config
	email, err := exec.Command("git", "config", "user.email").Output()
	if err != nil {
		log.Fatal("Could not get git user.email", "error", err)
	}
	userEmail := strings.TrimSpace(string(email))

	// Calculate the time threshold
	threshold := time.Now().Add(-*durationFlag)

	// Create a map to store unique paths and their corresponding git directories
	unique := make(map[string]string) // Map of git dir -> working dir

	// Track directories we couldn't access
	var inaccessibleDirs []string

	// Find all git repositories in the specified paths
	for _, searchPath := range absPaths {
		err = filepath.WalkDir(searchPath, func(p string, d os.DirEntry, err error) error {
			if err != nil {
				if !*ignoreFailures {
					inaccessibleDirs = append(inaccessibleDirs, fmt.Sprintf("%s (access error: %v)", p, err))
				}
				return filepath.SkipDir
			}

			if !d.IsDir() {
				return nil
			}

			gitDir, err := getGitDir(p)
			if err != nil {
				// Only track if it's a permission error, not if it's just not a git repo
				if !*ignoreFailures && os.IsPermission(err) {
					inaccessibleDirs = append(inaccessibleDirs, fmt.Sprintf("%s (permission denied)", p))
				}
				return nil // Not a git repository or can't access, continue walking
			}

			// Store the working directory for this git directory
			unique[gitDir] = p
			return filepath.SkipDir // Skip subdirectories once we find a git repo
		})

		if err != nil && !*ignoreFailures {
			inaccessibleDirs = append(inaccessibleDirs, fmt.Sprintf("%s (walk error: %v)", searchPath, err))
		}
	}

	// Print inaccessible directories summary if any were found
	if len(inaccessibleDirs) > 0 && !*ignoreFailures {
		fmt.Printf("The following directories could not be fully accessed:\n")
		for _, dir := range inaccessibleDirs {
			fmt.Printf("  %s\n", dir)
		}
		fmt.Println()
	}

	foundCommits := false

	// Process each unique git repository
	for _, workingDir := range unique {
		// Get commits for the specified time period
		since := fmt.Sprintf("--since=%s", threshold.Format(time.RFC3339))
		cmd := exec.Command("git", "-C", workingDir, "log", "--author="+userEmail, since, "--format=%h %ad %s", "--date=iso")
		output, err := cmd.Output()
		if err != nil {
			if !*ignoreFailures {
				inaccessibleDirs = append(inaccessibleDirs, fmt.Sprintf("%s (error getting git log: %v)", workingDir, err))
			}
			continue
		}

		commits := strings.TrimSpace(string(output))
		if commits != "" {
			commitLines := strings.Split(commits, "\n")
			commitCount := len(commitLines)

			if !foundCommits {
				fmt.Printf("Commits in the past %v (starting time: %s) starting from %s\n\n", *durationFlag, threshold.Format(time.RFC3339), strings.Join(absPaths, ", "))
				foundCommits = true
			}

			fmt.Printf("Repository %s (%d commits)\n", workingDir, commitCount)
			if !*summaryOnly {
				commits = "  " + strings.ReplaceAll(commits, "\n", "\n  ")
				fmt.Println(commits)
				fmt.Println()
			}
		}
	}

	if !foundCommits {
		fmt.Printf("No commits in the past %v (starting time: %s) starting from %s\n", *durationFlag, threshold.Format(time.RFC3339), strings.Join(absPaths, ", "))
	}
}
