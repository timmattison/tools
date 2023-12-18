package main_model

import (
	"github.com/muesli/reflow/indent"
	"github.com/muesli/reflow/wrap"
	"golang.org/x/text/message"
	"math"
	"strings"
	"time"
)

const (
	MB = 1_000_000
	KB = 1_000
)

func (m MainModel) View() string {
	if m.Err != nil {
		return m.Err.Error()
	}
	indentAmount := 2
	widthWithoutIndent := m.WindowWidth - indentAmount*4

	var output strings.Builder

	if m.SourceFileInfo == nil {
		output.WriteString(wrap.String("Waiting to start the file copy...", widthWithoutIndent))
	} else {
		percentComplete := float64(m.DestinationPosition) / float64(m.SourceFileInfo.Size())
		m.ProgressBar.Width = widthWithoutIndent

		copyingString := "Copying "
		copyingString += m.SourceFilename
		copyingString += " to "
		copyingString += m.DestinationFilename

		output.WriteString(wrap.String(copyingString, widthWithoutIndent))

		output.WriteString("\n\n")
		output.WriteString(m.ProgressBar.ViewAs(percentComplete))
		output.WriteString("\n\n")

		throughput := CalculateThroughput(m.StartTime, time.Now(), m.DestinationPosition)

		positionString := "[ "
		positionString += m.Printer.Sprintf("%d", m.DestinationPosition)
		positionString += " / "
		positionString += m.Printer.Sprintf("%d", m.SourceFileInfo.Size())
		positionString += " ]"
		positionString += ThroughputString(m.Printer, throughput)

		output.WriteString(wrap.String(positionString, widthWithoutIndent))

		output.WriteString("\n\n")

		if m.Paused {
			output.WriteString(wrap.String("Paused  - press space to continue", widthWithoutIndent))
		} else {
			output.WriteString(wrap.String("Copying - press space to pause", widthWithoutIndent))
		}

		output.WriteString("\n")
		output.WriteString(wrap.String("CTRL-C  - abort copy", widthWithoutIndent))
	}

	return indent.String(output.String(), uint(indentAmount))
}

func CalculateThroughput(startTime time.Time, endTime time.Time, transferredBytes int64) int64 {
	if startTime.IsZero() {
		return -1
	}

	durationInMilliseconds := endTime.Sub(startTime).Milliseconds()
	throughput := int64(math.Round(float64(transferredBytes) / float64(durationInMilliseconds) * 1000))

	return throughput
}

func ThroughputString(printer *message.Printer, throughput int64) string {
	if throughput == -1 {
		return "Unknown"
	} else if throughput > MB {
		return printer.Sprintf("%15d MB/s", throughput/MB)
	} else if throughput > KB {
		return printer.Sprintf("%15d KB/s", throughput/KB)
	}

	return printer.Sprintf("%15d B/s", throughput)
}
