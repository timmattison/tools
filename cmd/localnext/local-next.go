package main

import (
	"flag"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/charmbracelet/log"
	"github.com/timmattison/tools/internal/version"
)

const ExpectedRootDirectory = "out"
const ExpectedStaticDirectory = "static"
const StaticPattern = "/" + ExpectedStaticDirectory + "/"

var root string
var showVersion bool

func init() {
	flag.BoolVar(&showVersion, "version", false, "Show version information")
	flag.BoolVar(&showVersion, "V", false, "Show version information (shorthand)")
}

func fromRoot(path string) string {
	if root == "" {
		log.Fatal("Root directory not set")
	}

	return filepath.Join(root, path)
}

func main() {
	flag.Parse()

	if showVersion {
		fmt.Println(version.String("localnext"))
		os.Exit(0)
	}

	var cwd string
	var err error

	if cwd, err = os.Getwd(); err != nil {
		log.Fatal("Couldn't get current working directory", "error", err)
	}

	if filepath.Base(cwd) == ExpectedRootDirectory {
		root = cwd
	} else if _, err = os.Stat(ExpectedRootDirectory); err == nil {
		root = filepath.Join(cwd, ExpectedRootDirectory)
	} else {
		log.Fatal("Couldn't find the expected root directory", "expectedRootDirectory", ExpectedRootDirectory, "error", err)
	}

	staticDir := fromRoot(ExpectedStaticDirectory)

	fs := http.FileServer(http.Dir(staticDir))
	http.Handle(StaticPattern, http.StripPrefix(StaticPattern, fs))

	http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		// Determine the requested path
		path := r.URL.Path

		// Default to serving index.html
		filePath := "index.html"

		if path != "/" {
			trimmedPath := strings.Trim(path, "/")

			potentialStaticFile := fromRoot(trimmedPath)
			potentialHtmlFile := fromRoot(trimmedPath + ".html")

			if _, err = os.Stat(potentialStaticFile); err == nil {
				filePath = trimmedPath
			} else if _, err = os.Stat(potentialHtmlFile); err == nil {
				filePath = trimmedPath + ".html"
			} else {
				log.Warn("Couldn't find file", "path", path, "file", trimmedPath)
			}
		}

		finalPath := fromRoot(filePath)

		http.ServeFile(w, r, finalPath)
	})

	const addr = ":8080"
	log.Info("Serving static NextJS application", "directory", root, "address", "http://127.0.0.1"+addr)
	http.ListenAndServe(addr, nil)
}
