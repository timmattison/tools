package internal

import (
	"os"
	"path/filepath"
)

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
