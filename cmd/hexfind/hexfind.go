package main

import (
	"encoding/hex"
	"flag"
	"fmt"
	"os"
	"strings"

	"github.com/edsrzf/mmap-go"
)

func main() {
	var contextBytes = flag.Int("context", 16, "Number of bytes to show before and after the match")
	var allMatches = flag.Bool("all", false, "Show all matches instead of just the first one")

	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, "Usage: %s [options] <hex-string> <file>\n\n", os.Args[0])
		fmt.Fprintf(os.Stderr, "Search for a hex string in a binary file and display a hex dump with surrounding bytes.\n\n")
		fmt.Fprintf(os.Stderr, "Options:\n")
		flag.PrintDefaults()
		fmt.Fprintf(os.Stderr, "\nExample: hexfind 0xf9beb4d9 bitcoin_block.dat\n")
		fmt.Fprintf(os.Stderr, "         hexfind f9beb4d9 bitcoin_block.dat\n")
	}

	flag.Parse()

	if flag.NArg() != 2 {
		flag.Usage()
		os.Exit(1)
	}

	// Get the hex string to search for
	hexString := flag.Arg(0)
	// Remove 0x prefix if present
	hexString = strings.TrimPrefix(hexString, "0x")

	// Decode the hex string
	pattern, err := hex.DecodeString(hexString)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error decoding hex string: %v\n", err)
		os.Exit(1)
	}

	// Get the file to search
	filename := flag.Arg(1)
	file, err := os.Open(filename)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error opening file: %v\n", err)
		os.Exit(1)
	}
	defer file.Close()

	// Search for the pattern
	matches, err := findPattern(file, pattern, *contextBytes, *allMatches)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error searching file: %v\n", err)
		os.Exit(1)
	}

	if len(matches) == 0 {
		fmt.Printf("Pattern '%s' not found in file '%s'\n", hexString, filename)
		os.Exit(0)
	}

	// Display the matches
	fmt.Printf("Found %d match(es) for pattern '%s' in file '%s'\n\n", len(matches), hexString, filename)
	for i, match := range matches {
		fmt.Printf("Match #%d:\n", i+1)
		fmt.Printf("Offset: 0x%08x (%d decimal)\n", match.offset, match.offset)
		displayHexDump(match.data, match.offset, *contextBytes, len(pattern))
		fmt.Println()
	}
}

type Match struct {
	offset int64
	data   []byte
}

func findPattern(file *os.File, pattern []byte, contextBytes int, allMatches bool) ([]Match, error) {
	var matches []Match
	patternLen := len(pattern)
	bufSize := patternLen + contextBytes*2 // Context before + pattern + context after

	// Memory map the file
	mmapped, err := mmap.Map(file, mmap.RDONLY, 0)
	if err != nil {
		return nil, fmt.Errorf("error memory mapping file: %v", err)
	}
	defer mmapped.Unmap() // Ensure we unmap when done

	fileSize := len(mmapped)

	// Search for pattern in the memory mapped file
	for i := 0; i <= fileSize-patternLen; i++ {
		match := true
		for j := 0; j < patternLen; j++ {
			if mmapped[i+j] != pattern[j] {
				match = false
				break
			}
		}

		if match {
			matchOffset := int64(i)

			// Calculate the start of the context
			contextStart := matchOffset - int64(contextBytes)
			if contextStart < 0 {
				contextStart = 0
			}

			// Calculate how many bytes to read for context
			bytesToRead := bufSize
			if int(contextStart)+bytesToRead > fileSize {
				bytesToRead = fileSize - int(contextStart)
			}

			// Create a copy of the data with context
			matchData := make([]byte, bytesToRead)
			copy(matchData, mmapped[contextStart:int(contextStart)+bytesToRead])

			// Add match to results
			matches = append(matches, Match{
				offset: matchOffset,
				data:   matchData,
			})

			if !allMatches {
				return matches, nil
			}
		}
	}

	return matches, nil
}

func displayHexDump(data []byte, fileOffset int64, contextBytes int, patternLen int) {
	// Calculate the offset of the first byte in the data relative to the file
	startOffset := fileOffset - int64(contextBytes)
	if startOffset < 0 {
		startOffset = 0
	}

	// Calculate the position of the pattern in the data
	patternPos := int(fileOffset - startOffset)

	// Display the hex dump
	for i := 0; i < len(data); i += 16 {
		// Print offset
		fmt.Printf("%08x: ", startOffset+int64(i))

		// Print hex values
		for j := 0; j < 16; j++ {
			if i+j < len(data) {
				// Highlight the pattern bytes
				inPattern := i+j >= patternPos && i+j < patternPos+patternLen
				if inPattern {
					fmt.Printf("\033[1;31m%02x\033[0m ", data[i+j]) // Red and bold
				} else {
					fmt.Printf("%02x ", data[i+j])
				}
			} else {
				fmt.Print("   ")
			}

			// Add extra space in the middle
			if j == 7 {
				fmt.Print(" ")
			}
		}

		// Print ASCII representation
		fmt.Print(" |")
		for j := 0; j < 16; j++ {
			if i+j < len(data) {
				// Highlight the pattern bytes
				inPattern := i+j >= patternPos && i+j < patternPos+patternLen

				// Print printable ASCII characters, replace others with a dot
				if data[i+j] >= 32 && data[i+j] <= 126 {
					if inPattern {
						fmt.Printf("\033[1;31m%c\033[0m", data[i+j]) // Red and bold
					} else {
						fmt.Printf("%c", data[i+j])
					}
				} else {
					if inPattern {
						fmt.Print("\033[1;31m.\033[0m")
					} else {
						fmt.Print(".")
					}
				}
			} else {
				fmt.Print(" ")
			}
		}
		fmt.Print("|\n")
	}
}
