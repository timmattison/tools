package main

import (
	"flag"
	"fmt"
	"image"
	"image/png"
	"io/ioutil"
	"math"
	"os"
	"path/filepath"
	"strings"

	"github.com/yeqown/go-qrcode/v2"
	"github.com/yeqown/go-qrcode/writer/standard"
)

func main() {
	// Define command-line flags
	var resolution = flag.Int("resolution", 1024, "Resolution of the QR code image (width and height in pixels)")
	var logoFile = flag.String("logo", "", "Path to an image file to use as a logo in the center of the QR code")
	var logoSize = flag.Float64("logo-size", 10.0, "Size of the logo as a percentage of the QR code (1-100, default 10%)")
	var ssid = flag.String("ssid", "", "WiFi network name (SSID)")
	var password = flag.String("password", "", "WiFi network password")

	// Custom usage message
	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, "Usage: %s [options]\n\n", os.Args[0])
		fmt.Fprintf(os.Stderr, "Generate a QR code for WiFi network access.\n\n")
		fmt.Fprintf(os.Stderr, "Options:\n")
		flag.PrintDefaults()
		fmt.Fprintf(os.Stderr, "\nExamples:\n")
		fmt.Fprintf(os.Stderr, "  wifiqr -ssid MyWiFiNetwork -password MySecretPassword\n")
		fmt.Fprintf(os.Stderr, "  wifiqr -resolution 512 -ssid MyWiFiNetwork -password MySecretPassword\n")
		fmt.Fprintf(os.Stderr, "  wifiqr -logo company_logo.png -ssid MyWiFiNetwork -password MySecretPassword\n")
		fmt.Fprintf(os.Stderr, "  wifiqr -logo company_logo.png -logo-size 20 -ssid MyWiFiNetwork -password MySecretPassword\n")
	}

	flag.Parse()

	// Check if SSID and password are provided
	if *ssid == "" || *password == "" {
		fmt.Fprintf(os.Stderr, "Error: Both SSID and password are required.\n\n")
		flag.Usage()
		os.Exit(1)
	}

	// Generate QR code
	err := generateWiFiQRCode(*ssid, *password, *resolution, *logoFile, *logoSize)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error generating QR code: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("QR code generated successfully: %s.png\n", *ssid)
}

func generateWiFiQRCode(ssid, password string, resolution int, logoFile string, logoSize float64) error {
	// Format the WiFi connection string
	// Format: WIFI:S:<SSID>;T:WPA;P:<PASSWORD>;;
	wifiString := fmt.Sprintf("WIFI:S:%s;T:WPA;P:%s;;", escapeSpecialChars(ssid), escapeSpecialChars(password))

	// Create a new QR code with high error correction level
	qrc, err := qrcode.NewWith(
		wifiString,
		qrcode.WithErrorCorrectionLevel(qrcode.ErrorCorrectionHighest),
	)
	if err != nil {
		return fmt.Errorf("failed to create QR code: %w", err)
	}

	// Create the final output file name
	outputFile := fmt.Sprintf("%s.png", ssid)

	// Ensure the directory exists
	dir := filepath.Dir(outputFile)
	if dir != "" && dir != "." {
		if err := os.MkdirAll(dir, 0755); err != nil {
			return fmt.Errorf("failed to create directory: %w", err)
		}
	}

	// Create a temporary file for the initial QR code
	tempFile, err := ioutil.TempFile("", "qrcode-*.png")
	if err != nil {
		return fmt.Errorf("failed to create temporary file: %w", err)
	}
	tempFileName := tempFile.Name()
	tempFile.Close()              // Close the file so it can be written to by the QR code writer
	defer os.Remove(tempFileName) // Clean up the temporary file when done

	// Create a writer to output the QR code as PNG
	// Note: The standard library only supports QR width up to 255 (uint8)
	// We'll use the maximum size possible within this constraint
	qrWidth := uint8(255) // Maximum value for uint8

	// Store the original requested resolution for later use
	originalResolution := resolution

	// If the requested resolution is within the library's limit, use it directly
	// This will ensure the logo size is properly applied without resizing
	if resolution <= 255 {
		qrWidth = uint8(resolution)
		// Set resolution to 0 to skip the resizing step later
		resolution = 0
	}

	// Create writer options
	var writerOptions []standard.ImageOption
	writerOptions = append(writerOptions, standard.WithQRWidth(qrWidth))
	writerOptions = append(writerOptions, standard.WithBuiltinImageEncoder(standard.PNG_FORMAT))

	// Add logo if specified
	if logoFile != "" {
		// Calculate the QR code size to use for scaling the logo
		qrSize := calculateQRSize(originalResolution, int(qrWidth))

		// Open and decode the logo image, scaling it to the appropriate size
		logoImg, err := loadImage(logoFile, qrSize, logoSize)
		if err != nil {
			return fmt.Errorf("failed to load logo image: %w", err)
		}

		// Add logo image option
		writerOptions = append(writerOptions, standard.WithLogoImage(logoImg))

		// Note: We're not using WithLogoSizeMultiplier anymore because we're already
		// scaling the logo in the loadImage function. Using both would result in double scaling.
	}

	// Create the writer with all options, writing to the temporary file
	w, err := standard.New(tempFileName, writerOptions...)
	if err != nil {
		return fmt.Errorf("failed to create QR code writer: %w", err)
	}

	// Save the QR code to the temporary file
	if err = qrc.Save(w); err != nil {
		return fmt.Errorf("failed to save QR code: %w", err)
	}

	// Now resize the image to the requested resolution if needed
	if resolution > 0 {
		if err = resizeImage(tempFileName, outputFile, originalResolution); err != nil {
			return fmt.Errorf("failed to resize QR code image: %w", err)
		}
	} else {
		// No resizing needed, just copy the file
		if err = copyFile(tempFileName, outputFile); err != nil {
			return fmt.Errorf("failed to copy QR code image: %w", err)
		}
	}

	return nil
}

