package internal

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

// GenerateOllamaSummary generates a summary of git commits using Ollama
func GenerateOllamaSummary(workingDir string, commits []string, fullCommitMessages map[string]string, ollamaURL, ollamaModel string) (string, error) {
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

					// Only process a few files to avoid overwhelming the model
					maxFiles := 5
					if len(changedFiles) > maxFiles {
						changedFiles = changedFiles[:maxFiles]
					}

					for _, file := range changedFiles {
						if file == "" {
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

	return callOllama(ollamaURL, ollamaModel, prompt, true)
}

// GenerateMetaSummary generates a meta-summary across multiple repositories
func GenerateMetaSummary(summaries []string, ollamaURL, ollamaModel string, duration time.Duration) (string, error) {
	// Format the prompt for Ollama
	prompt := fmt.Sprintf(`Please provide a comprehensive overview of all work done across multiple repositories over the past %v.

Here are summaries from each repository:

%s

Focus on the big picture rather than repeating details from individual repositories.`,
		duration,
		strings.Join(summaries, "\n\n---\n\n"),
	)

	return callOllama(ollamaURL, ollamaModel, prompt, false)
}

// callOllama makes a request to the Ollama API
func callOllama(ollamaURL, ollamaModel, prompt string, stream bool) (string, error) {
	// Prepare the request to Ollama
	requestBody, err := json.Marshal(map[string]interface{}{
		"model":  ollamaModel,
		"prompt": prompt,
		"stream": stream,
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

	if stream {
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
	} else {
		// Read the non-streaming response
		body, err := io.ReadAll(resp.Body)
		if err != nil {
			return "", fmt.Errorf("error reading response body: %w", err)
		}

		// Parse the JSON response
		var result map[string]interface{}
		if err := json.Unmarshal(body, &result); err != nil {
			return "", fmt.Errorf("error parsing JSON response: %w", err)
		}

		// Extract the response text
		if response, ok := result["response"].(string); ok {
			return response, nil
		}

		return "", fmt.Errorf("unexpected response format from Ollama: %s", string(body))
	}
}
