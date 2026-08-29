const std = @import("std");

/// Returns a greeting.
pub fn greet() []const u8 {
    return "こんにちは";
}

test "greet says こんにちは" {
    try std.testing.expectEqualStrings("こんにちは", greet());
}
