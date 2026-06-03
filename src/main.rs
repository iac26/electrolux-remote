#![no_std]
#![no_main]

//! PaperUI firmware for the M5StickC Plus2 — a one-button IR remote that sends
//! the Electrolux "AC OFF" frame on BtnA.
//!
//! ## M5StickC Plus2 pin map
//! Display (1.14" ST7789, 135x240, portrait):
//!   * SCLK = GPIO13
//!   * MOSI = GPIO15
//!   * CS   = GPIO5
//!   * DC   = GPIO14
//!   * RST  = GPIO12
//!   * BL   = GPIO27 (backlight, driven high)
//! Buttons (active-low):
//!   * BtnA = GPIO37
//!   * BtnB = GPIO39
//! IR:
//!   * IR LED = GPIO19 (driven via the RMT peripheral)
//!
//! The ST7789 panel sits at an offset inside its 240x320 framebuffer; for the
//! StickC Plus2 portrait orientation that offset is (52, 40).

use esp_backtrace as _;

use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::main;
use esp_hal::rmt::Rmt;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;

use embedded_hal_bus::spi::ExclusiveDevice;

use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::Builder;

use paperui::{
    Button, ButtonEvent, ButtonId, Canvas, Color, DefaultTheme, DrawCtx, EgCanvas, Rect,
    UpdateHint, Widget,
};

use paperui_tft::ButtonReader;

use electrolux_ir::{build_off_frame, encode_frame};

mod ir;
use ir::IrTx;

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());

    // Backlight on.
    let _bl = Output::new(p.GPIO27, Level::High, OutputConfig::default());

    // SPI2 bus for the display: 40 MHz, SPI mode 0, SCLK = GPIO13, MOSI = GPIO15.
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(40))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(p.GPIO13)
    .with_mosi(p.GPIO15);

    // Control pins.
    let cs = Output::new(p.GPIO5, Level::High, OutputConfig::default());
    let dc = Output::new(p.GPIO14, Level::Low, OutputConfig::default());
    let rst = Output::new(p.GPIO12, Level::High, OutputConfig::default());

    // Wrap the bus + CS into an embedded-hal SpiDevice. The device's internal
    // delay (used for CS settling) gets its own Delay instance.
    let spi_dev = ExclusiveDevice::new(spi, cs, Delay::new()).unwrap();

    // mipidsi SPI display interface needs a scratch buffer for pixel batches.
    let mut buffer = [0u8; 512];
    let di = SpiInterface::new(spi_dev, dc, &mut buffer);

    // Initialize the ST7789. `init` needs its own &mut DelayNs.
    let mut init_delay = Delay::new();
    let mut display = Builder::new(ST7789, di)
        .display_size(135, 240)
        .display_offset(52, 40)
        .reset_pin(rst)
        .init(&mut init_delay)
        .unwrap();

    // Draw the one-button UI through the PaperUI engine/theme/canvas stack.
    let theme = DefaultTheme;
    let btn = Button::new("AC OFF");
    {
        let mut canvas = EgCanvas::new(&mut display);
        canvas.fill_rect(Rect::new(0, 0, 135, 240), Color::rgb(0x10, 0x10, 0x10));
        let mut hint = UpdateHint::None;
        let bounds = Rect::new(18, 100, 100, 40);
        let mut ctx = DrawCtx::new(&mut canvas, bounds, true, &mut hint);
        Widget::<EgCanvas<_>, DefaultTheme>::draw(&btn, &mut ctx, &theme);
    }

    // IR transmitter on GPIO19 via the RMT peripheral (80 MHz source on esp32).
    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap();
    let mut ir = IrTx::new(rmt.channel0, p.GPIO19);
    let frame = build_off_frame();
    let pulses = encode_frame(&frame);
    let mut ir_delay = Delay::new();

    // Buttons: active-low, internal pull-ups.
    let in_cfg = InputConfig::default().with_pull(Pull::Up);
    let mut buttons = ButtonReader::new(
        Input::new(p.GPIO37, in_cfg),
        Input::new(p.GPIO39, in_cfg),
    );

    let mut now_ms: u32 = 0;
    let loop_delay = Delay::new();
    loop {
        if let Some((ButtonId::A, ButtonEvent::Click)) = buttons.poll(now_ms) {
            esp_println::println!("BtnA: sending AC OFF");
            // Two frames with a ~40 ms inter-frame gap so the AC accepts the repeat.
            ir.send(&pulses, 2, &mut ir_delay, 40);
        }
        loop_delay.delay_millis(5);
        now_ms = now_ms.wrapping_add(5);
    }
}
