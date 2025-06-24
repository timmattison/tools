#!/bin/bash

# Test script for safeboard using Python to generate Unicode characters

echo "Starting safeboard tests..."
echo "Make sure safeboard is running!"
echo

echo "Test 1: Normal text (should not trigger)"
python3 -c "print('Hello World', end='')" | pbcopy
sleep 2

echo "Test 2: Zero-width space (U+200B)"
python3 -c "print('Hello\u200bWorld', end='')" | pbcopy
sleep 2

echo "Test 3: Right-to-left override (U+202E)"
python3 -c "print('Hello\u202eWorld', end='')" | pbcopy
sleep 2

echo "Test 4: Private use area character (U+E000)"
python3 -c "print('Hello\ue000World', end='')" | pbcopy
sleep 2

echo "Test 5: Multiple dangerous characters"
python3 -c "print('Hello\u200b\u202e\ue000World', end='')" | pbcopy
sleep 2

echo "Test 6: Zero-width joiner (U+200D)"
python3 -c "print('Test\u200dString', end='')" | pbcopy
sleep 2

echo "Test 7: Left-to-right embedding (U+202A)"
python3 -c "print('Test\u202aString', end='')" | pbcopy
sleep 2

echo "Test 8: Zero-width no-break space (U+FEFF)"
python3 -c "print('Test\ufeffString', end='')" | pbcopy
sleep 2

echo "Test 9: Invisible separator (U+2063)"
python3 -c "print('Test\u2063String', end='')" | pbcopy
sleep 2

echo "Tests complete!"