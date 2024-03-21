package main

import (
	"flag"
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
	"os"
	"path/filepath"
	"time"
)

var filesMoved int64
var nameChecker internal.NameChecker

func main() {
	var suffixParam = flag.String("suffix", "", "suffix to search for")
	var prefixParam = flag.String("prefix", "", "prefix to search for")
	var substringParam = flag.String("substring", "", "substring to search for")
	var destinationParam = flag.String("destination", "", "destination to copy files to")

	flag.Parse()

	paramsSpecified := 0

	if *suffixParam != "" {
		nameChecker = internal.HasSuffixNameChecker(*suffixParam)
		paramsSpecified++
	}

	if *prefixParam != "" {
		nameChecker = internal.HasPrefixNameChecker(*prefixParam)
		paramsSpecified++
	}

	if *substringParam != "" {
		nameChecker = internal.ContainsNameChecker(*substringParam)
		paramsSpecified++
	}

	if paramsSpecified != 1 || *destinationParam == "" {
		flag.Usage()
		os.Exit(1)
	}

	if nameChecker == nil {
		panic("nameChecker should not be nil. This is a bug.")
	}

	var paths []string

	args := flag.Args()

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

	fileHandler := internal.FileHandler(func(fileInfo os.FileInfo) {
		// Move a file to the destination
		if err := os.Rename(fileInfo.Name(), filepath.Join(*destinationParam, fileInfo.Name())); err != nil {
			log.Fatal("Error moving file", "file", fileInfo.Name(), "error", err)
		}

		filesMoved++
	})

	startTime := time.Now()

	for path := range unique {
		if err := filepath.WalkDir(path, internal.VisitWithNameChecker(nameChecker, fileHandler, nil)); err != nil {
			log.Fatal("Error walking path", "path", path, "error", err)
		}
	}

	duration := time.Since(startTime)
	filesMovedPerSecond := float64(filesMoved) / duration.Seconds()

	log.Info("Move complete", "filesMoved", filesMoved, "duration", duration, "filesMovedPerSecond", filesMovedPerSecond)
}
