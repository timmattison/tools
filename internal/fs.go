package internal

import (
	"github.com/charmbracelet/log"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

type NameChecker func(filename string) bool
type FileHandler func(fileInfo os.FileInfo)
type DirectoryHandler func(entry os.DirEntry)

func HasSuffixNameChecker(suffix string) NameChecker {
	return func(filename string) bool {
		return strings.HasSuffix(filename, suffix)
	}
}

func HasPrefixNameChecker(prefix string) NameChecker {
	return func(filename string) bool {
		return strings.HasPrefix(filename, prefix)
	}
}

func ContainsNameChecker(substring string) NameChecker {
	return func(filename string) bool {
		return strings.Contains(filename, substring)
	}
}

func CalculateDirSize(dirPath string) (int64, error) {
	var totalSize int64

	err := filepath.Walk(dirPath, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}

		var linfo os.FileInfo

		if linfo, err = os.Lstat(path); err != nil {
			return err
		}

		if linfo.Mode()&os.ModeSymlink != 0 {
			// It's a symlink; ignore it.
			return nil
		}

		if !info.IsDir() {
			totalSize += info.Size()
		}

		return nil
	})

	return totalSize, err
}

func VisitWithNameChecker(nameChecker NameChecker, fileHandler FileHandler, directoryHandler DirectoryHandler) fs.WalkDirFunc {
	return func(path string, dirEntry os.DirEntry, err error) error {
		if err != nil {
			log.Warn("Error visiting path", "path", path, "error", err)
			return nil
		}

		if dirEntry.IsDir() {
			if directoryHandler != nil {
				directoryHandler(dirEntry)
			}

			return nil
		}

		if (nameChecker != nil) && nameChecker(dirEntry.Name()) {
			var info os.FileInfo

			if info, err = dirEntry.Info(); err != nil {
				log.Fatal("Couldn't get file info", "error", err)
			}

			if fileHandler != nil {
				fileHandler(info)
			}
		}

		return nil
	}
}
