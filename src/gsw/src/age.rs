//! Format durations as short human strings and pick a dim level by age.

use std::time::Duration;

/// How "fresh" an age is — drives display brightness.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AgeDim {
    /// `< 5m` — bright.
    Fresh,
    /// `< 1h` — normal.
    Recent,
    /// `< 1d` — dim.
    Aging,
    /// `>= 1d` — very dim.
    Stale,
}

/// Format a duration with two units, e.g. `5m23s`, `2h14m`, `3d12h`.
pub fn format_age_detailed(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d{}h", secs / 86400, (secs % 86400) / 3600)
    }
}

/// Classify an age into a display brightness bucket.
pub fn age_dim_level(age: Duration) -> AgeDim {
    let secs = age.as_secs();
    if secs < 60 * 5 {
        AgeDim::Fresh
    } else if secs < 60 * 60 {
        AgeDim::Recent
    } else if secs < 60 * 60 * 24 {
        AgeDim::Aging
    } else {
        AgeDim::Stale
    }
}

/// Minimum fraction of base brightness retained at the stale boundary and
/// beyond. The fade approaches this floor so commits never disappear into
/// the background.
pub const FADE_FLOOR: f32 = 0.30;

/// Age at which the fade reaches the dark floor and stops darkening further.
pub const FADE_DARKEST_AT: Duration = Duration::from_secs(2 * 60 * 60);

/// Continuous fade factor in `[0.0, 1.0]` for a commit `age`.
///
/// `0.0` means use the full base color; `1.0` means use the dark floor.
/// One smooth linear ramp from age=0 to [`FADE_DARKEST_AT`], then clamped
/// at the floor — no per-bucket checkpoints, just a single gradient.
pub fn age_fade_factor(age: Duration) -> f32 {
    let secs = age.as_secs_f32();
    let end = FADE_DARKEST_AT.as_secs_f32();
    (secs / end).clamp(0.0, 1.0)
}

