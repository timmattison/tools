package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

// This file contains integration tests that test the complete workflow
// These tests require git to be installed and available in the PATH

func setupTestRepo(t *testing.T) (string, func()) {
	// Create a temporary directory for the test repository
	tempDir, err := os.MkdirTemp("", "gitrdun-test-*")
	if err != nil {
		t.Fatalf("Failed to create temp dir: %v", err)
	}

	// Initialize a git repository
	cmd := exec.Command("git", "init")
	cmd.Dir = tempDir
	if err := cmd.Run(); err != nil {
		os.RemoveAll(tempDir)
		t.Fatalf("Failed to initialize git repo: %v", err)
	}

	// Configure git user
	cmd = exec.Command("git", "config", "user.name", "Test User")
	cmd.Dir = tempDir
	if err := cmd.Run(); err != nil {
		os.RemoveAll(tempDir)
		t.Fatalf("Failed to configure git user name: %v", err)
	}

	cmd = exec.Command("git", "config", "user.email", "test@example.com")
	cmd.Dir = tempDir
	if err := cmd.Run(); err != nil {
		os.RemoveAll(tempDir)
		t.Fatalf("Failed to configure git user email: %v", err)
	}

	// Create a test file and commit it
	testFile := filepath.Join(tempDir, "test.txt")
	if err := os.WriteFile(testFile, []byte("test content"), 0644); err != nil {
		os.RemoveAll(tempDir)
		t.Fatalf("Failed to create test file: %v", err)
	}

	cmd = exec.Command("git", "add", "test.txt")
	cmd.Dir = tempDir
	if err := cmd.Run(); err != nil {
		os.RemoveAll(tempDir)
		t.Fatalf("Failed to add test file: %v", err)
	}

	cmd = exec.Command("git", "commit", "-m", "Initial commit")
	cmd.Dir = tempDir
	if err := cmd.Run(); err != nil {
		os.RemoveAll(tempDir)
		t.Fatalf("Failed to commit test file: %v", err)
	}

	// Return the path to the test repository and a cleanup function
	return tempDir, func() {
		os.RemoveAll(tempDir)
	}
}

func TestEndToEndWorkflow(t *testing.T) {
	// Skip this test in short mode
	if testing.Short() {
		t.Skip("Skipping integration test in short mode")
	}

	// Set up a test repository
	repoPath, cleanup := setupTestRepo(t)
	defer cleanup()

	// Test the complete workflow
	// This would involve running the main function with different arguments
	// and checking the output

	// For now, we'll just test that we can find the repository
	// and that it has the expected commit

	// Create a searchResult to store the results
	result := searchResult{
		repositories: make(map[string][]string),
	}

	// Set up the threshold time to include our commit
	threshold := time.Now().Add(-1 * time.Hour)

	// Mock the user email to match the one we set in the test repository
	userEmail := "test@example.com"

	// Call processGitRepo directly to test it
	ignoreFailures := false
	searchAllBranches := true
	filterByUser := true
	processGitRepo(repoPath, &result, &ignoreFailures, &searchAllBranches,
		threshold, &gitStats{}, userEmail, &filterByUser)

	// Check that we found the repository and it has one commit
	if len(result.repositories) != 1 {
		t.Errorf("Expected 1 repository, got %d", len(result.repositories))
	}

	commits, ok := result.repositories[repoPath]
	if !ok {
		t.Errorf("Repository %s not found in results", repoPath)
	} else if len(commits) != 1 {
		t.Errorf("Expected 1 commit, got %d", len(commits))
	}
}

// Mock flag for testing
type filterByUserFlag struct {
	value bool
}

func (f *filterByUserFlag) Value() bool {
	return f.value
}
