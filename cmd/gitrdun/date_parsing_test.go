package main

import (
	"testing"
	"time"
)

func TestParseRelativeTimeString(t *testing.T) {
	now := time.Now()

	tests := []struct {
		input       string
		checkResult func(time.Time) bool
		expectError bool
	}{
		{
			"yesterday",
			func(result time.Time) bool {
				// Should be roughly 24 hours ago
				diff := now.Sub(result)
				return diff > 23*time.Hour && diff < 25*time.Hour
			},
			false,
		},
		{
			"last week",
			func(result time.Time) bool {
				// Should be roughly 7 days ago
				diff := now.Sub(result)
				return diff > 6*24*time.Hour && diff < 8*24*time.Hour
			},
			false,
		},
		{
			"tomorrow",
			func(result time.Time) bool {
				// Should be roughly 24 hours in the future
				diff := result.Sub(now)
				return diff > 23*time.Hour && diff < 25*time.Hour
			},
			false,
		},
		{
			"invalid relative time",
			nil,
			true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result, err := parseTimeString(tt.input)

			if (err != nil) != tt.expectError {
				t.Errorf("parseTimeString(%q) error = %v, expectError %v", tt.input, err, tt.expectError)
				return
			}

			if !tt.expectError && !tt.checkResult(result) {
				t.Errorf("parseTimeString(%q) = %v, not within expected range", tt.input, result)
			}
		})
	}
}

func TestTimeOfDayHandling(t *testing.T) {
	now := time.Now()

	tests := []struct {
		input       string
		checkResult func(time.Time) bool
		expectError bool
	}{
		{
			"12:00",
			func(result time.Time) bool {
				// Should be today or tomorrow at 12:00
				return result.Hour() == 12 && result.Minute() == 0 &&
					(result.Day() == now.Day() || result.Day() == now.Add(24*time.Hour).Day())
			},
			false,
		},
		{
			"23:59",
			func(result time.Time) bool {
				// Should be today or tomorrow at 23:59
				return result.Hour() == 23 && result.Minute() == 59 &&
					(result.Day() == now.Day() || result.Day() == now.Add(24*time.Hour).Day())
			},
			false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result, err := parseTimeString(tt.input)

			if (err != nil) != tt.expectError {
				t.Errorf("parseTimeString(%q) error = %v, expectError %v", tt.input, err, tt.expectError)
				return
			}

			if !tt.expectError && !tt.checkResult(result) {
				t.Errorf("parseTimeString(%q) = %v, not within expected range", tt.input, result)
			}

			// Check that the time is in the future
			if !tt.expectError && !result.After(now) {
				t.Errorf("parseTimeString(%q) = %v, should be in the future", tt.input, result)
			}
		})
	}
}

func TestEndTimeHandling(t *testing.T) {
	// Test that when both start and end times are specified, they're handled correctly
	startStr := "two days ago"
	endStr := "yesterday"

	startDuration, err := parseDuration(startStr)
	if err != nil {
		t.Fatalf("Failed to parse start duration: %v", err)
	}

	thresholdTime := time.Now().Add(-startDuration)

	endTime, err := parseTimeString(endStr)
	if err != nil {
		t.Fatalf("Failed to parse end time: %v", err)
	}

	// The threshold time should be before the end time
	if !thresholdTime.Before(endTime) {
		t.Errorf("Threshold time %v should be before end time %v", thresholdTime, endTime)
	}

	// The end time should be in the past
	if !endTime.Before(time.Now()) {
		t.Errorf("End time %v should be in the past", endTime)
	}
}
