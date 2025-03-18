package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/charmbracelet/bubbles/spinner"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
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
	startTime      time.Time     // New field to track duration
	cancel         chan struct{} // Add this field
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
	if m.searchDone {
		return "\033[2K\r"
	}

	var output strings.Builder
	duration := time.Since(m.startTime).Round(time.Second)

	// Stats header
	startTimeStr := m.startTime.Format("2006-01-02 15:04:05")
	output.WriteString(fmt.Sprintf("🔍 Searching for commits since %s\n", startTimeStr))
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

	// Fall back to git command if .git directory isn't found
	cmd := exec.Command("git", "-C", path, "rev-parse", "--git-dir")
	output, err := cmd.Output()
	if err != nil {
		return "", err
	}

	gitDir := strings.TrimSpace(string(output))
	if !filepath.IsAbs(gitDir) {
		gitDir = filepath.Join(path, gitDir)
	}

	return filepath.Clean(gitDir), nil
}

type searchResult struct {
	inaccessibleDirs []string
	repositories     map[string][]string // workingDir -> commits
	foundCommits     bool
	threshold        time.Time
	absPaths         []string
	stats            gitStats
}

func main() {
	var durationFlag = flag.Duration("duration", 24*time.Hour, "how far back to look for commits (e.g. 24h, 168h for a week)")
	var ignoreFailures = flag.Bool("ignore-failures", false, "suppress output about directories that couldn't be accessed")
	var summaryOnly = flag.Bool("summary-only", false, "only show repository names and commit counts")
	var findNested = flag.Bool("find-nested", false, "look for nested git repositories inside other git repositories")
	var showStats = flag.Bool("stats", false, "show git operation statistics")
	var searchAllBranches = flag.Bool("all", false, "search all branches, not just the current branch")
	var useOllama = flag.Bool("ollama", false, "use Ollama to generate summaries of work done in each repository")
	var ollamaModel = flag.String("ollama-model", "llama3.3", "Ollama model to use for summaries")
	var ollamaURL = flag.String("ollama-url", "http://localhost:11434", "URL for Ollama API")
	var help = flag.Bool("help", false, "show help message")
	var h = flag.Bool("h", false, "show help message")

	flag.Parse()

	// Check for help flags before starting bubbletea
	if *help || *h {
		flag.Usage()
		return
	}

	var paths []string
	args := flag.Args()

	if len(args) > 0 {
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
		startTime:      time.Now(),          // Initialize start time
		cancel:         make(chan struct{}), // Initialize the cancel channel
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

		// Get git email
		start := time.Now()
		email, err := exec.Command("git", "config", "user.email").Output()
		if err != nil {
			log.Fatal("Could not get git user.email", "error", err)
		}
		result.stats.getEmail.record(time.Since(start))
		userEmail := strings.TrimSpace(string(email))

		threshold := time.Now().Add(-*durationFlag)
		result.threshold = threshold

		unique := &sync.Map{}

		// Find all git repositories
		var wg sync.WaitGroup
		for _, searchPath := range absPaths {
			wg.Add(1)
			go func(path string) {
				defer wg.Done()
				err := scanPath(path, &result, &dirsChecked, &reposFound, unique,
					ignoreFailures, findNested, initialModel.cancel, sendProgress)
				if err != nil && !*ignoreFailures {
					result.inaccessibleDirs = append(result.inaccessibleDirs,
						fmt.Sprintf("%s (walk error: %v)", path, err))
				}
			}(searchPath)
		}

		wg.Wait()

		// Get commits for each repository
		unique.Range(func(key, value interface{}) bool {
			workingDir := value.(string)

			// Keep the since parameter for initial filtering but we'll do additional filtering
			since := fmt.Sprintf("--since=%s", threshold.Format(time.RFC3339))

			start := time.Now()
			// Use a more detailed format to include timestamp for filtering
			gitArgs := []string{"-C", workingDir, "log", "--author=" + userEmail, since, "--format=%h %ad %s", "--date=iso8601-strict", "--date-order"}

			if *searchAllBranches {
				gitArgs = append(gitArgs, "--all")
			}

			cmd := exec.Command("git", gitArgs...)
			output, err := cmd.Output()
			result.stats.getLog.record(time.Since(start))

			if err != nil {
				if !*ignoreFailures {
					result.inaccessibleDirs = append(result.inaccessibleDirs,
						fmt.Sprintf("%s (error getting git log: %v)", workingDir, err))
				}
				return true
			}

			commits := strings.TrimSpace(string(output))

			if commits != "" {
				// Split commits and filter by date
				allCommits := strings.Split(commits, "\n")
				filteredCommits := []string{}

				for _, commit := range allCommits {
					// Extract date from commit line (format: hash date message)
					parts := strings.SplitN(commit, " ", 3)
					if len(parts) >= 2 {
						// Parse the ISO8601 date
						commitDate, err := time.Parse("2006-01-02T15:04:05-07:00", parts[1])
						if err == nil && !commitDate.Before(threshold) {
							filteredCommits = append(filteredCommits, commit)
						}
					}
				}

				if len(filteredCommits) > 0 {
					result.foundCommits = true
					result.repositories[workingDir] = filteredCommits
				}
			}

			return true
		})

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
				fmt.Printf("⚠️  The following directories could not be fully accessed:\n")

				for _, dir := range results.inaccessibleDirs {
					fmt.Printf("  %s\n", dir)
				}

				fmt.Println()
			}

			if results.foundCommits {
				fmt.Printf("🔍 Found commits from the last %v\n", *durationFlag)
				fmt.Printf("📅 Starting from: %s\n", results.threshold.Format(time.RFC3339))
				fmt.Printf("📂 Search paths: %s\n", strings.Join(results.absPaths, ", "))
				if *searchAllBranches {
					fmt.Printf("🔀 Searching across all branches\n")
				}
				fmt.Println()

				// Calculate total commits
				totalCommits := 0

				for _, commits := range results.repositories {
					totalCommits += len(commits)
				}

				fmt.Printf("📊 Summary:\n")
				fmt.Printf("   • Found %d commits across %d repositories\n\n", totalCommits, len(results.repositories))

				for workingDir, commits := range results.repositories {
					fmt.Printf("📁 %s - %d commits\n", workingDir, len(commits))

					if *useOllama {
						// First show the commits as we did before
						if !*summaryOnly {
							for _, commit := range commits {
								fmt.Printf("      • %s\n", commit)
							}
						}

						// Then show the Ollama summary
						fmt.Printf("\n🤖 Generating summary with Ollama...\n")
						summary, err := generateOllamaSummary(workingDir, commits, *ollamaURL, *ollamaModel)
						if err != nil {
							fmt.Printf("⚠️  Error generating summary: %v\n", err)
						} else {
							fmt.Printf("📝 Summary: \n%s\n\n", summary)
						}
					} else if !*summaryOnly {
						for _, commit := range commits {
							fmt.Printf("      • %s\n", commit)
						}
						fmt.Println()
					}
				}
			} else {
				fmt.Printf("😴 No commits found\n")
				fmt.Printf("   • Time period: last %v\n", *durationFlag)
				fmt.Printf("   • Starting from: %s\n", results.threshold.Format(time.RFC3339))
				fmt.Printf("   • Search paths: %s\n", strings.Join(results.absPaths, ", "))
			}

			// Only show stats if the stats flag is set
			if *showStats {
				fmt.Printf("\n🔍 Git Operation Stats:\n")
				fmt.Printf("   • getGitDir: %d calls, avg %v per call\n",
					results.stats.getGitDir.count,
					results.stats.getGitDir.average().Round(time.Microsecond))
				fmt.Printf("   • git log: %d calls, avg %v per call\n",
					results.stats.getLog.count,
					results.stats.getLog.average().Round(time.Microsecond))
				fmt.Printf("   • git config: %d calls, avg %v per call\n",
					results.stats.getEmail.count,
					results.stats.getEmail.average().Round(time.Microsecond))
				fmt.Println()
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
	findNested *bool, cancel chan struct{}, sendProgress func(string)) error {

	// Add check for directory existence before walking
	if _, err := os.Stat(searchPath); err != nil {
		if !*ignoreFailures {
			result.inaccessibleDirs = append(result.inaccessibleDirs,
				fmt.Sprintf("%s (access error: %v)", searchPath, err))
		}
		return nil // Return nil to continue with other paths
	}

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

			unique.Store(gitDir, p)
			atomic.AddInt32(reposFound, 1)
			sendProgress(p)

			// Skip subdirectories unless --find-nested is set
			if !*findNested {
				return filepath.SkipDir
			}

			return nil
		}
	})
}

