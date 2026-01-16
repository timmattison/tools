package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
	"github.com/timmattison/tools/internal/version"
)

var filesMoved int64
var nameChecker internal.NameChecker

func main() {
	var showVersion bool
	flag.BoolVar(&showVersion, "version", false, "Show version information")
	flag.BoolVar(&showVersion, "V", false, "Show version information (shorthand)")
	var suffixParam = flag.String("suffix", "", "suffix to search for")
	var prefixParam = flag.String("prefix", "", "prefix to search for")
	var substringParam = flag.String("substring", "", "substring to search for")
	var destinationParam = flag.String("destination", "", "destination to copy files to")

	flag.Parse()

	if showVersion {
		fmt.Println(version.String("bm"))
		os.Exit(0)
	}

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

	fileHandler := internal.FileHandler(func(entryPath string, fileInfo os.FileInfo) {
		// Move a file to the destination
		if err := os.Rename(entryPath, filepath.Join(*destinationParam, fileInfo.Name())); err != nil {
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
