package internal

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"github.com/go-git/go-git/v5/plumbing"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/go-git/go-git/v5"
)

// StatusCallback is a function that can be called to update status during processing
type StatusCallback func(status string)

// ModelContextSizes defines the context window sizes for different models
var ModelContextSizes = map[string]int{
	"llama2:7b":     4096,
	"llama2:13b":    4096,
	"mistral:7b":    8192,
	"qwen3:30b-a3b": 32768,
	// Add other models as needed
	"default": 8192, // Default size if model not found
}

// estimateTokens provides a rough estimate of the number of tokens in a text
func estimateTokens(text string) int {
	// Rough estimate: 1 token ≈ 4 characters for English text
	return len(text) / 4
}

// scoreFileRelevance scores a file's relevance based on its path and estimated importance
func scoreFileRelevance(filePath string) int {
	score := 10 // Base score

	// Reduce score for less relevant file types
	if strings.HasSuffix(filePath, ".lock") ||
		strings.HasSuffix(filePath, ".sum") ||
		strings.HasSuffix(filePath, ".mod") ||
		strings.Contains(filePath, "node_modules/") ||
		strings.Contains(filePath, "vendor/") ||
		strings.Contains(filePath, ".git/") {
		score /= 5
	}

	// Further reduce score for binary or generated files
	if strings.HasSuffix(filePath, ".png") ||
		strings.HasSuffix(filePath, ".jpg") ||
		strings.HasSuffix(filePath, ".jpeg") ||
		strings.HasSuffix(filePath, ".gif") ||
		strings.HasSuffix(filePath, ".pdf") ||
		strings.HasSuffix(filePath, ".zip") ||
		strings.HasSuffix(filePath, ".tar") ||
		strings.HasSuffix(filePath, ".gz") {
		score /= 10
	}

	// Increase score for likely important files
	if strings.HasSuffix(filePath, ".go") ||
		strings.HasSuffix(filePath, ".js") ||
		strings.HasSuffix(filePath, ".py") ||
		strings.HasSuffix(filePath, ".java") ||
		strings.HasSuffix(filePath, ".c") ||
		strings.HasSuffix(filePath, ".cpp") ||
		strings.HasSuffix(filePath, ".h") {
		score *= 2
	}

	// Boost score for documentation and configuration files
	if strings.Contains(strings.ToLower(filePath), "readme") ||
		strings.Contains(strings.ToLower(filePath), "config") ||
		strings.HasSuffix(filePath, ".md") ||
		strings.HasSuffix(filePath, ".yaml") ||
		strings.HasSuffix(filePath, ".yml") ||
		strings.HasSuffix(filePath, ".json") {
		score *= 3
	}

	return score
}