// calculateQRSize determines the effective QR code size for logo scaling
// It estimates the actual source width of the QR code image based on the requested resolution
// The QR code library generates an image that's much larger than the requested resolution
// (approximately 10x larger), so we need to account for this when calculating the logo size
func calculateQRSize(originalResolution, maxQRWidth int) int {
	// Based on observations, the actual source width is approximately 10x the requested resolution
	// This ensures the logo is sized correctly relative to the actual QR code size
	return originalResolution * 10
}

// calculateMaxLogoSize determines the maximum allowed logo size based on the QR code size
// The library documentation states that the logo should be at most 1/5 of the QR code width
func calculateMaxLogoSize(qrSize int) int {
	return qrSize / 5
}

// calculateDesiredLogoSize determines the desired logo size based on the maximum allowed size and percentage
// For example, 100% means the logo will be the maximum allowed size (1/5 of QR code)
// 50% means the logo will be half of the maximum allowed size (1/10 of QR code)
// Any value greater than 100% is capped at 100% to ensure the logo is not too large
func calculateDesiredLogoSize(maxLogoSize int, logoSizePercent float64) int {
	// Cap the logo size percentage at 100%
	if logoSizePercent > 100.0 {
		logoSizePercent = 100.0
	}

	desiredLogoSize := int(float64(maxLogoSize) * logoSizePercent / 100.0)

	// Ensure the logo size is at least 1 pixel
	if desiredLogoSize < 1 {
		desiredLogoSize = 1
	}

	return desiredLogoSize
}

// calculateLogoSizeMultiplier determines the multiplier to use with the library's WithLogoSizeMultiplier option
// A higher multiplier means a smaller logo, so we need to invert the percentage
func calculateLogoSizeMultiplier(logoSizePercent float64) int {
	if logoSizePercent <= 0 || logoSizePercent > 100 {
		return 10 // Default to 10 (10% size) if percentage is invalid
	}

	// Use floating-point division and rounding to get the nearest integer value
	// This provides more precise control over the logo size across all percentage values
	return int(100.0/logoSizePercent + 0.5) // Adding 0.5 and truncating is equivalent to rounding
}

// loadImage loads an image from a file path, ensures it's square, and scales it to the appropriate size
// based on the QR code size and desired logo size percentage
func loadImage(filePath string, qrSize int, logoSizePercent float64) (image.Image, error) {
	file, err := os.Open(filePath)
	if err != nil {
		return nil, fmt.Errorf("failed to open image file: %w", err)
	}
	defer file.Close()

	// Decode the image
	img, _, err := image.Decode(file)
	if err != nil {
		return nil, fmt.Errorf("failed to decode image: %w", err)
	}

	// Get original logo dimensions
	bounds := img.Bounds()
	originalWidth := bounds.Max.X - bounds.Min.X
	originalHeight := bounds.Max.Y - bounds.Min.Y

	// Calculate the maximum and desired logo sizes
	maxLogoSize := calculateMaxLogoSize(qrSize)
	desiredLogoSize := calculateDesiredLogoSize(maxLogoSize, logoSizePercent)

	// Create a square version of the image with the desired size
	// Using nearest-neighbor algorithm to maintain consistency with QR code resizing
	scaledImg := nearestNeighborResize(img, desiredLogoSize, desiredLogoSize)

	// Print logo dimensions for debugging
	fmt.Printf("Logo dimensions: original width=%d, original height=%d, qrSize=%d, maxLogoSize=%d, desiredLogoSize=%d\n",
		originalWidth, originalHeight, qrSize, maxLogoSize, desiredLogoSize)

	return scaledImg, nil
}

