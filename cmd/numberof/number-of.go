package main

import (
	"flag"
	"fmt"
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
	"os"
	"path/filepath"
)

var countTotal int64
var nameChecker internal.NameChecker

func main() {
	var suffixParam = flag.String("suffix", "", "suffix to search for")
	var prefixParam = flag.String("prefix", "", "prefix to search for")
	var substringParam = flag.String("substring", "", "substring to search for")

	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, "Usage of %s:\n", os.Args[0])
		flag.PrintDefaults()
		fmt.Fprintf(os.Stderr, "\n")
		fmt.Fprintf(os.Stderr, "Optionally, you can specify any number of paths to search after specifying the search option.\n")
		fmt.Fprintf(os.Stderr, "  (e.g. `%s -suffix mp4 ../videos ./more-videos`)\n", os.Args[0])
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

	if paramsSpecified != 1 {
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
		countTotal++
	})

	for path := range unique {
		countTotal = 0

		if err := filepath.WalkDir(path, internal.VisitWithNameChecker(nameChecker, fileHandler, nil)); err != nil {
			log.Fatal("Error walking path", "path", path, "error", err)
		}

		fmt.Println(fmt.Sprintf("%d %s", uint64(countTotal), path))
	}
}
