//! 38 kHz IR transmit over the ESP32 RMT peripheral, consuming electrolux-ir pulses.
use electrolux_ir::{Pulse, CARRIER_HZ, PULSE_LEN};
use embedded_hal::delay::DelayNs;
use esp_hal::gpio::{Level, OutputPin};
use esp_hal::rmt::{Channel, PulseCode, TxChannelConfig, TxChannelCreator};
use esp_hal::Blocking;
use heapless::Vec;

/// RMT source clock divider: the esp32 RMT source is the 80 MHz APB clock, so a
/// divider of 80 yields a 1 MHz channel clock, i.e. 1 RMT tick == 1 µs.
const RMT_CLK_DIVIDER: u8 = 80;

/// Half-period of the 38 kHz carrier, expressed in RMT source-clock ticks
/// (1e6 / 38000 / 2 ≈ 13). The carrier duty stays ~50%.
const CARRIER_HALF_TICKS: u16 = (1_000_000 / (CARRIER_HZ * 2)) as u16;

/// Number of RMT symbols: each symbol packs two pulses, plus one end marker.
const SYMBOL_LEN: usize = PULSE_LEN / 2 + 1;

/// Map a "carrier on/off" flag to the RMT output [`Level`] for that segment.
///
/// With carrier modulation enabled and `carrier_level = High`, the 38 kHz
/// carrier is gated onto segments driven `High` (the IR "marks"); `Low`
/// segments are silent "spaces".
#[inline]
fn level(carrier_on: bool) -> Level {
    if carrier_on {
        Level::High
    } else {
        Level::Low
    }
}

/// Owns a configured RMT TX channel on the IR LED pin and transmits
/// electrolux-ir pulse lists as 38 kHz-modulated IR bursts.
pub struct IrTx<'d> {
    // Held in an Option so the blocking `transmit`/`wait` round-trip (which
    // moves the channel out and hands it back) can run repeatedly.
    channel: Option<Channel<'d, Blocking, Tx>>,
}

// Re-export the transmit-direction marker for the channel type alias above.
use esp_hal::rmt::Tx;

impl<'d> IrTx<'d> {
    /// Configure `creator`'s TX channel for a gated 38 kHz carrier and bind it
    /// to the IR LED `pin`.
    ///
    /// `creator` is one of the `Rmt` channel creators (e.g. `rmt.channel0`).
    pub fn new<C>(creator: C, pin: impl OutputPin + 'd) -> Self
    where
        C: TxChannelCreator<'d, Blocking>,
    {
        let config = TxChannelConfig::default()
            .with_clk_divider(RMT_CLK_DIVIDER)
            .with_idle_output(true)
            .with_idle_output_level(Level::Low)
            .with_carrier_modulation(true)
            .with_carrier_high(CARRIER_HALF_TICKS)
            .with_carrier_low(CARRIER_HALF_TICKS)
            .with_carrier_level(Level::High);

        // `configure_tx` validates the config against the hardware; for these
        // fixed, known-good values it cannot fail, so unwrap is acceptable here.
        let channel = creator
            .configure_tx(&config)
            .unwrap_or_else(|_| panic!("RMT TX configure failed"))
            .with_pin(pin);

        Self {
            channel: Some(channel),
        }
    }

    /// Transmit `pulses` (an even-length mark/space list from electrolux-ir)
    /// `repeats` times, waiting `gap_ms` between consecutive frames. The Electrolux
    /// receiver needs a real inter-frame gap (~40 ms) to accept the 2nd frame as a
    /// repeat rather than glitching the two together, so the gap is explicit.
    pub fn send(&mut self, pulses: &[Pulse], repeats: u8, delay: &mut impl DelayNs, gap_ms: u32) {
        // Pack consecutive pairs of pulses into RMT symbols
        // (level0, dur0, level1, dur1), then append a zero-length end marker.
        let mut data: Vec<PulseCode, SYMBOL_LEN> = Vec::new();
        let mut it = pulses.chunks_exact(2);
        for pair in &mut it {
            let code = PulseCode::new(
                level(pair[0].carrier_on),
                pair[0].micros,
                level(pair[1].carrier_on),
                pair[1].micros,
            );
            // Capacity is sized for the full PULSE_LEN; push cannot overflow.
            let _ = data.push(code);
        }
        // electrolux-ir pulse lists are even-length, but stay robust if a stray
        // trailing pulse appears: emit it with a zero-length second slot.
        if let [last] = it.remainder() {
            let code = PulseCode::new(level(last.carrier_on), last.micros, Level::Low, 0);
            let _ = data.push(code);
        }
        let _ = data.push(PulseCode::end_marker());

        for i in 0..repeats {
            if i > 0 {
                delay.delay_ms(gap_ms);
            }
            let channel = match self.channel.take() {
                Some(ch) => ch,
                None => return,
            };
            match channel.transmit(&data) {
                Ok(tx) => match tx.wait() {
                    Ok(ch) => self.channel = Some(ch),
                    Err((_e, ch)) => self.channel = Some(ch),
                },
                Err((_e, ch)) => {
                    self.channel = Some(ch);
                    return;
                }
            }
        }
    }
}
