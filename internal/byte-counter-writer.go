package internal

import "io"

type ByteCounterWriter struct {
	Writer io.Writer
	count  int64
}

func (bcw *ByteCounterWriter) Write(p []byte) (int, error) {
	n, err := bcw.Writer.Write(p)
	bcw.count += int64(n)
	return n, err
}

func (bcw *ByteCounterWriter) Count() int64 {
	return bcw.count
}
