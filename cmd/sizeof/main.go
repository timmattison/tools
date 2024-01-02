package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

var sizeTotal int64
var nameChecker NameChecker

func visit(path string, dirEntry os.DirEntry, err error) error {
	if err != nil {
		fmt.Printf("error visiting path %s: %v\n", path, err)
		return nil
	}

	if dirEntry.IsDir() {
		return nil
	}

	if (nameChecker != nil) && nameChecker(dirEntry.Name()) {
		info, err := dirEntry.Info()

		if err != nil {
			panic(err)
		}

		sizeTotal += info.Size()
	}

	return nil
}

type NameChecker func(filename string) bool

func HasSuffixNameChecker(suffix string) NameChecker {
	return func(filename string) bool {
		return strings.HasSuffix(filename, suffix)
	}
}

func HasPrefixNameChecker(prefix string) NameChecker {
	return func(filename string) bool {
		return strings.HasPrefix(filename, prefix)
	}
}

func ContainsNameChecker(substring string) NameChecker {
	return func(filename string) bool {
		return strings.Contains(filename, substring)
	}
}

func formatBytes(bytes uint64) string {
	const unit = 1024
	if bytes < unit {
		return fmt.Sprintf("%dB", bytes)
	}
	div, exp := int64(unit), 0
	for n := bytes / unit; n >= unit; n /= unit {
		div *= unit
		exp++
	}

	return fmt.Sprintf("%.1f%cB", float64(bytes)/float64(div), "kMGTPE"[exp])
}

func main() {
	var suffixParam = flag.String("suffix", "", "suffix to search for")
	var prefixParam = flag.String("prefix", "", "prefix to search for")
	var substringParam = flag.String("substring", "", "substring to search for")

	flag.Parse()
	paramsSpecified := 0

	if *suffixParam != "" {
		nameChecker = HasSuffixNameChecker(*suffixParam)
		paramsSpecified++
	}

	if *prefixParam != "" {
		nameChecker = HasPrefixNameChecker(*prefixParam)
		paramsSpecified++
	}

	if *substringParam != "" {
		nameChecker = ContainsNameChecker(*substringParam)
		paramsSpecified++
	}

	if paramsSpecified == 0 {
		fmt.Println("You must specify at least one of -suffix, -prefix, or -substring")
		os.Exit(1)
	}

	if paramsSpecified > 1 {
		fmt.Println("You must specify only one of -suffix, -prefix, or -substring")
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
		path, err := os.Getwd()

		if err != nil {
			panic(err)
		}

		paths = append(paths, path)
	}

	unique := map[string]bool{}

	for _, v := range paths {
		unique[v] = true
	}

	for path := range unique {
		if err := filepath.WalkDir(path, visit); err != nil {
			panic(err)
		}
	}

	println(formatBytes(uint64(sizeTotal)))
}
