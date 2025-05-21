package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"github.com/go-git/go-git/v5"
	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/object"
	"github.com/timmattison/tools/internal"
)

// objectInfo represents information about a Git object
type objectInfo struct {
	Hash string
	Size int64
	Path string
}

func main() {
	// Parse command-line flags
	var repoPath string
	var topCount int
	flag.StringVar(&repoPath, "repo", "", "Path to the Git repository (optional, defaults to current directory)")
	flag.IntVar(&topCount, "top", 20, "Number of items to display (default: 20)")
	flag.Parse()

	var repo *git.Repository
	var err error

	// If repo path is specified, use it directly
	if repoPath != "" {
		// Find the absolute path to the repository
		absPath, err := filepath.Abs(repoPath)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Error resolving repository path: %v\n", err)
			os.Exit(1)
		}

		// Open the repository
		repo, err = git.PlainOpen(absPath)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Error opening Git repository: %v\n", err)
			os.Exit(1)
		}
	} else {
		// No repo path specified, use GetRepoBase to find the repository
		gitDir, err := internal.GetRepoBase()
		if err != nil {
			fmt.Fprintf(os.Stderr, "Error finding Git repository: %v\n", err)
			fmt.Fprintf(os.Stderr, "Use -repo flag to specify a repository path\n")
			os.Exit(1)
		}

		// Get the repository root (parent directory of .git)
		repoRoot := filepath.Dir(gitDir)

		// Open the repository
		repo, err = git.PlainOpen(repoRoot)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Error opening Git repository at %s: %v\n", repoRoot, err)
			os.Exit(1)
		}

		fmt.Printf("Using Git repository at: %s\n", repoRoot)
	}

	// Get all objects in the repository
	objects, err := getAllObjects(repo)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error getting Git objects: %v\n", err)
		os.Exit(1)
	}

	// Sort objects by size (smallest first)
	sort.Slice(objects, func(i, j int) bool {
		return objects[i].Size < objects[j].Size
	})

	// Limit the number of objects to display
	displayCount := topCount
	if displayCount > len(objects) {
		displayCount = len(objects)
	}

	// Print the objects (largest first, limited by topCount)
	// Start from the end of the slice to get the largest objects
	startIndex := len(objects) - displayCount
	for i := 0; i < displayCount; i++ {
		obj := objects[startIndex+i]
		fmt.Printf("%s %s %s\n", obj.Hash[:12], formatSize(obj.Size), obj.Path)
	}
}

// getAllObjects retrieves all blob objects in the repository
func getAllObjects(repo *git.Repository) ([]objectInfo, error) {
	var objects []objectInfo

	// Map to track objects we've already seen
	seenObjects := make(map[string]bool)

	// Get all references (branches, tags, etc.)
	refs, err := repo.References()
	if err != nil {
		return nil, fmt.Errorf("error getting references: %v", err)
	}

	// Process each reference to find all blob objects
	err = refs.ForEach(func(ref *plumbing.Reference) error {
		// Skip references that don't point to a commit
		if ref.Type() != plumbing.HashReference {
			return nil
		}

		// Get the commit
		commit, err := repo.CommitObject(ref.Hash())
		if err != nil {
			return nil // Skip this reference if we can't get the commit
		}

		// Get a commit iterator to walk through all commits
		commitIter, err := repo.Log(&git.LogOptions{From: commit.Hash})
		if err != nil {
			return nil // Skip if we can't get the commit iterator
		}
		defer commitIter.Close()

		// Process each commit
		return commitIter.ForEach(func(c *object.Commit) error {
			// Get the tree for this commit
			tree, err := c.Tree()
			if err != nil {
				return nil // Skip this commit if we can't get the tree
			}

			// Walk the tree and collect all blob objects
			return tree.Files().ForEach(func(f *object.File) error {
				hash := f.Hash.String()

				// Skip if we've already seen this object
				if seenObjects[hash] {
					return nil
				}
				seenObjects[hash] = true

				// Get the blob
				blob, err := repo.BlobObject(f.Hash)
				if err != nil {
					return nil // Skip this file if we can't get the blob
				}

				// Add the object to our list
				objects = append(objects, objectInfo{
					Hash: hash,
					Size: blob.Size,
					Path: f.Name,
				})

				return nil
			})
		})
	})

	if err != nil {
		return nil, fmt.Errorf("error processing references: %v", err)
	}

	return objects, nil
}

// formatSize formats a size in bytes to a human-readable string
func formatSize(size int64) string {
	const unit = 1024
	if size < unit {
		return fmt.Sprintf("%d B", size)
	}

	div, exp := int64(unit), 0
	for n := size / unit; n >= unit; n /= unit {
		div *= unit
		exp++
	}

	return fmt.Sprintf("%.1f %ciB", float64(size)/float64(div), "KMGTPE"[exp])
}
