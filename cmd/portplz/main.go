package main

import (
	"crypto/sha256"
	"github.com/charmbracelet/log"
	"os"
	"path/filepath"
)

func unprivilegedPortNumberFromString(input string) uint16 {
	hash := sha256.New()
	hash.Write([]byte(input))
	hashBytes := hash.Sum(nil)

	hashBytes = hashBytes[:2]

	var hashUint16 uint16
	hashUint16 = uint16(hashBytes[0])<<8 + uint16(hashBytes[1])

	for hashUint16 < 1024 {
		hashUint16 += 1024
		hashUint16 %= 65535
	}

	return hashUint16
}

func main() {
	var cwd string
	var err error

	if cwd, err = os.Getwd(); err != nil {
		log.Fatal("Couldn't get the current working directory", "error", err)
	}

	baseCwd := filepath.Base(cwd)
	hashUint16 := unprivilegedPortNumberFromString(baseCwd)

	log.Info("Port", "port", hashUint16, "directory", baseCwd)
}