// GenerateOllamaSummary generates a summary of recent commits using Ollama
func GenerateOllamaSummary(repoPath string, commits []string, ollamaURL, ollamaModel string, keepThinking bool, statusCallback StatusCallback) (string, error) {
	if statusCallback != nil {
		statusCallback(fmt.Sprintf("Analyzing repository %s", filepath.Base(repoPath)))
	}
	// Get repository context
	repoContext := fmt.Sprintf("Repository: %s", filepath.Base(repoPath))

	// Try to read README file for additional context
	readmeContent := ""
	readmePaths := []string{
		filepath.Join(repoPath, "README.md"),
		filepath.Join(repoPath, "README"),
		filepath.Join(repoPath, "readme.md"),
	}

	for _, readmePath := range readmePaths {
		if content, err := os.ReadFile(readmePath); err == nil {
			readmeContent = fmt.Sprintf("README:\n```\n%s\n```\n\n", string(content))
			break
		}
	}

	// Get detailed commit information using go-git
	detailedCommits := ""
	totalSize := 0
	maxSize := 50000 // Limit the total size to avoid overwhelming the model

	// Open the repository
	repo, err := git.PlainOpen(repoPath)
	if err == nil {
		// Process each commit
		totalCommits := len(commits)
		for i, commitLine := range commits {
			if statusCallback != nil && i%5 == 0 {
				// Update status every 5 commits to avoid too many updates
				statusCallback(fmt.Sprintf("Analyzing commit %d of %d (%d%%)",
					i+1, totalCommits, (i+1)*100/totalCommits))
			}
			// Extract commit hash from the line
			parts := strings.SplitN(commitLine, " ", 2)
			if len(parts) < 1 {
				continue
			}

			hash := parts[0]

			// Get the commit object
			commitObj, err := repo.CommitObject(plumbing.NewHash(hash))
			if err != nil {
				continue
			}

			// Add commit info to detailed commits
			detailedCommits += fmt.Sprintf("COMMIT: %s\nAUTHOR: %s <%s>\nDATE: %s\nMESSAGE:\n%s\n\n",
				commitObj.Hash.String(),
				commitObj.Author.Name,
				commitObj.Author.Email,
				commitObj.Author.When.Format(time.RFC3339),
				commitObj.Message)

			// Get the changes in this commit
			if commitObj.NumParents() > 0 {
				// Get parent commit
				parent, err := commitObj.Parent(0)
				if err == nil {
					// Get changes between parent and this commit
					patch, err := parent.Patch(commitObj)
					if err == nil {
						detailedCommits += "CHANGES:\n"

						for _, filePatch := range patch.FilePatches() {
							from, to := filePatch.Files()
							var filePath string

							// Determine file path
							if to != nil {
								filePath = to.Path()
							} else if from != nil {
								filePath = from.Path()
							} else {
								continue
							}

							// Add file info to the list
							detailedCommits += fmt.Sprintf("- %s\n", filePath)

							// Score the file for relevance
							score := scoreFileRelevance(filePath)

							// Only process files with a score above threshold
							if score > 5 {
								// Construct the full file path
								fullFilePath := filepath.Join(repoPath, filePath)

								if statusCallback != nil {
									// Add more detailed information about what's happening with the file
									fileExt := filepath.Ext(filePath)
									fileSize := "file not found"
									if info, err := os.Stat(fullFilePath); err == nil {
										fileSize = fmt.Sprintf("%.1f KB", float64(info.Size())/1024)
									}
									statusCallback(fmt.Sprintf("Processing file %s (%s, %s, relevance score: %d, commit: %d/%d)",
										filePath, fileExt, fileSize, score, i, len(commits)))
								}

								// Try to get current file content from filesystem first
								file, err := os.ReadFile(fullFilePath)
								if err != nil {
									// If file not found in filesystem, try to get it from git
									if statusCallback != nil {
										statusCallback(fmt.Sprintf("File not found in filesystem, retrieving from git: %s", filePath))
									}

									// Get file from commit
									gitFile, err := commitObj.File(filePath)
									if err != nil {
										// Skip if file not found in git either
										if statusCallback != nil {
											statusCallback(fmt.Sprintf("File not found in git either: %s", filePath))
										}
										continue
									}

									// Get file content
									content, err := gitFile.Contents()
									if err != nil {
										// Skip if can't get content
										continue
									}

									// Update file size in status message if needed
									if statusCallback != nil {
										fileSize := fmt.Sprintf("%.1f KB (from git)", float64(len(content))/1024)
										statusCallback(fmt.Sprintf("Processing file %s (%s, %s, relevance score: %d, commit: %d/%d)",
											filePath, filepath.Ext(filePath), fileSize, score, i, len(commits)))
									}

									// Only include if not too large
									if len(content) > 5000 {
										fileInfo := fmt.Sprintf("FILE %s: (truncated, too large)\n", filePath)
										detailedCommits += fileInfo
										totalSize += len(fileInfo)
									} else {
										fileInfo := fmt.Sprintf("FILE %s:\n```\n%s\n```\n\n", filePath, content)
										detailedCommits += fileInfo
										totalSize += len(fileInfo)
									}
								} else {
									// File found in filesystem
									content := string(file)

									// Only include if not too large
									if len(content) > 5000 {
										fileInfo := fmt.Sprintf("FILE %s: (truncated, too large)\n", filePath)
										detailedCommits += fileInfo
										totalSize += len(fileInfo)
									} else {
										fileInfo := fmt.Sprintf("FILE %s:\n```\n%s\n```\n\n", filePath, content)
										detailedCommits += fileInfo
										totalSize += len(fileInfo)
									}
								}
							}
						}
					}
				}
			}

			detailedCommits += "---\n"

			// Check if we've exceeded our size limit
			if totalSize >= maxSize {
				if statusCallback != nil {
					statusCallback(fmt.Sprintf("Content size limit reached (%d KB), truncating additional information", maxSize/1024))
				}
				detailedCommits += "...\n(truncated for brevity, reached size limit)\n"
				break
			}
		}
	}

	// Format the prompt for Ollama
	if statusCallback != nil {
		statusCallback("Preparing prompt for Ollama summarization")
	}

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

	if statusCallback != nil {
		promptSize := len(prompt) / 1024
		statusCallback(fmt.Sprintf("Sending %d KB of data to Ollama for processing", promptSize))
	}

	return callOllama(ollamaURL, ollamaModel, prompt, true, keepThinking, statusCallback)
}

