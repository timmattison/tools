package main

import "github.com/timmattison/tools/internal"

func main() {
	internal.RunCommandInRepoDirectoriesWithFile("go.mod", []string{"go", "get", "-u", "all"})
}
