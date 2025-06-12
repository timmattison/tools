package main

import (
	"flag"
	"fmt"
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/charmbracelet/bubbles/spinner"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/go-git/go-git/v5"
	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/object"
)

type model struct {
	spinner        spinner.Model
	searching      bool
	paths          []string
	searchDone     bool
	quitting       bool
	dirsChecked    int
	reposFound     int
	currentPath    string
	lastUpdateTime time.Time
	startTime      time.Time
	searchTerm     string
	searchContents bool
	cancel         chan struct{} // Channel to signal cancellation
}

func (m model) Init() tea.Cmd {
	return tea.Batch(
		m.spinner.Tick,
	)
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		switch msg.String() {
		case "ctrl+c", "esc", "q":
			close(m.cancel) // Signal cancellation
			m.quitting = true
			return m, tea.Quit
		}
	case spinner.TickMsg:
		var cmd tea.Cmd
		m.spinner, cmd = m.spinner.Update(msg)
		return m, cmd
	case progressMsg:
		m.dirsChecked = msg.dirsChecked
		m.reposFound = msg.reposFound
		m.currentPath = msg.currentPath
		return m, nil
	case bool: // search completion
		m.searchDone = true
		m.quitting = true
		return m, tea.Quit
	}
	return m, nil
}

func (m model) View() string {
	if m.quitting {
		return "Goodbye!\n"
	}

	if m.searchDone {
		return ""
	}

	var output strings.Builder
	duration := time.Since(m.startTime).Round(time.Second)

	// Stats header
	output.WriteString(fmt.Sprintf("🔍 Searching for \"%s\" in commit ", m.searchTerm))
	if m.searchContents {
		output.WriteString("messages and contents\n")
	} else {
		output.WriteString("messages\n")
	}

	output.WriteString(strings.Repeat("─", 50) + "\n")

	// Define styles
	labelStyle := lipgloss.NewStyle().Width(25)

	// Helper function to create aligned label-value pairs
	writeStatLine := func(emoji, label, value string) {
		fullLabel := lipgloss.NewStyle().Render(emoji + " " + label + ":")
		paddedLabel := labelStyle.Render(fullLabel)
		output.WriteString(fmt.Sprintf("%s %s\n", paddedLabel, value))
	}

	// Write stats with consistent alignment
	writeStatLine("", "Directories scanned", fmt.Sprintf("%d", m.dirsChecked))
	writeStatLine("", "Repositories found", fmt.Sprintf("%d", m.reposFound))
	writeStatLine("", "Time elapsed", duration.String())

	// Calculate and show scan rate
	scanRate := float64(m.dirsChecked) / duration.Seconds()
	writeStatLine("", "Scan rate", fmt.Sprintf("%.1f dirs/sec", scanRate))

	// Current path with truncation if too long
	currentPath := m.currentPath
	if len(currentPath) > 38 {
		currentPath = "..." + currentPath[len(currentPath)-35:]
	}
	output.WriteString(strings.Repeat("─", 50) + "\n")
	output.WriteString(fmt.Sprintf("🔎 Current: %s\n", currentPath))

	// Spinner at the bottom
	output.WriteString(fmt.Sprintf("\n%s Searching...", m.spinner.View()))

	return output.String()
}

// Add a new message type for progress updates
type progressMsg struct {
	dirsChecked int
	reposFound  int
	currentPath string
}

type searchResult struct {
	repositories     map[string][]string // Map of repo path to commit info
	inaccessibleDirs []string
	foundCommits     bool
	absPaths         []string
	searchTerm       string
	searchContents   bool
}

