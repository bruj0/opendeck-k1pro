// Namespace must be unique, 2 characters, and match DeviceNamespace in manifest.json
pub const DEVICE_NAMESPACE: &str = "k1";

pub const ROW_COUNT: usize = 2;
pub const COL_COUNT: usize = 3;
#[allow(dead_code)]
pub const KEY_COUNT: usize = ROW_COUNT * COL_COUNT; // 6 display keys
pub const ENCODER_COUNT: usize = 3; // 3 knobs

/// Converts OpenDeck logical position (0..5) to physical hardware key code.
///
/// Physical layout:
///   Row 1: Pos 0 (Key 1), Pos 1 (Key 2), Pos 2 (Key 3)
///   Row 2: Pos 3 (Key 4), Pos 4 (Key 5), Pos 5 (Key 6)
///
/// Hardware mapping from K1Pro.py:
///   Key 1: 0x05, Key 2: 0x03, Key 3: 0x01
///   Key 4: 0x06, Key 5: 0x04, Key 6: 0x02
pub fn opendeck_to_device(position: u8) -> Option<u8> {
    match position {
        0 => Some(0x05),
        1 => Some(0x03),
        2 => Some(0x01),
        3 => Some(0x06),
        4 => Some(0x04),
        5 => Some(0x02),
        _ => None,
    }
}

/// Converts physical hardware key code to OpenDeck logical position.
pub fn device_to_opendeck(hw_code: u8) -> Option<u8> {
    match hw_code {
        0x05 => Some(0),
        0x03 => Some(1),
        0x01 => Some(2),
        0x06 => Some(3),
        0x04 => Some(4),
        0x02 => Some(5),
        _ => None,
    }
}