// GenerateMetaSummary generates a meta-summary across multiple repositories
func GenerateMetaSummary(summaries []string, ollamaURL, ollamaModel string, duration time.Duration, keepThinking bool, statusCallback StatusCallback) (string, error) {
	if statusCallback != nil {
		statusCallback("Generating meta-summary across all repositories")
	}
	// Format the prompt for Ollama
	if statusCallback != nil {
		statusCallback("Preparing prompt for meta-summary generation")
	}

	prompt := fmt.Sprintf(`Please provide a comprehensive overview of all work done across multiple repositories over the past %v.

Here are summaries from each repository:

%s

Focus on the big picture rather than repeating details from individual repositories.`,
		duration,
		strings.Join(summaries, "\n\n---\n\n"),
	)

	if statusCallback != nil {
		promptSize := len(prompt) / 1024
		statusCallback(fmt.Sprintf("Sending %d KB of data to Ollama for meta-summary processing", promptSize))
	}

	return callOllama(ollamaURL, ollamaModel, prompt, false, keepThinking, statusCallback)
}

// callOllama makes a request to the Ollama API
func callOllama(ollamaURL, ollamaModel, prompt string, stream bool, keepThinking bool, statusCallback ...StatusCallback) (string, error) {
	// Handle optional statusCallback parameter
	var callback StatusCallback
	if len(statusCallback) > 0 && statusCallback[0] != nil {
		callback = statusCallback[0]
		callback("Preparing to send request to Ollama")

		// Get the context window size for this model
		contextSize, ok := ModelContextSizes[ollamaModel]
		if !ok {
			contextSize = ModelContextSizes["default"]
		}

		// Estimate tokens in the prompt
		estimatedTokens := estimateTokens(prompt)
		callback(fmt.Sprintf("Estimated tokens: %d (model context size: %d)", estimatedTokens, contextSize))

		// If we're close to the context limit, try to reduce the prompt size
		if float64(estimatedTokens) > float64(contextSize)*0.9 {
			callback("Prompt is too large, may exceed model context window")
		}
	}
	// Check if the prompt is too large and might cause scanner issues
	const maxScannerTokenSize = 64 * 1024 // Default scanner buffer size is 64KB

	if len(prompt) > maxScannerTokenSize && stream {
		// For streaming responses with large prompts, split into chunks
		return callOllamaWithChunks(ollamaURL, ollamaModel, prompt, keepThinking, statusCallback...)
	}

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
		// Increase the buffer size to handle larger tokens
		const maxBufferSize = 1024 * 1024 // 1MB buffer
		scannerBuffer := make([]byte, maxBufferSize)
		scanner.Buffer(scannerBuffer, maxBufferSize)

		var fullResponse strings.Builder
		responseLength := 0
		lastUpdate := time.Now()

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
			responseLength += len(responseChunk.Response)

			// Update status every 500ms to show progress
			if callback != nil && time.Since(lastUpdate) > 500*time.Millisecond {
				callback(fmt.Sprintf("Receiving response from Ollama (%d characters so far)", responseLength))
				lastUpdate = time.Now()
			}

			if responseChunk.Done {
				if callback != nil {
					callback(fmt.Sprintf("Response complete, received %d characters", responseLength))
				}
				break
			}
		}

		if err := scanner.Err(); err != nil {
			// Check if it's a token too long error
			if errors.Is(err, bufio.ErrTooLong) {
				// Try again with the chunking approach
				return callOllamaWithChunks(ollamaURL, ollamaModel, prompt, keepThinking)
			}
			return "", fmt.Errorf("error reading response stream: %w", err)
		}

		response := fullResponse.String()

		// Remove text between <think> and </think> tags if keepThinking is false
		if !keepThinking {
			response = removeThinkingText(response)
		}

		return response, nil
	} else {
		// Read the non-streaming response
		if callback != nil {
			callback("Waiting for complete response from Ollama")
		}

		body, err := io.ReadAll(resp.Body)
		if err != nil {
			return "", fmt.Errorf("error reading response body: %w", err)
		}

		if callback != nil {
			callback(fmt.Sprintf("Received %d KB response from Ollama", len(body)/1024))
		}

		// Parse the JSON response
		var result map[string]interface{}
		if err := json.Unmarshal(body, &result); err != nil {
			return "", fmt.Errorf("error parsing JSON response: %w", err)
		}

		// Extract the response text
		if response, ok := result["response"].(string); ok {
			// Remove text between <think> and </think> tags if keepThinking is false
			if !keepThinking {
				response = removeThinkingText(response)
			}

			if callback != nil {
				callback(fmt.Sprintf("Processing complete, final response is %d characters", len(response)))
			}

			return response, nil
		}

		return "", fmt.Errorf("unexpected response format from Ollama: %s", string(body))
	}
}

// removeThinkingText removes text between <think> and </think> tags
func removeThinkingText(text string) string {
	// Find all occurrences of <think> and </think> tags
	startTag := "<think>"
	endTag := "</think>"

	for {
		startIdx := strings.Index(text, startTag)
		if startIdx == -1 {
			break
		}

		endIdx := strings.Index(text[startIdx:], endTag)
		if endIdx == -1 {
			break
		}

		// Calculate the actual end index in the original string
		endIdx = startIdx + endIdx + len(endTag)

		// Remove the text between the tags, including the tags themselves
		text = text[:startIdx] + text[endIdx:]
	}

	return text
}

