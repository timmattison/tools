package main

import (
	"encoding/json"
	"github.com/charmbracelet/log"
	"golang.design/x/clipboard"
	_ "golang.design/x/clipboard"
	"strings"
	"time"
)

func runLoop() {
	var err error

	if err = clipboard.Init(); err != nil {
		log.Fatal(err)
	}

	var lastSeen string

	log.Info("Waiting for JSON in clipboard, press CTRL-C in this terminal to stop the program")

	for {
		select {
		case <-time.After(500 * time.Millisecond):
		}

		clipboardData := clipboard.Read(clipboard.FmtText)

		clipboardString := string(clipboardData)

		if lastSeen == clipboardString {
			continue
		}

		if !(strings.Contains(clipboardString, "{") ||
			strings.Contains(clipboardString, "}") ||
			strings.Contains(clipboardString, "[") ||
			strings.Contains(clipboardString, "]") ||
			strings.Contains(clipboardString, "\"")) {
			continue
		}

		var object interface{}

		if err = json.Unmarshal([]byte(clipboardString), &object); err != nil {
			// Don't try to parse this again
			lastSeen = clipboardString
			continue
		}

		var output []byte

		if output, err = json.MarshalIndent(object, "", "   "); err != nil {
			// Marshalling error, don't try to parse it again
			lastSeen = clipboardString
			continue
		}

		reformattedString := string(output)

		log.Info("Reformatted JSON in clipboard")
		clipboard.Write(clipboard.FmtText, []byte(reformattedString))
		lastSeen = reformattedString
	}
}

func main() {
	runLoop()
}