func main() {
	var rootDir = flag.String("root", "", "root directory to start scanning from (overrides positional arguments)")
	var searchContents = flag.Bool("contents", false, "search in commit contents (diffs) in addition to commit messages")
	var ignoreFailures = flag.Bool("ignore-failures", false, "suppress output about directories that couldn't be accessed")
	var searchAllBranches = flag.Bool("all", false, "search all branches, not just the current branch")
	var help = flag.Bool("help", false, "show help message")
	var h = flag.Bool("h", false, "show help message")

	flag.Parse()

	// Check for help flags before starting bubbletea
	if *help || *h {
		flag.Usage()
		return
	}

	// Get the search term from the remaining arguments
	args := flag.Args()
	if len(args) < 1 {
		fmt.Println("Error: No search term provided")
		fmt.Println("Usage: git-diggin [options] <search-term>")
		flag.PrintDefaults()
		return
	}

	searchTerm := args[0]

	// Determine the search paths
	var paths []string
	if *rootDir != "" {
		paths = append(paths, *rootDir)
	} else if len(args) > 1 {
		paths = append(paths, args[1:]...)
	} else {
		paths = append(paths, ".")
	}

	// Initialize bubbletea model with spinner
	s := spinner.New()
	s.Spinner = spinner.Dot
	s.Style = s.Style.Foreground(s.Style.GetForeground())

	initialModel := model{
		spinner:        s,
		searching:      true,
		paths:          paths,
		searchDone:     false,
		quitting:       false,
		dirsChecked:    0,
		reposFound:     0,
		currentPath:    "",
		lastUpdateTime: time.Now(),
		startTime:      time.Now(),
		searchTerm:     searchTerm,
		searchContents: *searchContents,
		cancel:         make(chan struct{}),
	}

	p := tea.NewProgram(initialModel)

	// Channel for search results
	resultsChan := make(chan searchResult)

	// Run the search in a goroutine
	go func() {
		defer close(resultsChan) // Ensure channel gets closed
		var result searchResult
		result.repositories = make(map[string][]string)
		result.searchTerm = searchTerm
		result.searchContents = *searchContents

		var dirsChecked int32
		var reposFound int32

		lastUpdate := time.Now()

		// Function to send progress update if enough time has passed
		sendProgress := func(currentPath string) {
			select {
			case <-initialModel.cancel:
				return
			default:
				if time.Since(lastUpdate) > 33*time.Millisecond {
					p.Send(progressMsg{
						dirsChecked: int(atomic.LoadInt32(&dirsChecked)),
						reposFound:  int(atomic.LoadInt32(&reposFound)),
						currentPath: currentPath,
					})
					lastUpdate = time.Now()
				}
			}
		}

		var absPaths []string
		for _, path := range paths {
			absPath, err := filepath.Abs(path)
			if err != nil {
				if !*ignoreFailures {
					result.inaccessibleDirs = append(result.inaccessibleDirs,
						fmt.Sprintf("%s (could not resolve absolute path: %v)", path, err))
				}
				continue
			}

			resolvedPath, err := filepath.EvalSymlinks(absPath)
			if err != nil {
				if !*ignoreFailures {
					result.inaccessibleDirs = append(result.inaccessibleDirs,
						fmt.Sprintf("%s (could not resolve symlink: %v)", absPath, err))
				}
				continue
			}
			absPaths = append(absPaths, resolvedPath)
		}
		result.absPaths = absPaths

		unique := &sync.Map{}

		// Find all git repositories
		var wg sync.WaitGroup
		for _, searchPath := range absPaths {
			wg.Add(1)
			go func(path string) {
				defer wg.Done()
				err := scanPath(path, &result, &dirsChecked, &reposFound, unique,
					ignoreFailures, initialModel.cancel, sendProgress, searchTerm, *searchContents, *searchAllBranches)
				if err != nil && !*ignoreFailures {
					result.inaccessibleDirs = append(result.inaccessibleDirs,
						fmt.Sprintf("%s (walk error: %v)", path, err))
				}
			}(searchPath)
		}

		wg.Wait()

		// If no repositories were found, try to find the repository root using internal.GetRepoBase
		if atomic.LoadInt32(&reposFound) == 0 {
			// Try to find the repository root
			gitDir, err := internal.GetRepoBase()
			if err == nil {
				// Found a repository root
				repoPath := filepath.Dir(gitDir) // Get the directory containing .git
				atomic.AddInt32(&reposFound, 1)
				sendProgress(repoPath)

				// Process the repository
				processGitRepo(repoPath, &result, ignoreFailures, searchTerm, *searchContents, *searchAllBranches)
			}
		}

		// Only send results if we haven't cancelled
		select {
		case <-initialModel.cancel:
			return
		default:
			p.Send(true)
			resultsChan <- result
		}
	}()

	// Run the spinner program
	if _, err := p.Run(); err != nil {
		if !initialModel.quitting {
			log.Fatal("Error running program", "error", err)
		}
		return // Exit immediately if quitting
	}

	// Only process results if we're not quitting
	if !initialModel.quitting {
		// Get and process results
		select {
		case results, ok := <-resultsChan:
			if !ok {
				return // Channel was closed, exit gracefully
			}

			// Print results
			if len(results.inaccessibleDirs) > 0 && !*ignoreFailures {
				fmt.Println("⚠️  The following directories could not be fully accessed:")

				for _, dir := range results.inaccessibleDirs {
					fmt.Printf("  %s\n", dir)
				}

				fmt.Println()
			}

			if results.foundCommits {
				fmt.Printf("🔍 Found commits containing \"%s\"\n", results.searchTerm)
				if results.searchContents {
					fmt.Println("📄 Searched in commit messages and contents")
				} else {
					fmt.Println("📄 Searched in commit messages only")
				}
				fmt.Printf("📂 Search paths: %s\n", strings.Join(results.absPaths, ", "))
				if *searchAllBranches {
					fmt.Println("🔀 Searched across all branches")
				}
				fmt.Println()

				// Calculate total commits
				totalCommits := 0
				for _, commits := range results.repositories {
					totalCommits += len(commits)
				}

				fmt.Println("📊 Summary:")
				fmt.Printf("   • Found %d matching commits across %d repositories\n\n", totalCommits, len(results.repositories))

				// Sort repository paths for consistent output
				var sortedRepoPaths []string
				for workingDir := range results.repositories {
					sortedRepoPaths = append(sortedRepoPaths, workingDir)
				}
				sort.Strings(sortedRepoPaths)

				// Display results in sorted order
				for _, workingDir := range sortedRepoPaths {
					commits := results.repositories[workingDir]
					fmt.Printf("📁 %s - %d commits\n", workingDir, len(commits))
					for _, commit := range commits {
						fmt.Printf("      • %s\n", commit)
					}
					fmt.Println()
				}
			} else {
				fmt.Printf("😴 No commits found containing \"%s\"\n", results.searchTerm)
				if results.searchContents {
					fmt.Println("   • Searched in commit messages and contents")
				} else {
					fmt.Println("   • Searched in commit messages only")
				}
				fmt.Printf("   • Search paths: %s\n", strings.Join(results.absPaths, ", "))
			}

		case <-time.After(100 * time.Millisecond): // Add short timeout
			return // Exit if we don't get results quickly after quitting
		}
	}
}

