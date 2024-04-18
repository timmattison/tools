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
		return m.Err.Error() + "\n"
	}

	indentAmount := 2
	widthWithoutIndent := m.WindowWidth - indentAmount*4

	if m.HashValue != "" {
		return wrap.String(m.HashValue+"  "+m.InputFilename+"\n", m.WindowWidth)
	}

	var output strings.Builder

	if m.InputFileInfo == nil {
		output.WriteString(wrap.String("Waiting to start hashing...", widthWithoutIndent))
	} else {
		percentComplete := float64(m.Position) / float64(m.InputFileInfo.Size())
		m.ProgressBar.Width = widthWithoutIndent

		HashingString := "Hashing "
		HashingString += m.InputFilename
		HashingString += " with "
		HashingString += m.HashType

		output.WriteString(wrap.String(HashingString, widthWithoutIndent))

		output.WriteString("\n\n")
		output.WriteString(m.ProgressBar.ViewAs(percentComplete))
		output.WriteString("\n\n")

		throughput := CalculateThroughput(m.StartTime, time.Now(), m.Position)

		positionString := "[ "
		positionString += m.Printer.Sprintf("%d", m.Position)
		positionString += " / "
		positionString += m.Printer.Sprintf("%d", m.InputFileInfo.Size())
		positionString += " ]"
		positionString += ThroughputString(m.Printer, throughput)

		output.WriteString(wrap.String(positionString, widthWithoutIndent))

		output.WriteString("\n\n")

		if m.Paused {
			output.WriteString(wrap.String("Paused  - press space to continue", widthWithoutIndent))
		} else {
			output.WriteString(wrap.String("Hashing - press space to pause", widthWithoutIndent))
		}

		output.WriteString("\n")
		output.WriteString(wrap.String("CTRL-C  - abort hash", widthWithoutIndent))
	}

	return indent.String(output.String(), uint(indentAmount))
}

func CalculateThroughput(startTime time.Time, endTime time.Time, processedBytes int64) int64 {
	if startTime.IsZero() {
		return -1
	}

	durationInMilliseconds := endTime.Sub(startTime).Milliseconds()
	throughput := int64(math.Round(float64(processedBytes) / float64(durationInMilliseconds) * 1000))

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
