package main

import (
	"github.com/charmbracelet/log"
	"golang.design/x/clipboard"
	"strings"
	"time"
)

func runLoop() {
	var err error

	if err = clipboard.Init(); err != nil {
		log.Fatal(err)
	}

	var lastSeen string

	log.Info("Waiting for escaped text in clipboard, press CTRL-C in this terminal to stop the program")

	for {
		select {
		case <-time.After(500 * time.Millisecond):
		}

		clipboardData := clipboard.Read(clipboard.FmtText)

		clipboardString := string(clipboardData)

		if lastSeen == clipboardString {
			continue
		}

		lastSeen = clipboardString

		if !strings.Contains(clipboardString, "\\\"") {
			continue
		}

		log.Info("Unescaping string", "clipboardString", clipboardString)

		updatedString := strings.ReplaceAll(clipboardString, `\"`, `"`)

		if updatedString == clipboardString {
			continue
		}

		clipboard.Write(clipboard.FmtText, []byte(updatedString))

		lastSeen = updatedString
	}
}

func main() {
	runLoop()
}
