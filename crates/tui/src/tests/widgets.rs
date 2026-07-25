use super::*;

// --- human_size ---

#[test]
fn human_size_reports_plain_bytes_below_a_kilobyte() {
    assert_eq!(human_size(0), "0B");
    assert_eq!(human_size(999), "999B");
    assert_eq!(human_size(1023), "1023B");
}

#[test]
fn human_size_switches_unit_at_each_power_of_1024() {
    assert_eq!(human_size(1024), "1.0K");
    assert_eq!(human_size(1024 * 1024), "1.0M");
    assert_eq!(human_size(1024 * 1024 * 1024), "1.0G");
}

#[test]
fn human_size_drops_the_decimal_once_it_stops_adding_information() {
    assert_eq!(human_size(1536), "1.5K");
    assert_eq!(human_size(20 * 1024), "20K");
}

#[test]
fn human_size_saturates_at_the_largest_unit() {
    let huge = 5_u64 * 1024 * 1024 * 1024 * 1024 * 1024;
    assert!(huge > u64::from(u32::MAX));
    assert!(human_size(huge).ends_with('T'));
}

// --- pad_or_truncate ---

#[test]
fn pad_or_truncate_pads_short_text_to_the_full_width() {
    assert_eq!(pad_or_truncate("ab", 5), "ab   ");
}

#[test]
fn pad_or_truncate_leaves_exact_width_text_alone() {
    assert_eq!(pad_or_truncate("abcde", 5), "abcde");
}

#[test]
fn pad_or_truncate_elides_the_middle_and_keeps_the_width() {
    let result = pad_or_truncate("a-very-long-file-name.txt", 12);
    assert_eq!(result.chars().count(), 12);
    assert!(result.contains('…'));
    // The extension survives, which is the point of eliding the middle rather than the tail.
    assert!(result.ends_with(".txt"));
}

#[test]
fn pad_or_truncate_counts_characters_not_bytes() {
    // Multi-byte input would panic on a byte-indexed slice.
    let result = pad_or_truncate("ünïcödé-näme-with-áccents.txt", 10);
    assert_eq!(result.chars().count(), 10);
}

#[test]
fn pad_or_truncate_handles_degenerate_widths() {
    assert_eq!(pad_or_truncate("abcdef", 1), "…");
    assert_eq!(pad_or_truncate("abcdef", 0), "");
}
