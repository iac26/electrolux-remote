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

/// AC operating mode (byte 6 when powered on).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode { Auto, Cool, Heat, Dry, Fan }

/// Fan speed (byte 4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fan { Auto, Low, Mid, High }

/// The full desired AC state. `temp` is in °C and clamped to 16..=32 by `build_frame`.
/// Copy + PartialEq so it can be stored directly in a PaperUI `Signal`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AcState {
    pub power: bool,
    pub mode: Mode,
    pub temp: i8,
    pub fan: Fan,
    pub swing: bool,
}

/// Next mode in the UI cycle Cool→Heat→Dry→Fan→Auto→Cool.
pub fn next_mode(m: Mode) -> Mode {
    match m { Mode::Cool => Mode::Heat, Mode::Heat => Mode::Dry, Mode::Dry => Mode::Fan, Mode::Fan => Mode::Auto, Mode::Auto => Mode::Cool }
}
/// Next fan speed in the UI cycle Auto→Low→Mid→High→Auto.
pub fn next_fan(f: Fan) -> Fan {
    match f { Fan::Auto => Fan::Low, Fan::Low => Fan::Mid, Fan::Mid => Fan::High, Fan::High => Fan::Auto }
}

/// Build the 13-byte frame for a full AC state (mirrors the Python `build_packet`).
pub fn build_frame(s: AcState) -> [u8; FRAME_LEN] {
    let mut arr = [0u8; FRAME_LEN];
    arr[0] = 0b1100_0011; // constant header 0xC3
    arr[1] = if s.swing { 0x00 } else { 0xE0 }; // swing on=000 / off=111 in the top 3 bits
    let t = (s.temp.clamp(16, 32) - 8) as u8;
    arr[1] |= reverse_bits(t) >> 3;
    arr[4] = match s.fan {
        Fan::Auto => 0b0000_0101,
        Fan::Low => 0b0000_0110,
        Fan::Mid => 0b0000_0010,
        Fan::High => 0b0000_0100,
    };
    if s.power {
        arr[6] = match s.mode {
            Mode::Auto => 0b000,
            Mode::Cool => 0b100,
            Mode::Heat => 0b001,
            Mode::Fan => 0b011,
            Mode::Dry => 0b010,
        };
        arr[9] = 0b0000_0100; // on/off bit = ON
    }
    let mut sum: u32 = 0;
    let mut i = 0;
    while i < 12 {
        sum += reverse_bits(arr[i]) as u32;
        i += 1;
    }
    arr[12] = reverse_bits((sum & 0xFF) as u8);
    arr
}

/// The verified OFF frame (power off, mode auto, fan low, swing off, temp 24).
pub fn build_off_frame() -> [u8; FRAME_LEN] {
    build_frame(AcState { power: false, mode: Mode::Auto, temp: 24, fan: Fan::Low, swing: false })
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

    #[test]
    fn build_frame_off_matches_off_helper() {
        let s = AcState { power: false, mode: Mode::Auto, temp: 24, fan: Fan::Low, swing: false };
        assert_eq!(build_frame(s), build_off_frame());
        assert_eq!(
            build_frame(s),
            [0xC3, 0xE1, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x55]
        );
    }

    #[test]
    fn power_on_sets_mode_byte_and_onoff_byte() {
        for (mode, code) in [
            (Mode::Auto, 0b000u8), (Mode::Cool, 0b100), (Mode::Heat, 0b001),
            (Mode::Fan, 0b011), (Mode::Dry, 0b010),
        ] {
            let f = build_frame(AcState { power: true, mode, temp: 24, fan: Fan::Low, swing: false });
            assert_eq!(f[6], code, "mode byte for {:?}", mode);
            assert_eq!(f[9], 0b0000_0100, "on/off byte ON for {:?}", mode);
        }
    }

    #[test]
    fn power_off_zeroes_mode_and_onoff() {
        let f = build_frame(AcState { power: false, mode: Mode::Cool, temp: 24, fan: Fan::High, swing: false });
        assert_eq!(f[6], 0);
        assert_eq!(f[9], 0);
    }

    #[test]
    fn fan_field_maps_each_speed() {
        for (fan, code) in [
            (Fan::Auto, 0b0000_0101u8), (Fan::Low, 0b0000_0110),
            (Fan::Mid, 0b0000_0010), (Fan::High, 0b0000_0100),
        ] {
            let f = build_frame(AcState { power: true, mode: Mode::Cool, temp: 24, fan, swing: false });
            assert_eq!(f[4], code, "fan byte for {:?}", fan);
        }
    }

    #[test]
    fn temp_field_and_clamp() {
        let b = |t: i8| build_frame(AcState { power: true, mode: Mode::Cool, temp: t, fan: Fan::Low, swing: false })[1];
        assert_eq!(b(16), 0xE2);
        assert_eq!(b(24), 0xE1);
        assert_eq!(b(32), 0xE3);
        assert_eq!(b(40), 0xE3, "clamps high to 32");
        assert_eq!(b(10), 0xE2, "clamps low to 16");
    }

    #[test]
    fn swing_clears_high_three_bits_of_byte1() {
        let on = build_frame(AcState { power: true, mode: Mode::Cool, temp: 24, fan: Fan::Low, swing: true });
        assert_eq!(on[1] & 0b1110_0000, 0, "swing on => top 3 bits of arr[1] are 0");
        let off = build_frame(AcState { power: true, mode: Mode::Cool, temp: 24, fan: Fan::Low, swing: false });
        assert_eq!(off[1] & 0b1110_0000, 0b1110_0000, "swing off => top 3 bits set");
    }

    #[test]
    fn checksum_is_reverse_bits_sum() {
        let f = build_frame(AcState { power: true, mode: Mode::Heat, temp: 30, fan: Fan::High, swing: true });
        let sum: u32 = f[..12].iter().map(|&b| reverse_bits(b) as u32).sum();
        assert_eq!(reverse_bits((sum & 0xFF) as u8), f[12]);
    }

    #[test]
    fn mode_and_fan_cycle_order() {
        assert_eq!(next_mode(Mode::Cool), Mode::Heat);
        assert_eq!(next_mode(Mode::Heat), Mode::Dry);
        assert_eq!(next_mode(Mode::Dry), Mode::Fan);
        assert_eq!(next_mode(Mode::Fan), Mode::Auto);
        assert_eq!(next_mode(Mode::Auto), Mode::Cool);
        assert_eq!(next_fan(Fan::Auto), Fan::Low);
        assert_eq!(next_fan(Fan::Low), Fan::Mid);
        assert_eq!(next_fan(Fan::Mid), Fan::High);
        assert_eq!(next_fan(Fan::High), Fan::Auto);
    }
}
