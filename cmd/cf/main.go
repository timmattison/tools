package main

import (
	"flag"
	"fmt"
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
	"path/filepath"
)

func main() {
	args := flag.Args()

	var paths []string

	if len(args) > 0 {
		for _, v := range args {
			paths = append(paths, v)
		}
	} else {
		paths = append(paths, ".")
	}

	unique := map[string]bool{}

	for _, v := range paths {
		unique[v] = true
	}

	var fileCount int64

	nameChecker := func(filename string) bool {
		fileCount++
		return false
	}

	for path := range unique {
		if err := filepath.WalkDir(path, internal.VisitWithNameChecker(nameChecker, nil, nil)); err != nil {
			log.Fatal("Error walking path", "path", path, "error", err)
		}
	}

	fmt.Println(internal.PrettyPrintInt(fileCount))
}
