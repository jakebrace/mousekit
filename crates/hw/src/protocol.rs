//! The HID++ 2.0 message format, as pure functions: building requests and
//! taking replies apart. No device access here, so all of it is unit-tested.
//!
//! A HID++ 2.0 device offers numbered "features" (DPI, report rate, ...).
//! Each device places them at its own positions, so the first step is always
//! asking the root feature where a feature lives. A request then names that
//! position (the feature index) and a function number within the feature.

/// Report ID of a 7-byte HID++ report.
pub const SHORT_REPORT: u8 = 0x10;
/// Report ID of a 20-byte HID++ report.
pub const LONG_REPORT: u8 = 0x11;
pub const LONG_REPORT_LENGTH: usize = 20;

/// Device index meaning "the device this node belongs to". The kernel
/// rewrites it to the real index for a mouse behind a wireless receiver.
const DEVICE_SELF: u8 = 0xff;

/// A tag we put in every request and get back in its reply, so our replies
/// can be told apart from those meant for other software talking to the
/// same mouse (the kernel uses 1; Solaar picks its own).
const SOFTWARE_ID: u8 = 0x0b;

/// The root feature is always at index 0. Its function 0 looks a feature up.
pub const ROOT_INDEX: u8 = 0x00;
pub const ROOT_GET_FEATURE: u8 = 0;

// Feature IDs and their functions.
pub const FEATURE_ADJUSTABLE_DPI: u16 = 0x2201;
pub const DPI_GET_LIST: u8 = 1;
pub const DPI_GET: u8 = 2;
pub const DPI_SET: u8 = 3;

pub const FEATURE_REPORT_RATE: u16 = 0x8060;
pub const RATE_GET_LIST: u8 = 0;
pub const RATE_GET: u8 = 1;
pub const RATE_SET: u8 = 2;

pub const FEATURE_ONBOARD_PROFILES: u16 = 0x8100;
pub const ONBOARD_SET_MODE: u8 = 1;
pub const ONBOARD_GET_MODE: u8 = 2;
/// Host mode: the host (us) controls DPI and report rate. The other mode,
/// 1, makes the mouse apply its stored onboard profile and ignore us.
pub const MODE_HOST: u8 = 2;

/// Builds a request as a long report. `params` may be up to 16 bytes.
pub fn request(feature_index: u8, function: u8, params: &[u8]) -> [u8; LONG_REPORT_LENGTH] {
    assert!(params.len() <= LONG_REPORT_LENGTH - 4 && function < 16);
    let mut report = [0u8; LONG_REPORT_LENGTH];
    report[0] = LONG_REPORT;
    report[1] = DEVICE_SELF;
    report[2] = feature_index;
    report[3] = (function << 4) | SOFTWARE_ID;
    report[4..4 + params.len()].copy_from_slice(params);
    report
}

/// What an incoming report means for a pending request.
#[derive(Debug, PartialEq, Eq)]
pub enum Reply<'a> {
    /// The answer: the bytes after the 4-byte header.
    Answer(&'a [u8]),
    /// The device refused the request, with its error code.
    Error(u8),
    /// Something else (mouse movement, a notification, another program's
    /// reply). Keep waiting.
    Unrelated,
}

/// Matches a report read from the device against the request we sent.
pub fn classify(report: &[u8], feature_index: u8, function: u8) -> Reply<'_> {
    if report.len() < 7 || !matches!(report[0], SHORT_REPORT | LONG_REPORT) {
        return Reply::Unrelated;
    }
    let tag = (function << 4) | SOFTWARE_ID;
    // An error reply echoes the request's feature index and tag after a
    // marker byte: 0xff for HID++ 2.0, 0x8f for the older 1.0 form.
    if matches!(report[2], 0xff | 0x8f) && report[3] == feature_index && report[4] == tag {
        return Reply::Error(report[5]);
    }
    if report[2] == feature_index && report[3] == tag {
        return Reply::Answer(&report[4..]);
    }
    Reply::Unrelated
}

/// A HID++ error code in words.
pub fn error_text(code: u8) -> &'static str {
    match code {
        1 => "unknown error",
        2 => "invalid argument",
        3 => "out of range",
        4 => "hardware error",
        5 => "not allowed",
        6 => "invalid feature index",
        7 => "invalid function",
        8 => "busy",
        9 => "unsupported",
        _ => "unrecognised error",
    }
}

/// The most DPI values we are willing to list for one sensor.
const MAX_DPI_VALUES: usize = 4096;

