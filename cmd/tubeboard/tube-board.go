package main

import (
	"github.com/charmbracelet/log"
	"golang.design/x/clipboard"
	"net/url"
	"strings"
	"time"
)

func runLoop() {
	var err error

	if err = clipboard.Init(); err != nil {
		log.Fatal(err)
	}

	var lastSeen string

	log.Info("Waiting for YouTube URLs in clipboard, press CTRL-C in this terminal to stop the program")

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

		// Sample URLs:
		// - https://www.youtube.com/watch?v=xWWHaGjBNFs&pp=ygUWbG9uZG9uIHdhbGtpbmcgdG91ciA0aw%3D%3D
		// - https://www.youtube.com/watch?v=RgRGlFpqoH8&pp=ygUWbG9uZG9uIHdhbGtpbmcgdG91ciA0aw%3D%3D

		if !(strings.Contains(clipboardString, "youtube.com")) {
			continue
		}

		var parsedURL *url.URL

		if parsedURL, err = url.Parse(clipboardString); err != nil {
			continue
		}

		var contentId string

		if strings.Contains(clipboardString, "watch?v=") {
			// Get query parameters
			params := parsedURL.Query()

			// Get specific parameters
			contentId = params.Get("v")

			if contentId == "" {
				continue
			}
		} else if strings.Contains(clipboardString, "/live/") {
			// Get path segments
			pathSegments := strings.Split(parsedURL.Path, "/")

			// Get the last path segment
			contentId = pathSegments[len(pathSegments)-1]

			if contentId == "" {
				continue
			}
		}

		log.Info("Content ID placed in clipboard", "contentId", contentId)
		clipboard.Write(clipboard.FmtText, []byte(contentId))
		lastSeen = contentId
	}
}

func main() {
	runLoop()
}