var skipDirs = map[string]bool{
	"node_modules": true,
	"vendor":       true,
	".idea":        true,
	".vscode":      true,
	"dist":         true,
	"build":        true,
}

func scanPath(searchPath string, result *searchResult, dirsChecked *int32,
	reposFound *int32, unique *sync.Map, ignoreFailures *bool,
	cancel chan struct{}, sendProgress func(string),
	searchTerm string, searchContents bool, searchAllBranches bool) error {

	// Add check for directory existence before walking
	if _, err := os.Stat(searchPath); err != nil {
		if !*ignoreFailures {
			result.inaccessibleDirs = append(result.inaccessibleDirs,
				fmt.Sprintf("%s (access error: %v)", searchPath, err))
		}
		return nil // Return nil to continue with other paths
	}

	// Use filepath.WalkDir with a custom fs.WalkDirFunc that follows symlinks
	return filepath.WalkDir(searchPath, func(p string, d os.DirEntry, err error) error {
		select {
		case <-cancel:
			return filepath.SkipAll
		default:
			if err != nil {
				if !*ignoreFailures {
					result.inaccessibleDirs = append(result.inaccessibleDirs,
						fmt.Sprintf("%s (access error: %v)", p, err))
				}
				return filepath.SkipDir
			}

			// Check if this is a symlink to a directory
			if d.Type()&os.ModeSymlink != 0 {
				// Resolve the symlink
				realPath, err := filepath.EvalSymlinks(p)
				if err != nil {
					if !*ignoreFailures {
						result.inaccessibleDirs = append(result.inaccessibleDirs,
							fmt.Sprintf("%s (symlink resolution error: %v)", p, err))
					}
					return nil
				}

				// Check if it's a directory
				info, err := os.Stat(realPath)
				if err != nil {
					if !*ignoreFailures {
						result.inaccessibleDirs = append(result.inaccessibleDirs,
							fmt.Sprintf("%s (stat error after symlink resolution: %v)", realPath, err))
					}
					return nil
				}

				// If it's a directory, process it
				if info.IsDir() {
					// Process this directory as if we found it normally
					atomic.AddInt32(dirsChecked, 1)
					sendProgress(realPath)

					// Check if it's a git repository
					_, err := git.PlainOpen(realPath)
					if err == nil {
						// Store the working directory path
						unique.Store(realPath, true)
						atomic.AddInt32(reposFound, 1)
						sendProgress(realPath)

						// Process git repo
						processGitRepo(realPath, result, ignoreFailures, searchTerm, searchContents, searchAllBranches)
					}

					// Always recurse into symlinked directories
					err = scanPath(realPath, result, dirsChecked, reposFound, unique,
						ignoreFailures, cancel, sendProgress, searchTerm, searchContents, searchAllBranches)
					if err != nil && !*ignoreFailures {
						result.inaccessibleDirs = append(result.inaccessibleDirs,
							fmt.Sprintf("%s (walk error in symlinked dir: %v)", realPath, err))
					}
				}
				return nil
			}

			if !d.IsDir() {
				return nil
			}

			if skipDirs[d.Name()] {
				return filepath.SkipDir
			}

			atomic.AddInt32(dirsChecked, 1)
			sendProgress(p)

			// Check if it's a git repository
			_, err = git.PlainOpen(p)
			if err == nil {
				// Store the working directory path
				unique.Store(p, true)
				atomic.AddInt32(reposFound, 1)
				sendProgress(p)

				// Process git repo
				processGitRepo(p, result, ignoreFailures, searchTerm, searchContents, searchAllBranches)
			}

			return nil
		}
	})
}