func generateOllamaSummary(workingDir string, commits []string, ollamaURL, ollamaModel string) (string, error) {
	// Gather repository context
	repoContext := "Repository: " + workingDir + "\n\n"

	// Try to read README.md if it exists
	readmeContent := ""
	readmePath := filepath.Join(workingDir, "README.md")
	if readmeBytes, err := os.ReadFile(readmePath); err == nil {
		readmeContent = "README.md content:\n" + string(readmeBytes) + "\n\n"
	} else {
		// If no README.md, try to get repository description
		cmd := exec.Command("git", "-C", workingDir, "remote", "-v")
		if remoteOutput, err := cmd.Output(); err == nil {
			repoContext += "Git remotes:\n" + string(remoteOutput) + "\n"
		}

		// Get repository structure overview
		cmd = exec.Command("find", workingDir, "-type", "f", "-name", "*.go", "-o", "-name", "*.js", "-o", "-name", "*.py", "-o", "-name", "*.java", "-o", "-name", "*.ts", "|", "head", "-10")
		if filesOutput, err := cmd.Output(); err == nil && len(filesOutput) > 0 {
			repoContext += "Key files (sample):\n" + string(filesOutput) + "\n"
		}
	}

	// Get detailed commit information with full file contents
	detailedCommits := ""
	var totalSize int
	maxSize := 20000 // Limit total size to avoid overwhelming the model

	for _, commitLine := range commits {
		parts := strings.SplitN(commitLine, " ", 2)
		if len(parts) > 0 {
			commitHash := parts[0]

			// Get commit message and stats
			cmd := exec.Command("git", "-C", workingDir, "show", "--stat", commitHash)
			if output, err := cmd.Output(); err == nil {
				commitInfo := string(output)
				detailedCommits += "COMMIT: " + commitInfo + "\n"
				totalSize += len(commitInfo)

				// Get list of changed files in this commit
				cmd = exec.Command("git", "-C", workingDir, "diff-tree", "--no-commit-id", "--name-only", "-r", commitHash)
				if filesOutput, err := cmd.Output(); err == nil {
					changedFiles := strings.Split(strings.TrimSpace(string(filesOutput)), "\n")

					// For each changed file, get its content after the commit
					for _, file := range changedFiles {
						if totalSize >= maxSize {
							detailedCommits += "... (truncated due to size limits)\n"
							break
						}

						// Skip binary files, large files, and certain extensions
						if shouldSkipFile(file) {
							continue
						}

						// Get file content at this commit
						cmd = exec.Command("git", "-C", workingDir, "show", commitHash+":"+file)
						if fileContent, err := cmd.Output(); err == nil {
							content := string(fileContent)

							// Only include if not too large
							if len(content) > 5000 {
								fileInfo := fmt.Sprintf("FILE %s: (truncated, too large)\n", file)
								detailedCommits += fileInfo
								totalSize += len(fileInfo)
							} else {
								fileInfo := fmt.Sprintf("FILE %s:\n```\n%s\n```\n\n", file, content)
								detailedCommits += fileInfo
								totalSize += len(fileInfo)
							}
						}
					}
				}

				detailedCommits += "---\n"
			}

			// Check if we've exceeded our size limit
			if totalSize >= maxSize {
				detailedCommits += "...\n(truncated for brevity, reached size limit)\n"
				break
			}
		}
	}

	// Format the prompt for Ollama
	prompt := fmt.Sprintf(`Please summarize the work done in this repository based on these recent commits.

%s
%s
Recent commits:
%s

Detailed commit information with file contents:
%s

Please provide a concise summary of what was worked on in this repository. Focus on:
1. What features or changes were implemented
2. Any bug fixes or improvements
3. The overall purpose of the changes
4. Technical details that would be relevant to a developer

Use the file contents to understand the code changes in depth.`,
		repoContext,
		readmeContent,
		strings.Join(commits, "\n"),
		detailedCommits,
	)

	// Prepare the request to Ollama
	requestBody, err := json.Marshal(map[string]interface{}{
		"model":  ollamaModel,
		"prompt": prompt,
	})
	if err != nil {
		return "", fmt.Errorf("error creating request: %w", err)
	}

	// Send the request to Ollama
	resp, err := http.Post(ollamaURL+"/api/generate", "application/json", bytes.NewBuffer(requestBody))
	if err != nil {
		return "", fmt.Errorf("error calling Ollama API: %w", err)
	}
	defer resp.Body.Close()

	// Process the streaming NDJSON response
	scanner := bufio.NewScanner(resp.Body)
	var fullResponse strings.Builder

	for scanner.Scan() {
		line := scanner.Text()
		var responseChunk struct {
			Model     string `json:"model"`
			CreatedAt string `json:"created_at"`
			Response  string `json:"response"`
			Done      bool   `json:"done"`
		}

		if err := json.Unmarshal([]byte(line), &responseChunk); err != nil {
			return "", fmt.Errorf("error parsing response chunk: %w", err)
		}

		fullResponse.WriteString(responseChunk.Response)

		if responseChunk.Done {
			break
		}
	}

	if err := scanner.Err(); err != nil {
		return "", fmt.Errorf("error reading response stream: %w", err)
	}

	return fullResponse.String(), nil
}

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
