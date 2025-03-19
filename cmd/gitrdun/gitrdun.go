package main

import (
	"flag"
	"fmt"
	"github.com/charmbracelet/log"
	"github.com/sho0pi/naturaltime"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
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
	"github.com/timmattison/tools/internal"
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
	startTime      time.Time     // When the search started
	thresholdTime  time.Time     // Start time for commit search
	endTime        time.Time     // End time for commit search (if specified)
	hasEndTime     bool          // Whether an end time was specified
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
	thresholdTimeStr := m.thresholdTime.Format("Monday, January 2, 2006 at 3:04 PM")

	// Check if we have an end time in the model
	if m.hasEndTime {
		endTimeStr := m.endTime.Format("Monday, January 2, 2006 at 3:04 PM")
		output.WriteString(fmt.Sprintf("🔍 Searching for commits between %s and %s\n", thresholdTimeStr, endTimeStr))
	} else {
		output.WriteString(fmt.Sprintf("🔍 Searching for commits since %s\n", thresholdTimeStr))
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

type gitOpStats struct {
	count    int64
	duration time.Duration
	sync.Mutex
}

type gitStats struct {
	getGitDir gitOpStats
	getLog    gitOpStats
	getEmail  gitOpStats
}

func (s *gitOpStats) record(duration time.Duration) {
	s.Lock()
	defer s.Unlock()
	s.count++
	s.duration += duration
}

func (s *gitOpStats) average() time.Duration {
	s.Lock()
	defer s.Unlock()
	if s.count == 0 {
		return 0
	}
	return time.Duration(int64(s.duration) / s.count)
}

func getGitDir(path string, stats *gitOpStats) (string, error) {
	start := time.Now()
	defer func() {
		stats.record(time.Since(start))
	}()

	// Quick check for .git directory first
	gitPath := filepath.Join(path, ".git")
	if info, err := os.Stat(gitPath); err == nil && info.IsDir() {
		return gitPath, nil
	}

	// Try to open as git repository using go-git
	_, err := git.PlainOpen(path)
	if err != nil {
		return "", err
	}

	// If we can open it, return the .git path (go-git handles gitdir indirection internally)
	return gitPath, nil
}

type searchResult struct {
	repositories       map[string][]string
	inaccessibleDirs   []string
	foundCommits       bool
	absPaths           []string
	threshold          time.Time
	stats              gitStats
	fullCommitMessages map[string]map[string]string // Map of repo path to commit hash to full message
}

func main() {
	var startStr = flag.String("start", "24h", "how far back to start looking for commits (e.g. 24h, 7d, 2w, 'monday', 'last month', 'february', etc.)")
	var endStr = flag.String("end", "", "when to stop looking for commits (e.g. '2023-12-31', 'yesterday', 'last month', etc.)")
	var ignoreFailures = flag.Bool("ignore-failures", false, "suppress output about directories that couldn't be accessed")
	var summaryOnly = flag.Bool("summary-only", false, "only show repository names and commit counts")
	var findNested = flag.Bool("find-nested", false, "look for nested git repositories inside other git repositories")
	var showStats = flag.Bool("stats", false, "show git operation statistics")
	var searchAllBranches = flag.Bool("all", false, "search all branches, not just the current branch")
	var useOllama = flag.Bool("ollama", false, "use Ollama to generate summaries of work done in each repository")
	var metaOllama = flag.Bool("meta-ollama", false, "generate a meta-summary across all repositories (implies --ollama)")
	var ollamaModel = flag.String("ollama-model", "llama3.3", "Ollama model to use for summaries")
	var ollamaURL = flag.String("ollama-url", "http://localhost:11434", "URL for Ollama API")
	var rootDir = flag.String("root", "", "root directory to start scanning from (overrides positional arguments)")
	var outputFile = flag.String("output", "", "file to write results to (in addition to stdout)")
	var help = flag.Bool("help", false, "show help message")
	var h = flag.Bool("h", false, "show help message")
	var filterByUser = flag.Bool("filter-user", true, "only show commits authored by the current git user")

	flag.Parse()

	// Parse the start string
	startDuration, err := parseDuration(*startStr)
	if err != nil {
		log.Fatal("Invalid start format", "error", err)
	}

	// Calculate the start time (how far back to look)
	thresholdTime := time.Now().Add(-startDuration)

	// Parse the end string if provided
	var endTime time.Time
	var hasEndTime bool
	if *endStr != "" {
		end, err := parseTimeString(*endStr)
		if err != nil {
			log.Fatal("Invalid end format", "error", err)
		}
		endTime = end
		hasEndTime = true
	}

	// Check for help flags before starting bubbletea
	if *help || *h {
		flag.Usage()
		return
	}

	// If meta-ollama is set, enable ollama as well
	if *metaOllama {
		*useOllama = true
	}

	var paths []string
	args := flag.Args()

	// Use rootDir if specified, otherwise use args or default to current directory
	if *rootDir != "" {
		paths = append(paths, *rootDir)
	} else if len(args) > 0 {
		paths = append(paths, args...)
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
		thresholdTime:  thresholdTime, // Use the calculated threshold time
		endTime:        endTime,
		hasEndTime:     hasEndTime,
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

		// Get git email using go-git if possible, fallback to command
		userEmail := ""
		start := time.Now()

		// Try to get from global config first
		homeDir, err := os.UserHomeDir()
		if err == nil {
			// Try to find a git config in the home directory
			configPath := filepath.Join(homeDir, ".gitconfig")
			if configBytes, err := os.ReadFile(configPath); err == nil {
				configContent := string(configBytes)
				for _, line := range strings.Split(configContent, "\n") {
					line = strings.TrimSpace(line)
					if strings.HasPrefix(line, "email = ") {
						userEmail = strings.TrimPrefix(line, "email = ")
						break
					}
				}
			}
		}

		// Fallback to git command if needed
		if userEmail == "" {
			email, err := exec.Command("git", "config", "user.email").Output()
			if err != nil {
				log.Fatal("Could not get git user.email", "error", err)
			}
			userEmail = strings.TrimSpace(string(email))
		}

		result.stats.getEmail.record(time.Since(start))

		threshold := time.Now().Add(-startDuration)
		result.threshold = threshold

		unique := &sync.Map{}

		// Find all git repositories
		var wg sync.WaitGroup
		for _, searchPath := range absPaths {
			wg.Add(1)
			go func(path string) {
				defer wg.Done()
				err := scanPath(path, &result, &dirsChecked, &reposFound, unique,
					ignoreFailures, findNested, initialModel.cancel, sendProgress, userEmail, threshold, searchAllBranches, filterByUser)
				if err != nil && !*ignoreFailures {
					result.inaccessibleDirs = append(result.inaccessibleDirs,
						fmt.Sprintf("%s (walk error: %v)", path, err))
				}
			}(searchPath)
		}

		wg.Wait()

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

			// Create a buffer to store output for file writing
			var outputBuffer strings.Builder

			// Helper function to write to both stdout and buffer
			writeOutput := func(format string, a ...interface{}) {
				msg := fmt.Sprintf(format, a...)
				fmt.Print(msg)
				outputBuffer.WriteString(msg)
			}

			// Print results
			if len(results.inaccessibleDirs) > 0 && !*ignoreFailures {
				writeOutput("⚠️  The following directories could not be fully accessed:\n")

				for _, dir := range results.inaccessibleDirs {
					writeOutput("  %s\n", dir)
				}

				writeOutput("\n")
			}

			if results.foundCommits {
				writeOutput("🔍 Found commits\n")
				writeOutput("📅 Start date: %s\n", results.threshold.Format("Monday, January 2, 2006 at 3:04 PM"))
				if *endStr != "" {
					writeOutput("📅 End date: %s\n", endTime.Format("Monday, January 2, 2006 at 3:04 PM"))
				}
				writeOutput("📂 Search paths: %s\n", strings.Join(results.absPaths, ", "))
				if *searchAllBranches {
					writeOutput("🔀 Searching across all branches\n")
				}
				writeOutput("\n")

				// Calculate total commits
				totalCommits := 0

				for _, commits := range results.repositories {
					totalCommits += len(commits)
				}

				writeOutput("📊 Summary:\n")
				writeOutput("   • Found %d commits across %d repositories\n\n", totalCommits, len(results.repositories))

				// Sort repository paths for consistent output
				var sortedRepoPaths []string
				for workingDir := range results.repositories {
					sortedRepoPaths = append(sortedRepoPaths, workingDir)
				}
				sort.Strings(sortedRepoPaths)

				// For meta-ollama, collect all summaries
				var allSummaries []string
				var repoSummaries = make(map[string]string)

				// Display results in sorted order
				for _, workingDir := range sortedRepoPaths {
					commits := results.repositories[workingDir]
					writeOutput("📁 %s - %d commits\n", workingDir, len(commits))

					if *useOllama {
						// First show the commits as we did before
						if !*summaryOnly {
							for _, commit := range commits {
								writeOutput("      • %s\n", commit)
							}
						}

						// Then show the Ollama summary with repository name and model
						repoName := filepath.Base(workingDir)
						writeOutput("\n🤖 Generating summary for %s with Ollama (%s)...\n", repoName, *ollamaModel)
						summary, err := internal.GenerateOllamaSummary(workingDir, commits, *ollamaURL, *ollamaModel)
						if err != nil {
							writeOutput("⚠️  Error generating summary: %v\n", err)
						} else {
							writeOutput("📝 Summary for %s (%s): \n%s\n\n", repoName, *ollamaModel, summary)
							// Store summary for meta-summary if needed
							if *metaOllama {
								repoSummaries[workingDir] = summary
								allSummaries = append(allSummaries, fmt.Sprintf("Repository: %s\n%s", workingDir, summary))
							}
						}
					} else if !*summaryOnly {
						for _, commit := range commits {
							writeOutput("      • %s\n", commit)
						}
						writeOutput("\n")
					}
				}

				// Generate meta-summary if requested
				if *metaOllama && len(allSummaries) > 0 {
					writeOutput("\n🔍 Generating meta-summary of all work with Ollama (%s)...\n", *ollamaModel)
					metaSummary, err := internal.GenerateMetaSummary(allSummaries, *ollamaURL, *ollamaModel, startDuration)
					if err != nil {
						writeOutput("⚠️  Error generating meta-summary: %v\n", err)
					} else {
						writeOutput("\n📊 Meta-Summary of All Work (%s):\n%s\n", *ollamaModel, metaSummary)
					}
				}
			} else {
				writeOutput("😴 No commits found\n")
				writeOutput("   • Start date: %s\n", results.threshold.Format("Monday, January 2, 2006 at 3:04 PM"))
				if *endStr != "" {
					writeOutput("   • End date: %s\n", endTime.Format("Monday, January 2, 2006 at 3:04 PM"))
				}
				writeOutput("   • Search paths: %s\n", strings.Join(results.absPaths, ", "))
			}

			// Only show stats if the stats flag is set
			if *showStats {
				writeOutput("\n🔍 Git Operation Stats:\n")
				writeOutput("   • getGitDir: %d calls, avg %v per call\n",
					results.stats.getGitDir.count,
					results.stats.getGitDir.average().Round(time.Microsecond))
				writeOutput("   • git log: %d calls, avg %v per call\n",
					results.stats.getLog.count,
					results.stats.getLog.average().Round(time.Microsecond))
				writeOutput("   • git config: %d calls, avg %v per call\n",
					results.stats.getEmail.count,
					results.stats.getEmail.average().Round(time.Microsecond))
				writeOutput("\n")
			}

			// Write to file if output file is specified
			if *outputFile != "" {
				err := os.WriteFile(*outputFile, []byte(outputBuffer.String()), 0644)
				if err != nil {
					fmt.Printf("⚠️  Error writing to output file: %v\n", err)
				} else {
					fmt.Printf("📝 Results written to %s\n", *outputFile)
				}
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
	findNested *bool, cancel chan struct{}, sendProgress func(string),
	userEmail string, threshold time.Time, searchAllBranches *bool, filterByUser *bool) error {

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

					gitDir, err := getGitDir(realPath, &result.stats.getGitDir)
					if err == nil {
						// Store the working directory path
						unique.Store(gitDir, realPath)
						atomic.AddInt32(reposFound, 1)
						sendProgress(realPath)

						// Process git repo
						processGitRepo(realPath, result, ignoreFailures, searchAllBranches, threshold, &result.stats, userEmail, filterByUser)
					}

					// Recursively scan this directory if needed
					if *findNested {
						err = scanPath(realPath, result, dirsChecked, reposFound, unique,
							ignoreFailures, findNested, cancel, sendProgress, userEmail, threshold, searchAllBranches, filterByUser)
						if err != nil && !*ignoreFailures {
							result.inaccessibleDirs = append(result.inaccessibleDirs,
								fmt.Sprintf("%s (walk error in symlinked dir: %v)", realPath, err))
						}
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

			gitDir, err := getGitDir(p, &result.stats.getGitDir)

			if err != nil {
				if !*ignoreFailures && os.IsPermission(err) {
					result.inaccessibleDirs = append(result.inaccessibleDirs,
						fmt.Sprintf("%s (permission denied)", p))
				}
				return nil
			}

			// Store the working directory path
			unique.Store(gitDir, p)
			atomic.AddInt32(reposFound, 1)
			sendProgress(p)

			// Process git repo
			processGitRepo(p, result, ignoreFailures, searchAllBranches, threshold, &result.stats, userEmail, filterByUser)

			// Skip subdirectories unless --find-nested is set
			if !*findNested {
				return filepath.SkipDir
			}

			return nil
		}
	})
}

// Extract git repository processing logic to a separate function
func processGitRepo(p string, result *searchResult, ignoreFailures *bool, searchAllBranches *bool,
	threshold time.Time, stats *gitStats, userEmail string, filterByUser *bool) {

	// Get commits using go-git
	start := time.Now()
	repo, err := git.PlainOpen(p)
	if err != nil {
		if !*ignoreFailures {
			result.inaccessibleDirs = append(result.inaccessibleDirs,
				fmt.Sprintf("%s (error opening git repo: %v)", p, err))
		}
		return
	}

	// Get the HEAD reference
	var commits []string

	// Function to process commits from a reference
	processRef := func(ref *plumbing.Reference) error {
		// Get the commit history
		commitIter, err := repo.Log(&git.LogOptions{
			From:  ref.Hash(),
			Since: &threshold,
		})
		if err != nil {
			return err
		}
		defer commitIter.Close()

		// Initialize map for full commit messages if needed
		if result.fullCommitMessages == nil {
			result.fullCommitMessages = make(map[string]map[string]string)
		}

		if _, ok := result.fullCommitMessages[p]; !ok {
			result.fullCommitMessages[p] = make(map[string]string)
		}

		return commitIter.ForEach(func(c *object.Commit) error {
			// Only include commits from the user if filtering is enabled
			if !*filterByUser || c.Author.Email == userEmail {
				// Store the full commit message
				result.fullCommitMessages[p][c.Hash.String()] = c.Message

				// Add the commit to our list
				commits = append(commits, fmt.Sprintf("%s %s", c.Hash.String(), getFirstLine(c.Message)))
			}
			return nil
		})
	}

	if *searchAllBranches {
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

	stats.getLog.record(time.Since(start))

	if len(commits) > 0 {
		result.foundCommits = true
		result.repositories[p] = commits
	}
}

// The generateOllamaSummary and generateMetaSummary functions have been removed
// as they are now in the internal package

// Helper function to determine if a file should be skipped
func shouldSkipFile(filename string) bool {
	// Skip binary files and other non-text formats
	ext := strings.ToLower(filepath.Ext(filename))
	skipExts := map[string]bool{
		".png": true, ".jpg": true, ".jpeg": true, ".gif": true,
		".pdf": true, ".zip": true, ".tar": true, ".gz": true,
		".bin": true, ".exe": true, ".dll": true, ".so": true,
		".mp3": true, ".mp4": true, ".avi": true, ".mov": true,
	}

	// Skip files that are likely to be binary or too large
	if skipExts[ext] {
		return true
	}

	// Skip files with paths containing certain directories
	skipDirs := []string{
		"node_modules/", "vendor/", "dist/", "build/",
		".git/", ".idea/", ".vscode/",
	}

	for _, dir := range skipDirs {
		if strings.Contains(filename, dir) {
			return true
		}
	}

	return false
}

// Function to get first line of a multi-line string
func getFirstLine(s string) string {
	if idx := strings.Index(s, "\n"); idx != -1 {
		return s[:idx]
	}
	return s
}

// The generateMetaSummary function has been removed
// as it is now in the internal package

// parseDuration extends the standard time.ParseDuration to support days (d) and weeks (w)
func parseDuration(durationStr string) (time.Duration, error) {
	// Check for day format (e.g., "6d")
	if strings.HasSuffix(durationStr, "d") {
		days, err := strconv.Atoi(strings.TrimSuffix(durationStr, "d"))
		if err == nil {
			return time.Duration(days) * 24 * time.Hour, nil
		}
	}

	// Check for week format (e.g., "2w")
	if strings.HasSuffix(durationStr, "w") {
		weeks, err := strconv.Atoi(strings.TrimSuffix(durationStr, "w"))
		if err == nil {
			return time.Duration(weeks) * 7 * 24 * time.Hour, nil
		}
	}

	// Fall back to standard duration parsing for hours, minutes, etc.
	var duration time.Duration
	var err error

	if duration, err = time.ParseDuration(durationStr); err == nil {
		return duration, nil
	}

	// Try parsing with naturaltime as a last ditch effort
	var parser *naturaltime.Parser

	if parser, err = naturaltime.New(); err != nil {
		return 0, err
	}

	now := time.Now()

	var date *time.Time

	if date, err = parser.ParseDate(durationStr, now); err != nil {
		return 0, err
	}

	if date == nil {
		return 0, fmt.Errorf("could not parse date")
	}

	// Calculate the duration from now to the parsed date
	return date.Sub(now).Abs(), nil
}

// parseTimeString parses a string into a time.Time
func parseTimeString(timeStr string) (time.Time, error) {
	// Try parsing with naturaltime
	parser, err := naturaltime.New()
	if err != nil {
		return time.Time{}, err
	}

	now := time.Now()
	date, err := parser.ParseDate(timeStr, now)
	if err == nil && date != nil {
		return *date, nil
	}

	// Try standard date formats
	formats := []string{
		"2006-01-02",
		"2006-01-02T15:04:05",
		"2006-01-02 15:04:05",
		"2006-01-02T15:04:05Z07:00",
	}

	for _, format := range formats {
		if t, err := time.Parse(format, timeStr); err == nil {
			return t, nil
		}
	}

	return time.Time{}, fmt.Errorf("could not parse date: %s", timeStr)
}
