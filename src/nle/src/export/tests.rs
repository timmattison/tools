//! Tests for the export of an edited project.

use super::*;

#[test]
fn parse_progress_line_reads_a_frame_line_and_refuses_other_lines() {
    assert_eq!(
        parse_progress_line("Processed frame 31 / 300"),
        Some(ExportProgress::Frames {
            done: 31,
            total: 300
        })
    );
    assert_eq!(
        parse_progress_line("Processed frame 5 / 10 \r"),
        Some(ExportProgress::Frames { done: 5, total: 10 })
    );
    assert_eq!(parse_progress_line("Video export saved to: x.mp4"), None);
    assert_eq!(parse_progress_line("Processed frame 5 / 0"), None);
    assert_eq!(parse_progress_line("Processed frame 7 / 3"), None);
    assert_eq!(parse_progress_line("Processed frame x / 3"), None);
}