/// Linearly interpolate `base` toward a dark floor by `factor` in `[0,1]`.
///
/// `factor = 0` returns `base` unchanged; `factor = 1` returns
/// `base * FADE_FLOOR` (rounded). Out-of-range factors are clamped.
pub fn fade_rgb(base: (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let f = factor.clamp(0.0, 1.0);
    let scale = 1.0 - f * (1.0 - FADE_FLOOR);
    // The cast is bounded: `scale` is in [FADE_FLOOR, 1.0] and `c` is u8, so
    // the product is in [0, 255]; the explicit clamp is belt-and-braces.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "value is clamped to [0.0, 255.0] before the cast"
    )]
    let scl = |c: u8| (f32::from(c) * scale).round().clamp(0.0, 255.0) as u8;
    (scl(base.0), scl(base.1), scl(base.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detailed_age_seconds_only() {
        assert_eq!(format_age_detailed(Duration::from_secs(0)), "0s");
        assert_eq!(format_age_detailed(Duration::from_secs(5)), "5s");
        assert_eq!(format_age_detailed(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn detailed_age_minutes_and_seconds() {
        assert_eq!(format_age_detailed(Duration::from_secs(60)), "1m0s");
        assert_eq!(format_age_detailed(Duration::from_secs(5 * 60 + 23)), "5m23s");
        assert_eq!(
            format_age_detailed(Duration::from_secs(59 * 60 + 59)),
            "59m59s",
        );
    }

    #[test]
    fn detailed_age_hours_and_minutes() {
        assert_eq!(format_age_detailed(Duration::from_secs(60 * 60)), "1h0m");
        assert_eq!(
            format_age_detailed(Duration::from_secs(2 * 3600 + 14 * 60)),
            "2h14m",
        );
        assert_eq!(
            format_age_detailed(Duration::from_secs(23 * 3600 + 59 * 60)),
            "23h59m",
        );
    }

    #[test]
    fn detailed_age_days_and_hours() {
        assert_eq!(format_age_detailed(Duration::from_secs(86400)), "1d0h");
        assert_eq!(
            format_age_detailed(Duration::from_secs(3 * 86400 + 12 * 3600)),
            "3d12h",
        );
    }

    #[test]
    fn dim_level_buckets_by_boundary() {
        assert_eq!(age_dim_level(Duration::from_secs(0)), AgeDim::Fresh);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 5 - 1)), AgeDim::Fresh);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 5)), AgeDim::Recent);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60 - 1)), AgeDim::Recent);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60)), AgeDim::Aging);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60 * 24 - 1)), AgeDim::Aging);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60 * 24)), AgeDim::Stale);
        assert_eq!(age_dim_level(Duration::from_secs(60 * 60 * 24 * 30)), AgeDim::Stale);
    }

    #[test]
    fn fade_factor_is_zero_at_age_zero() {
        // A just-authored commit should display at full base brightness.
        assert!(
            (age_fade_factor(Duration::from_secs(0)) - 0.0).abs() < 1e-6,
            "factor at age=0 should be exactly 0.0",
        );
    }

    #[test]
    fn fade_factor_midpoint_at_one_hour() {
        // With a single linear ramp from 0 to 2h, the halfway point is 1h.
        let one_hour = Duration::from_secs(60 * 60);
        let factor = age_fade_factor(one_hour);
        assert!(
            (factor - 0.5).abs() < 1e-6,
            "factor at 1h should be 0.5 on a 0..2h linear ramp, was {factor}",
        );
    }

    #[test]
    fn fade_factor_reaches_one_at_two_hours() {
        // At and beyond 2h the gradient hits its floor and stops darkening.
        let two_h = FADE_DARKEST_AT;
        assert!(
            (age_fade_factor(two_h) - 1.0).abs() < 1e-6,
            "factor at 2h should be 1.0, was {}",
            age_fade_factor(two_h),
        );
        let week = Duration::from_secs(60 * 60 * 24 * 7);
        assert!(
            (age_fade_factor(week) - 1.0).abs() < 1e-6,
            "factor past 2h should clamp at 1.0, was {}",
            age_fade_factor(week),
        );
    }

    #[test]
    fn fade_factor_increases_with_age() {
        // Walk minute by minute through the ramp and confirm every step
        // strictly increases until we hit the floor at 2h.
        let mut prev = age_fade_factor(Duration::from_secs(0));
        for m in 1..=120_u64 {
            let next = age_fade_factor(Duration::from_secs(m * 60));
            assert!(
                next > prev,
                "factor must strictly increase between minute {} and {}: prev={prev} next={next}",
                m - 1,
                m,
            );
            prev = next;
        }
    }

    #[test]
    fn fade_factor_per_minute_step_is_perceptible() {
        // A linear ramp over 2h moves by 1/120 ≈ 0.00833 per minute. With a
        // base channel of 200 and a floor of 30%, that's ~1.17 RGB units per
        // minute — perceptible to the eye and enough to read as continuous.
        let expected_step = 1.0 / 120.0;
        for m in 0..120_u64 {
            let a = age_fade_factor(Duration::from_secs(m * 60));
            let b = age_fade_factor(Duration::from_secs((m + 1) * 60));
            assert!(
                (b - a - expected_step).abs() < 1e-5,
                "per-minute step at minute {m} should be ~{expected_step}, got {} -> {} (delta {})",
                a,
                b,
                b - a,
            );
        }
    }

    #[test]
    fn fade_rgb_returns_base_at_zero() {
        // factor=0 means "no fade applied" — color comes out untouched.
        let base = (200, 150, 50);
        assert_eq!(fade_rgb(base, 0.0), base);
    }

    #[test]
    fn fade_rgb_hits_floor_at_factor_one() {
        // factor=1 should scale every channel to FADE_FLOOR * base, rounded.
        let base: (u8, u8, u8) = (200, 100, 50);
        let (r, g, b) = fade_rgb(base, 1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "c is u8 and FADE_FLOOR is in (0, 1), so the product is in [0, 255]"
        )]
        let expect = |c: u8| (f32::from(c) * FADE_FLOOR).round() as u8;
        assert_eq!(
            (r, g, b),
            (expect(base.0), expect(base.1), expect(base.2)),
            "factor=1 should scale every channel by FADE_FLOOR",
        );
    }

    #[test]
    fn fade_rgb_is_monotonic_per_channel() {
        // Each channel must monotonically decrease as factor grows — no
        // bumps, no overshoots. We sample a representative non-grey base so
        // any per-channel logic error (e.g. swapping channels) would show.
        let base = (255, 215, 80);
        let mut prev = fade_rgb(base, 0.0);
        for step in 1..=10 {
            let factor = step as f32 / 10.0;
            let now = fade_rgb(base, factor);
            assert!(
                now.0 <= prev.0 && now.1 <= prev.1 && now.2 <= prev.2,
                "channels must not brighten as factor increases ({prev:?} -> {now:?} at factor={factor})",
            );
            prev = now;
        }
    }

    #[test]
    fn fade_rgb_clamps_out_of_range_factors() {
        // Defensive: callers might hand in factors slightly outside [0,1]
        // through floating-point drift. We should clamp, not blow past the
        // floor (which would let colors disappear) or back past the base.
        let base = (200, 100, 50);
        let below = fade_rgb(base, -0.5);
        let above = fade_rgb(base, 1.5);
        assert_eq!(below, base, "negative factor should clamp to base");
        let floored = fade_rgb(base, 1.0);
        assert_eq!(above, floored, "factor > 1 should clamp to the floor color");
    }
}
