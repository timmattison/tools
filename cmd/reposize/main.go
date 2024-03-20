package main

import (
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
)

func main() {
	var repoBase string
	var err error

	if repoBase, err = internal.GetRepoBase(); err != nil {
		log.Fatal("Couldn't find the git repo", "error", err)
	}

	var totalSize int64

	if totalSize, err = internal.CalculateDirSize(repoBase); err != nil {
		log.Fatal("Couldn't calculate the git repo's size", "error", err)
	}

	log.Info("Git repo size", "size", internal.PrettyPrintInt(totalSize), "path", repoBase)
}