// callOllamaWithChunks handles large prompts by splitting them into multiple requests
func callOllamaWithChunks(ollamaURL, ollamaModel, prompt string, keepThinking bool, statusCallback ...StatusCallback) (string, error) {
	// Handle optional statusCallback parameter
	var callback StatusCallback
	if len(statusCallback) > 0 {
		callback = statusCallback[0]
		if callback != nil {
			callback("Prompt is too large, splitting into chunks for processing")
		}
	}
	// Split the prompt into manageable chunks
	const chunkSize = 32 * 1024 // 32KB chunks

	// First, try to make a non-streaming request with the full prompt
	// This is simpler and might work for many models
	if callback != nil {
		callback("Attempting to process entire prompt in one request")
	}

	requestBody, err := json.Marshal(map[string]interface{}{
		"model":  ollamaModel,
		"prompt": prompt,
		"stream": false, // Use non-streaming for large prompts
	})
	if err != nil {
		return "", fmt.Errorf("error creating request: %w", err)
	}

	if callback != nil {
		callback(fmt.Sprintf("Sending %d KB to Ollama in single request", len(prompt)/1024))
	}

	resp, err := http.Post(ollamaURL+"/api/generate", "application/json", bytes.NewBuffer(requestBody))
	if err != nil {
		if callback != nil {
			callback("Single request failed, will try chunked approach")
		}
		return "", fmt.Errorf("error calling Ollama API: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("error reading response body: %w", err)
	}

	// Parse the JSON response
	var result map[string]interface{}
	if err := json.Unmarshal(body, &result); err != nil {
		// If we can't parse the response, it might be too large for the model
		// Fall back to the chunking approach
	} else {
		// Extract the response text
		if response, ok := result["response"].(string); ok {
			return response, nil
		}
	}

	// If the simple approach failed, try the more complex chunking approach
	if callback != nil {
		callback("Starting chunked processing approach")
	}

	// Split the prompt into introduction, body chunks, and conclusion
	promptLines := strings.Split(prompt, "\n")
	totalLines := len(promptLines)

	if callback != nil {
		callback(fmt.Sprintf("Splitting %d lines of text into manageable chunks", totalLines))
	}

	// First 10 lines as introduction
	introLines := min(10, totalLines)
	introduction := strings.Join(promptLines[:introLines], "\n")

	// Last 10 lines as conclusion
	conclusionStart := max(introLines, totalLines-10)
	conclusion := strings.Join(promptLines[conclusionStart:], "\n")

	// Middle part to be chunked
	middleLines := promptLines[introLines:conclusionStart]

	if callback != nil {
		numChunks := (len(middleLines) + chunkSize - 1) / chunkSize
		callback(fmt.Sprintf("Text will be processed in %d chunks plus intro and conclusion", numChunks))
	}

	// Process in chunks
	var summaries []string

	for i := 0; i < len(middleLines); i += chunkSize {
		end := min(i+chunkSize, len(middleLines))
		chunk := strings.Join(middleLines[i:end], "\n")

		chunkPrompt := fmt.Sprintf("This is part %d of a larger document. Please analyze this part:\n\n%s",
			(i/chunkSize)+1, chunk)

		// Process this chunk
		if callback != nil {
			callback(fmt.Sprintf("Processing chunk %d of %d", (i/chunkSize)+1, (len(middleLines)+chunkSize-1)/chunkSize))
		}
		chunkSummary, err := callOllama(ollamaURL, ollamaModel, chunkPrompt, false, keepThinking, callback)
		if err != nil {
			return "", fmt.Errorf("error processing chunk %d: %w", (i/chunkSize)+1, err)
		}

		summaries = append(summaries, chunkSummary)
	}

	// Final prompt to combine all summaries
	if callback != nil {
		callback(fmt.Sprintf("Preparing final prompt with %d chunk summaries", len(summaries)))
	}

	finalPrompt := fmt.Sprintf(`I've analyzed a document in parts. Here's the introduction:

%s

Here are summaries of each part:
%s

And here's the conclusion:
%s

Based on all this information, please provide a comprehensive response to the original request.`,
		introduction,
		strings.Join(summaries, "\n\n---\n\n"),
		conclusion)

	if callback != nil {
		finalPromptSize := len(finalPrompt) / 1024
		callback(fmt.Sprintf("Final combined prompt is %d KB, sending for processing", finalPromptSize))
	}

	// Get the final combined response
	if callback != nil {
		callback("Generating final response from all chunks")
	}
	return callOllama(ollamaURL, ollamaModel, finalPrompt, false, keepThinking, callback)
}
