package main

import (
	"flag"
	"fmt"
	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal"
	"os"
	"path/filepath"
)

var sizeTotal int64
var nameChecker internal.NameChecker

func visit(path string, dirEntry os.DirEntry, err error) error {
	if err != nil {
		log.Warn("Error visiting path", "path", path, "error", err)
		return nil
	}

	if dirEntry.IsDir() {
		return nil
	}

	if (nameChecker != nil) && nameChecker(dirEntry.Name()) {
		var info os.FileInfo

		if info, err = dirEntry.Info(); err != nil {
			log.Fatal("Couldn't get file info", "error", err)
		}

		sizeTotal += info.Size()
	}

	return nil
}

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
		var path string
		var err error

		if path, err = os.Getwd(); err != nil {
			log.Fatal("Couldn't get the current working directory", "error", err)
		}

		paths = append(paths, path)
	}

	unique := map[string]bool{}

	for _, v := range paths {
		unique[v] = true
	}

	for path := range unique {
		if err := filepath.WalkDir(path, visit); err != nil {
			log.Fatal("Error walking path", "path", path, "error", err)
		}
	}

	fmt.Println(internal.PrettyPrintBytes(uint64(sizeTotal)))
}
