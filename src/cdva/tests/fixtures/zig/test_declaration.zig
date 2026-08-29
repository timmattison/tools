const std = @import("std");

/// Adds two numbers.
pub fn add(a: i32, b: i32) i32 {
    return a + b;
}

test "add sums its arguments" {
    try std.testing.expectEqual(@as(i32, 3), add(1, 2));
}