// escapeSpecialChars escapes special characters in SSID and password
// as per the WiFi QR code specification
func escapeSpecialChars(input string) string {
	// Escape semicolons, colons, and backslashes with a backslash
	escaped := strings.ReplaceAll(input, "\\", "\\\\")
	escaped = strings.ReplaceAll(escaped, ";", "\\;")
	escaped = strings.ReplaceAll(escaped, ":", "\\:")

	return escaped
}

// copyFile copies a file from sourcePath to destPath
func copyFile(sourcePath, destPath string) error {
	// Open the source file to get its dimensions
	sourceFile, err := os.Open(sourcePath)
	if err != nil {
		return fmt.Errorf("failed to open source file: %w", err)
	}

	// Decode the source image to get its dimensions
	sourceImg, err := png.Decode(sourceFile)
	if err != nil {
		sourceFile.Close()
		return fmt.Errorf("failed to decode source image: %w", err)
	}
	sourceFile.Close()

	// Get source image dimensions
	sourceBounds := sourceImg.Bounds()
	sourceWidth := sourceBounds.Max.X - sourceBounds.Min.X
	sourceHeight := sourceBounds.Max.Y - sourceBounds.Min.Y

	// Read the source file
	sourceData, err := ioutil.ReadFile(sourcePath)
	if err != nil {
		return fmt.Errorf("failed to read source file: %w", err)
	}

	// Write to the destination file
	if err := ioutil.WriteFile(destPath, sourceData, 0644); err != nil {
		return fmt.Errorf("failed to write destination file: %w", err)
	}

	// Print output file dimensions for debugging
	fmt.Printf("Output file dimensions (no resize): width=%d, height=%d\n",
		sourceWidth, sourceHeight)

	return nil
}

// nearestNeighborResize resizes an image using the nearest-neighbor algorithm
// This preserves sharp edges and is ideal for QR codes
func nearestNeighborResize(img image.Image, width, height int) image.Image {
	// Create a new RGBA image with the desired dimensions
	dst := image.NewRGBA(image.Rect(0, 0, width, height))

	// Get the bounds of the source image
	bounds := img.Bounds()
	srcWidth := bounds.Max.X - bounds.Min.X
	srcHeight := bounds.Max.Y - bounds.Min.Y

	// Calculate the scaling factors
	xRatio := float64(srcWidth) / float64(width)
	yRatio := float64(srcHeight) / float64(height)

	// Iterate through each pixel in the destination image
	for y := 0; y < height; y++ {
		for x := 0; x < width; x++ {
			// Find the corresponding pixel in the source image
			srcX := int(math.Floor(float64(x) * xRatio))
			srcY := int(math.Floor(float64(y) * yRatio))

			// Get the color from the source image
			c := img.At(srcX+bounds.Min.X, srcY+bounds.Min.Y)

			// Set the color in the destination image
			dst.Set(x, y, c)
		}
	}

	return dst
}

// resizeImage resizes the image at sourcePath to the specified resolution and saves it to destPath
func resizeImage(sourcePath, destPath string, resolution int) error {
	// Open the source image
	sourceFile, err := os.Open(sourcePath)
	if err != nil {
		return fmt.Errorf("failed to open source image: %w", err)
	}
	defer sourceFile.Close()

	// Decode the source image
	sourceImg, err := png.Decode(sourceFile)
	if err != nil {
		return fmt.Errorf("failed to decode source image: %w", err)
	}

	// Get source image dimensions
	sourceBounds := sourceImg.Bounds()
	sourceWidth := sourceBounds.Max.X - sourceBounds.Min.X
	sourceHeight := sourceBounds.Max.Y - sourceBounds.Min.Y

	// Resize the image to the requested resolution using nearest-neighbor algorithm
	// This maintains crisp edges for QR codes
	resizedImg := nearestNeighborResize(sourceImg, resolution, resolution)

	// Create the destination file
	destFile, err := os.Create(destPath)
	if err != nil {
		return fmt.Errorf("failed to create destination file: %w", err)
	}
	defer destFile.Close()

	// Encode the resized image as PNG
	if err := png.Encode(destFile, resizedImg); err != nil {
		return fmt.Errorf("failed to encode resized image: %w", err)
	}

	// Print output file dimensions for debugging
	fmt.Printf("Output file dimensions: source width=%d, source height=%d, resized width=%d, resized height=%d\n",
		sourceWidth, sourceHeight, resolution, resolution)

	return nil
}
