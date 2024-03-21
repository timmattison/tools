package internal

import (
	"fmt"
	"golang.org/x/text/language"
	"golang.org/x/text/message"
	"os"
)

// GetLocalePrinter returns a localized message printer
func GetLocalePrinter() *message.Printer {
	return message.NewPrinter(getUserLocale())
}

// PrettyPrintInt returns a localized string representation of the input integer
func PrettyPrintInt(input int64) string {
	return GetLocalePrinter().Sprintf("%d", input)
}

// PrettyPrintFloat returns a localized string representation of the input float
func PrettyPrintFloat(input float64, precision int) string {
	formatString := fmt.Sprintf("%%.%df", precision)

	return GetLocalePrinter().Sprintf(formatString, input)
}

// PrettyPrintBytes returns a localized string representation of the input byte count
func PrettyPrintBytes(bytes uint64) string {
	const unit = 1024

	suffix := ""
	div := int64(1)

	if bytes >= unit {
		var exp int

		div, exp = int64(unit), 0

		for n := bytes / unit; n >= unit; n /= unit {
			div *= unit
			exp++
		}

		const suffixes = "kMGTPE"

		if exp >= len(suffixes) {
			// More than exabytes, really?
			suffix = "OMFGz"
		} else {
			suffix = string(suffixes[exp])
		}
	}

	return fmt.Sprintf("%.1f %sB", float64(bytes)/float64(div), suffix)
}

func getUserLocale() language.Tag {
	// Get the preferred locale from the environment variables
	locale := os.Getenv("LC_ALL")

	if locale == "" {
		locale = os.Getenv("LC_MESSAGES")
	}

	if locale == "" {
		locale = os.Getenv("LANG")
	}

	if locale == "" {
		// Default fallback if no environment variable is set
		locale = "en_US.UTF-8"
	}

	// Parse the locale code
	tag, err := language.Parse(locale)

	if err != nil {
		// Fallback to default language if parsing failed
		return language.English
	}

	return tag
}
