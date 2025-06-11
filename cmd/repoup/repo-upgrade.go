package main

import (
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
)

func main() {
	if err := internal.RunCommandInRepoDirectoriesWithFile("go.mod", []string{"go", "get", "-u", "all"}); err != nil {
		log.Warn(err)
	}

	if err := internal.RunCommandInRepoDirectoriesWithFile("Cargo.toml", []string{"cargo", "upgrade"}); err != nil {
		log.Warn(err)
	}
}
