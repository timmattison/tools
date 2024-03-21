package main

import (
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
	"os"
	"os/exec"
	"path"
	"path/filepath"
)

func runGoModTidy(dir string) error {
	cmd := exec.Command("go", "mod", "tidy")
	cmd.Dir = dir    // Set the working directory
	err := cmd.Run() // Execute the command
	return err
}

func main() {
	var repoBase string
	var err error

	if repoBase, err = internal.GetRepoBase(); err != nil {
		log.Fatal("Couldn't find the git repo", "error", err)
	}

	repo := path.Dir(repoBase) + "/"

	directoryHandler := internal.DirectoryHandler(func(entryPath string, entry os.DirEntry) {
		filename := path.Join(entryPath, "go.mod")

		if !internal.FileExists(filename) {
			return
		}

		if err = runGoModTidy(entryPath); err != nil {
			log.Fatal("Error running go mod tidy", "directory", entryPath, "error", err)
		}

		log.Info("Ran go mod tidy", "directory", entryPath)
	})

	if err = filepath.WalkDir(repo, internal.VisitWithNameChecker(nil, nil, directoryHandler)); err != nil {
		log.Fatal("Error walking path", "path", repo, "error", err)
	}
}
