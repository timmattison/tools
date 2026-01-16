package main

import (
	"flag"
	"fmt"
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
	"os"
	"path/filepath"
	"time"
)

var filesMoved int64
var nameChecker internal.NameChecker

func main() {
	var suffixParam = flag.String("suffix", "", "suffix to search for (e.g., .jpg, .mkv)")
	var prefixParam = flag.String("prefix", "", "prefix to search for (e.g., IMG_, video_)")
	var substringParam = flag.String("substring", "", "substring to search for (e.g., 2024)")
	var destinationParam = flag.String("destination", "", "destination directory to move files to (required)")

	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, `bm - Bulk Move

Recursively find and move files matching a pattern to a destination directory.
Named "bm" because moving lots of files is shitty.

USAGE:
    bm -suffix|-prefix|-substring <PATTERN> -destination <PATH> [directories...]

OPTIONS:
`)
		flag.PrintDefaults()
		fmt.Fprintf(os.Stderr, `
EXAMPLES:
    # Move all .mkv files from current directory to backup drive
    bm -suffix .mkv -destination /Volumes/Backup/videos

    # Move camera photos (IMG_ prefix) to Pictures folder
    bm -prefix IMG_ -destination ~/Pictures/camera ~/Downloads

    # Move files containing "2024" in name to archive
    bm -substring 2024 -destination ~/archive/2024

    # Move PDFs from multiple directories
    bm -suffix .pdf -destination ~/Documents/pdfs ~/Downloads ~/Desktop

WHY USE BM INSTEAD OF MV?
    With mv and find (verbose, easy to get wrong):
        find . -name "*.mkv" -exec mv {} /backup/ \;

    With bm (simple and clear):
        bm -suffix .mkv -destination /backup

NOTES:
    - Exactly one of -suffix, -prefix, or -substring must be specified
    - If no source directories are given, searches the current directory
    - Files are moved recursively from all subdirectories
    - Shows statistics on completion (files moved, duration, rate)
`)
	}

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
