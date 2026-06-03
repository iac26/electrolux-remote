#![no_std]
//! Electrolux portable-AC IR protocol: frame builder + raw mark/space pulse encoder.
//! Pure no_std logic (no hardware). Mirrors the verified ESPHome electrolux_ac timings.

/// Carrier on/off segment with a duration in microseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pulse { pub carrier_on: bool, pub micros: u16 }

// Verified Electrolux timings (microseconds) and carrier.
pub const HEADER_MARK: u16 = 8950;
pub const HEADER_SPACE: u16 = 4530;
pub const BIT_MARK: u16 = 563;
pub const ONE_SPACE: u16 = 1690;
pub const ZERO_SPACE: u16 = 538;
pub const FOOTER_MARK: u16 = 563;
pub const FOOTER_GAP: u16 = 2000;
pub const CARRIER_HZ: u32 = 38_000;

/// Frame is 13 bytes; encoded pulses = header(2) + 13*8*2 + footer(2) = 212.
pub const FRAME_LEN: usize = 13;
pub const PULSE_LEN: usize = 2 + FRAME_LEN * 8 * 2 + 2;

/// Reverse the bit order of a byte (LSB<->MSB). Used by the checksum + temp field.
pub const fn reverse_bits(mut b: u8) -> u8 {
    let mut r: u8 = 0;
    let mut i = 0;
    while i < 8 {
        r = (r << 1) | (b & 1);
        b >>= 1;
        i += 1;
    }
    r
}

/// Build the 13-byte OFF frame (power_on=false, fan=low, swing=off, temp=24).
pub fn build_off_frame() -> [u8; FRAME_LEN] {
    let mut arr = [0u8; FRAME_LEN];
    arr[0] = 0b1100_0011; // 0xC3 constant header
    arr[1] = 0b1110_0000; // swing off
    let t: u8 = 24 - 8; // temp field = 16
    arr[1] |= reverse_bits(t) >> 3;
    arr[4] = 0b0000_0110; // fan = low
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < 12 {
        sum += reverse_bits(arr[i]) as u32;
        i += 1;
    }
    arr[12] = reverse_bits((sum & 0xFF) as u8);
    arr
}

/// Encode a 13-byte frame into the raw mark/space pulse list (MSB-first per byte).
pub fn encode_frame(frame: &[u8; FRAME_LEN]) -> heapless::Vec<Pulse, PULSE_LEN> {
    let mut v: heapless::Vec<Pulse, PULSE_LEN> = heapless::Vec::new();
    let _ = v.push(Pulse { carrier_on: true, micros: HEADER_MARK });
    let _ = v.push(Pulse { carrier_on: false, micros: HEADER_SPACE });
    for &byte in frame.iter() {
        let mut bit: i8 = 7;
        while bit >= 0 {
            let one = (byte >> bit) & 1 == 1;
            let _ = v.push(Pulse { carrier_on: true, micros: BIT_MARK });
            let _ = v.push(Pulse { carrier_on: false, micros: if one { ONE_SPACE } else { ZERO_SPACE } });
            bit -= 1;
        }
    }
    let _ = v.push(Pulse { carrier_on: true, micros: FOOTER_MARK });
    let _ = v.push(Pulse { carrier_on: false, micros: FOOTER_GAP });
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_bits_matches_reference() {
        assert_eq!(reverse_bits(0b0001_0000), 0b0000_1000); // 16 -> 8
        assert_eq!(reverse_bits(0xE1), 0x87);
        assert_eq!(reverse_bits(0xC3), 0xC3); // palindrome
        assert_eq!(reverse_bits(0xAA), 0x55);
    }

    #[test]
    fn off_frame_is_byte_exact() {
        let f = build_off_frame();
        assert_eq!(
            f,
            [0xC3, 0xE1, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x55]
        );
    }

    #[test]
    fn checksum_is_last_byte() {
        let f = build_off_frame();
        let sum: u32 = f[..12].iter().map(|&b| reverse_bits(b) as u32).sum();
        assert_eq!(reverse_bits((sum & 0xFF) as u8), f[12]);
    }

    #[test]
    fn encode_pulse_counts_and_endpoints() {
        let f = build_off_frame();
        let pulses = encode_frame(&f);
        assert_eq!(pulses.len(), 212);
        assert_eq!(pulses[0], Pulse { carrier_on: true, micros: HEADER_MARK });
        assert_eq!(pulses[1], Pulse { carrier_on: false, micros: HEADER_SPACE });
        assert_eq!(pulses[211], Pulse { carrier_on: false, micros: FOOTER_GAP });
        assert_eq!(pulses[210], Pulse { carrier_on: true, micros: FOOTER_MARK });
    }

    #[test]
    fn first_data_bit_is_msb_first() {
        let f = build_off_frame();
        let pulses = encode_frame(&f);
        assert_eq!(pulses[2], Pulse { carrier_on: true, micros: BIT_MARK });
        assert_eq!(pulses[3], Pulse { carrier_on: false, micros: ONE_SPACE });
        assert_eq!(pulses[5], Pulse { carrier_on: false, micros: ONE_SPACE });
        assert_eq!(pulses[7], Pulse { carrier_on: false, micros: ZERO_SPACE });
    }
}
