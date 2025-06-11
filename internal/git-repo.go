package internal

import (
	"errors"
	"fmt"
	"github.com/charmbracelet/log"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"strings"
)

func GetRepoBase() (string, error) {
	// Get the current working directory the user is in
	var cwd string
	var err error

	if cwd, err = os.Getwd(); err != nil {
		return "", err
	}

	// Make sure we don't end up in a loop by checking if our last path is the same as the current path
	lastPath := ""

	// Make sure we don't end up in a loop by checking to see if we do too many iterations
	maximumIterations := 50
	iterationCount := 0

	basePath := cwd

	for {
		gitDirectory := path.Join(basePath, ".git")

		if _, err = os.Stat(gitDirectory); err == nil {
			// .git exists, we are done
			// NOTE: This means this may not work as expected when using submodules or nested repos
			return gitDirectory, nil
		}

		if !os.IsPermission(err) && !os.IsNotExist(err) {
			// If permission is denied or the file doesn't exist we can just ignore it but anything else is a legit error
			return "", err
		}

		// Go up one level
		basePath = filepath.Dir(basePath)

		iterationCount++

		if iterationCount >= maximumIterations {
			// Too many iterations
			break
		}

		if lastPath == basePath {
			// Ended up in the same place we came from
			break
		}

		// Keep track of where we just were
		lastPath = basePath
	}

	return "", os.ErrNotExist
}

func RunCommandInRepoDirectoriesWithFile(file string, command []string) error {
	var repoBase string
	var err error

	commandString := strings.Join(command, " ")

	if repoBase, err = GetRepoBase(); err != nil {
		return errors.New(fmt.Sprintf("Couldn't find the git repo [%s]", err))
	}

	repo := path.Dir(repoBase) + "/"

	directoryHandler := DirectoryHandler(func(entryPath string, entry os.DirEntry) {
		filename := path.Join(entryPath, file)

		if !FileExists(filename) {
			return
		}

		if err = runCommandInDirectory(entryPath, command); err != nil {
			log.Error(fmt.Sprintf("Error running %s", commandString), "directory", entryPath, "error", err)
		}

		log.Info(fmt.Sprintf("Ran %s", commandString), "directory", entryPath)
	})

	if err = filepath.WalkDir(repo, VisitWithNameChecker(nil, nil, directoryHandler)); err != nil {
		return errors.New(fmt.Sprintf("Error walking path [%s] [%s]", repo, err))
	}

	return nil
}

func runCommandInDirectory(dir string, command []string) error {
	cmd := exec.Command(command[0], command[1:]...)
	cmd.Dir = dir
	return cmd.Run()
}
