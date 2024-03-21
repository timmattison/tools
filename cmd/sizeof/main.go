package main

import (
	"flag"
	"fmt"
	"github.com/timmattison/tools/internal"
	"os"
	"path/filepath"
)

var sizeTotal int64
var nameChecker internal.NameChecker

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

func main() {
	var suffixParam = flag.String("suffix", "", "suffix to search for")
	var prefixParam = flag.String("prefix", "", "prefix to search for")
	var substringParam = flag.String("substring", "", "substring to search for")

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

	println(internal.PrettyPrintBytes(uint64(sizeTotal)))
}
