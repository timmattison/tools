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
