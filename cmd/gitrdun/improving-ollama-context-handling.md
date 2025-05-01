# Improving Ollama Context Handling in gitrdun

## Current Problem

gitrdun sends text to Ollama for summarization but can end up sending too much text, exceeding Ollama's context window size. This results in:

1. Potential loss of information
2. Incomplete or inaccurate summaries
3. Possible API errors or timeouts
4. Inefficient use of the LLM's capabilities

## Current Implementation Analysis

The current implementation already has some mechanisms to handle large text volumes:

1. A 50,000 character limit for detailed commit information
2. Truncation of large files (>5,000 characters)
3. A chunking mechanism that splits large prompts into 32KB chunks
4. Fallback strategies when token limits are exceeded

However, these mechanisms are not sufficient to prevent context window issues in all cases.

## Proposed Solutions

### 1. Smart Content Filtering and Prioritization

**Implementation:**
- Implement a scoring system for content relevance
- Prioritize recent commits over older ones
- Filter out binary files, generated code, and other low-information content
- Prioritize files with meaningful changes (e.g., more lines changed)

```go
// Example scoring function
func scoreFileRelevance(filePath string, linesChanged int) int {
    score := linesChanged // Base score on lines changed
    
    // Reduce score for less relevant file types
    if strings.HasSuffix(filePath, ".lock") || 
       strings.HasSuffix(filePath, ".sum") ||
       strings.Contains(filePath, "node_modules/") {
        score /= 10
    }
    
    // Increase score for likely important files
    if strings.HasSuffix(filePath, ".go") || 
       strings.HasSuffix(filePath, ".js") ||
       strings.Contains(filePath, "README") {
        score *= 2
    }
    
    return score
}
```

### 2. Dynamic Context Window Management

**Implementation:**
- Add a configuration option for maximum context size
- Dynamically adjust content based on the model being used
- Implement a token counting estimation function
- Set model-specific limits based on known context windows

```go
// Example token estimation (simplified)
func estimateTokens(text string) int {
    // Rough estimate: 1 token ≈ 4 characters for English text
    return len(text) / 4
}

// Example context window sizes for different models
var modelContextSizes = map[string]int{
    "llama2:7b":    4096,
    "llama2:13b":   4096,
    "mistral:7b":   8192,
    "qwen3:30b-a3b": 32768,
    // Add other models as needed
}
```

### 3. Hierarchical Summarization

**Implementation:**
- Implement a multi-level summarization approach
- First summarize individual files or small groups of commits
- Then summarize these summaries at a higher level
- Create a tree-like structure of summaries

```go
// Pseudocode for hierarchical summarization
func generateHierarchicalSummary(commits []string) string {
    // Group commits by day or by related files
    commitGroups := groupCommits(commits)
    
    // Generate summaries for each group
    groupSummaries := []string{}
    for _, group := range commitGroups {
        summary := summarizeCommitGroup(group)
        groupSummaries = append(groupSummaries, summary)
    }
    
    // Generate meta-summary from group summaries
    return generateMetaSummary(groupSummaries)
}
```

### 4. Incremental Processing

**Implementation:**
- Process repositories incrementally rather than all at once
- Maintain a state of previously processed commits
- Only process new commits since the last run
- Combine with previous summaries for context

```go
// Pseudocode for incremental processing
func incrementalSummarization(repoPath string) string {
    // Get last processed commit hash from state file
    lastProcessedCommit := readLastProcessedCommit(repoPath)
    
    // Get only new commits since last processed
    newCommits := getCommitsSince(repoPath, lastProcessedCommit)
    
    // Get previous summary if available
    previousSummary := readPreviousSummary(repoPath)
    
    // Generate summary for new commits
    newSummary := summarizeCommits(newCommits)
    
    // Combine with context from previous summary
    combinedSummary := combineWithContext(previousSummary, newSummary)
    
    // Save state for next run
    saveLastProcessedCommit(repoPath, getLatestCommit(repoPath))
    savePreviousSummary(repoPath, combinedSummary)
    
    return combinedSummary
}
```

### 5. Adaptive Chunking with Semantic Boundaries

**Implementation:**
- Improve the current chunking mechanism to respect semantic boundaries
- Split text at logical points (e.g., between commits, files, or paragraphs)
- Adjust chunk size dynamically based on content complexity
- Use overlapping chunks to maintain context between chunks

```go
// Pseudocode for semantic chunking
func semanticChunking(text string, maxChunkSize int) []string {
    chunks := []string{}
    
    // Split text at semantic boundaries
    sections := splitAtSemanticBoundaries(text)
    
    currentChunk := ""
    for _, section := range sections {
        // If adding this section would exceed max size, start a new chunk
        if len(currentChunk) + len(section) > maxChunkSize && len(currentChunk) > 0 {
            chunks = append(chunks, currentChunk)
            // Include some overlap for context
            currentChunk = getLastNLines(currentChunk, 5) + section
        } else {
            currentChunk += section
        }
    }
    
    // Add the last chunk if not empty
    if len(currentChunk) > 0 {
        chunks = append(chunks, currentChunk)
    }
    
    return chunks
}
```

### 6. Model-Specific Optimizations

**Implementation:**
- Add support for different prompt templates optimized for different models
- Implement model-specific preprocessing steps
- Allow users to specify different models for different summarization tasks
- Provide fallback options for when preferred models are unavailable

```go
// Example model-specific prompt templates
var modelPromptTemplates = map[string]string{
    "default": `Please summarize the work done in this repository based on these recent commits.
%s
%s
Recent commits:
%s

Detailed commit information with file contents:
%s

Please provide a concise summary of what was worked on in this repository.`,

    "qwen3:30b-a3b": `You are a technical expert analyzing code changes. 
Summarize the following repository changes with technical precision.
%s
%s
Recent commits:
%s

Detailed commit information with file contents:
%s

Focus on key technical changes, architectural decisions, and implementation details.`,

    // Add other model-specific templates
}
```

## Implementation Recommendations

1. **Short-term improvements:**
   - Implement smarter content filtering to reduce input size
   - Add configuration options for context window size
   - Improve the existing chunking mechanism with semantic boundaries

2. **Medium-term improvements:**
   - Implement hierarchical summarization
   - Add model-specific optimizations
   - Develop token counting estimation

3. **Long-term improvements:**
   - Implement incremental processing with state management
   - Develop a fully adaptive system that learns from previous runs
   - Consider implementing a local caching mechanism for summaries

## Conclusion

By implementing these improvements, gitrdun can more effectively manage large volumes of text when generating summaries with Ollama. This will result in more reliable operation, better quality summaries, and a more efficient use of the LLM's capabilities.

The proposed solutions are designed to be modular, allowing for incremental implementation and testing. They also provide flexibility for users with different needs and different Ollama models.