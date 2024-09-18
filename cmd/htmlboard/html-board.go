package main

import (
	"github.com/charmbracelet/log"
	"github.com/yosssi/gohtml"
	"golang.design/x/clipboard"
	"golang.org/x/net/html"
	"strings"
	"time"
)

func runLoop() {
	var err error

	if err = clipboard.Init(); err != nil {
		log.Fatal(err)
	}

	var lastSeen string

	log.Info("Waiting for HTML in clipboard, press CTRL-C in this terminal to stop the program")

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

		if !strings.Contains(clipboardString, "<") ||
			!strings.Contains(clipboardString, ">") {
			continue
		}

		if _, err = html.Parse(strings.NewReader(clipboardString)); err != nil {
			continue
		}

		reformattedString := gohtml.Format(clipboardString)

		log.Info("Reformatted HTML in clipboard")
		clipboard.Write(clipboard.FmtText, []byte(reformattedString))
		lastSeen = reformattedString
	}
}

func main() {
	runLoop()
}