// Extract git repository processing logic to a separate function
func processGitRepo(p string, result *searchResult, ignoreFailures *bool,
	searchTerm string, searchContents bool, searchAllBranches bool) {

	// Get commits using go-git
	repo, err := git.PlainOpen(p)
	if err != nil {
		if !*ignoreFailures {
			result.inaccessibleDirs = append(result.inaccessibleDirs,
				fmt.Sprintf("%s (error opening git repo: %v)", p, err))
		}
		return
	}

	// Function to process commits from a reference
	processRef := func(ref *plumbing.Reference) error {
		// Get the commit history
		commitIter, err := repo.Log(&git.LogOptions{
			From: ref.Hash(),
		})
		if err != nil {
			return err
		}
		defer commitIter.Close()

		var matchingCommits []string

		err = commitIter.ForEach(func(c *object.Commit) error {
			// Check if the commit message contains the search term
			if strings.Contains(strings.ToLower(c.Message), strings.ToLower(searchTerm)) {
				matchingCommits = append(matchingCommits, fmt.Sprintf("%s %s", c.Hash.String(), getFirstLine(c.Message)))
				return nil
			}

			// If we're not searching contents, skip to the next commit
			if !searchContents {
				return nil
			}

			// Get the commit's parent to compare changes
			if c.NumParents() == 0 {
				// This is the first commit, no parent to compare with
				return nil
			}

			// Get the first parent
			parent, err := c.Parent(0)
			if err != nil {
				// Skip if we can't get the parent
				return nil
			}

			// Get the patch (changes) between this commit and its parent
			patch, err := parent.Patch(c)
			if err != nil {
				// Skip if we can't get the patch
				return nil
			}

			// Convert the patch to a string and search in it
			patchString := patch.String()
			if strings.Contains(strings.ToLower(patchString), strings.ToLower(searchTerm)) {
				matchingCommits = append(matchingCommits, fmt.Sprintf("%s %s", c.Hash.String(), getFirstLine(c.Message)))
			}

			return nil
		})

		if err != nil {
			return err
		}

		if len(matchingCommits) > 0 {
			result.foundCommits = true
			result.repositories[p] = matchingCommits
		}

		return nil
	}

	if searchAllBranches {
		// Get all branches
		refs, err := repo.References()
		if err != nil {
			if !*ignoreFailures {
				result.inaccessibleDirs = append(result.inaccessibleDirs,
					fmt.Sprintf("%s (error getting references: %v)", p, err))
			}
			return
		}

		err = refs.ForEach(func(ref *plumbing.Reference) error {
			if ref.Name().IsBranch() {
				return processRef(ref)
			}
			return nil
		})
		if err != nil && !*ignoreFailures {
			result.inaccessibleDirs = append(result.inaccessibleDirs,
				fmt.Sprintf("%s (error processing branches: %v)", p, err))
		}
	} else {
		// Just use HEAD
		ref, err := repo.Head()
		if err != nil {
			if !*ignoreFailures {
				result.inaccessibleDirs = append(result.inaccessibleDirs,
					fmt.Sprintf("%s (error getting HEAD: %v)", p, err))
			}
			return
		}

		err = processRef(ref)
		if err != nil && !*ignoreFailures {
			result.inaccessibleDirs = append(result.inaccessibleDirs,
				fmt.Sprintf("%s (error processing HEAD: %v)", p, err))
		}
	}
}

// Function to get first line of a multi-line string
func getFirstLine(s string) string {
	if idx := strings.Index(s, "\n"); idx != -1 {
		return s[:idx]
	}
	return s
}