/// Decodes the DPI list of feature 0x2201 (the bytes after the sensor
/// index). Entries are 16-bit big-endian and the list ends with a zero.
/// An entry of `0xE000 + step` between two values means "every `step` from
/// the value before to the value after".
///
/// Returns the values in ascending order, or `None` if the list has not
/// ended yet (the caller should fetch the next part and append it).
pub fn parse_dpi_list(bytes: &[u8]) -> Option<Vec<u32>> {
    let mut values: Vec<u32> = Vec::new();
    let mut pending_step: Option<u32> = None;
    for pair in bytes.as_chunks::<2>().0 {
        let entry = u32::from(u16::from_be_bytes(*pair));
        if entry == 0 {
            values.sort_unstable();
            values.dedup();
            return Some(values);
        }
        if entry >= 0xe000 {
            pending_step = Some(entry - 0xe000);
            continue;
        }
        if let (Some(step), Some(&from)) = (pending_step.take(), values.last())
            && step > 0
        {
            let mut value = from + step;
            while value < entry && values.len() < MAX_DPI_VALUES {
                values.push(value);
                value += step;
            }
        }
        if values.len() < MAX_DPI_VALUES {
            values.push(entry);
        }
    }
    None
}

/// Decodes the report rate list of feature 0x8060: bit N set means an
/// interval of N+1 milliseconds is supported. Returned as rates in Hz,
/// ascending.
pub fn parse_rate_list(mask: u8) -> Vec<u32> {
    let mut rates: Vec<u32> = (0..8)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| 1000 / (bit + 1))
        .collect();
    rates.sort_unstable();
    rates
}

/// The report interval in milliseconds for a rate in Hz, if it is a whole
/// number from 1 to 8 (the only intervals the protocol has).
pub fn interval_ms(hz: u32) -> Option<u8> {
    (1..=8u8).find(|ms| 1000 / u32::from(*ms) == hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_have_the_documented_layout() {
        let report = request(7, DPI_SET, &[0x00, 0x06, 0x40]);
        assert_eq!(report.len(), 20);
        assert_eq!(report[..7], [0x11, 0xff, 7, 0x3b, 0x00, 0x06, 0x40]);
        assert!(report[7..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn replies_are_matched_by_feature_and_tag() {
        // The mouse answers with its real device index (1 here), not 0xff.
        let mut answer = [0u8; 20];
        answer[..7].copy_from_slice(&[0x11, 0x01, 7, 0x2b, 0x00, 0x03, 0xe8]);
        assert_eq!(classify(&answer, 7, DPI_GET), Reply::Answer(&answer[4..]));
        // Same feature, another program's tag.
        answer[3] = 0x21;
        assert_eq!(classify(&answer, 7, DPI_GET), Reply::Unrelated);
        // Mouse movement on the same node.
        assert_eq!(
            classify(&[0x02, 0, 1, 0, 0, 0, 0, 0], 7, DPI_GET),
            Reply::Unrelated
        );
        assert_eq!(classify(&[], 7, DPI_GET), Reply::Unrelated);
    }

    #[test]
    fn error_replies_carry_their_code() {
        let error = [
            0x11, 0x01, 0xff, 7, 0x3b, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(classify(&error, 7, DPI_SET), Reply::Error(2));
        assert_eq!(error_text(2), "invalid argument");
        let old_style = [0x10, 0x01, 0x8f, 7, 0x3b, 0x05, 0];
        assert_eq!(classify(&old_style, 7, DPI_SET), Reply::Error(5));
        // An error for a different request is not ours.
        assert_eq!(classify(&error, 7, DPI_GET), Reply::Unrelated);
    }

    #[test]
    fn dpi_lists_decode_ranges_and_plain_values() {
        // 100 to 25600 in steps of 50, the shape a G502 X reports.
        let range = [0x00, 0x64, 0xe0, 0x32, 0x64, 0x00, 0x00, 0x00];
        let values = parse_dpi_list(&range).unwrap();
        assert_eq!(values.first(), Some(&100));
        assert_eq!(values.get(1), Some(&150));
        assert_eq!(values.last(), Some(&25600));
        assert_eq!(values.len(), 511);

        let plain = [0x01, 0x90, 0x03, 0x20, 0x06, 0x40, 0x00, 0x00];
        assert_eq!(parse_dpi_list(&plain), Some(vec![400, 800, 1600]));

        // No terminator yet: more to fetch.
        assert_eq!(parse_dpi_list(&[0x01, 0x90, 0x03, 0x20]), None);
    }

    #[test]
    fn report_rates_decode_from_the_bit_mask() {
        // Bits 0, 1, 3, 7: 1 ms, 2 ms, 4 ms and 8 ms.
        assert_eq!(parse_rate_list(0b1000_1011), [125, 250, 500, 1000]);
        assert_eq!(interval_ms(1000), Some(1));
        assert_eq!(interval_ms(500), Some(2));
        assert_eq!(interval_ms(125), Some(8));
        assert_eq!(interval_ms(300), None);
    }
}
