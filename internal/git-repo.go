package internal

import (
	"os"
	"path"
	"path/filepath"
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
