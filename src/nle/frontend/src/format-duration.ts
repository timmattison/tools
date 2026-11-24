/** Tenths of a second in one minute. */
const TENTHS_PER_MINUTE = 600;

/** Tenths of a second in one hour. */
const TENTHS_PER_HOUR = 36000;

/**
 * Formats a duration in seconds as "Xm Y.Ys", or as "Xh Xm Y.Ys" from one hour.
 *
 * The function rounds to tenths of a second before it splits the duration into
 * hours, minutes and seconds, so 59.96 is "1m 0.0s" and never "0m 60.0s".
 *
 * @param totalSeconds - The duration in seconds.
 * @returns The formatted duration.
 */
export function formatDuration(totalSeconds: number): string {
  const tenths = Math.round(totalSeconds * 10);
  const hours = String(Math.floor(tenths / TENTHS_PER_HOUR));
  const minutes = String(
    Math.floor((tenths % TENTHS_PER_HOUR) / TENTHS_PER_MINUTE),
  );
  const seconds = ((tenths % TENTHS_PER_MINUTE) / 10).toFixed(1);
  return tenths >= TENTHS_PER_HOUR
    ? `${hours}h ${minutes}m ${seconds}s`
    : `${minutes}m ${seconds}s`;
}
