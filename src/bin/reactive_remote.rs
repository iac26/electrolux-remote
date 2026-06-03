#![no_std]
#![no_main]

//! Reactive-style M5StickC Plus2 IR remote (PaperUI Layer #1 proof). Same hardware as
//! `main.rs`, but the UI is built declaratively with the reactive engine: a column of a title
//! + an "AC OFF" button. The button handler sets a `fire` signal; the EventSource (which owns
//! the IR peripherals) performs the blocking send when fire is set — the Layer-#1 way to
//! bridge reactive UI to device IO without async, and without capturing the non-'static, large
//! IR machinery in a 'static handler.

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

use paperui::reactive::{button, col, layout, run, text_static, EventSource, Scope, Signal, UiEvent};
use paperui::{Canvas, Color, EgCanvas, Rect};
use paperui_tft::TftTheme;

use electrolux_ir::{build_off_frame, encode_frame, Pulse, PULSE_LEN};

#[path = "../ir.rs"]
mod ir;
use ir::IrTx;

// ESP-IDF app descriptor — the bootloader/espflash require it to recognize & boot the image.
esp_bootloader_esp_idf::esp_app_desc!();

/// The device IO layer. As an `EventSource` it (1) emits focus/activate from the two buttons
/// (active-low press-edge detection) and (2) performs the queued IR send when the UI's `fire`
/// signal is set. It owns the (non-'static, large) IR peripherals so the reactive handler only
/// has to flip a tiny `Signal`.
struct ButtonsIo<'d> {
    a: Input<'d>,
    b: Input<'d>,
    a_down: bool,
    b_down: bool,
    primed: bool,
    fire: Signal<bool>,
    ir: IrTx<'d>,
    pulses: heapless::Vec<Pulse, PULSE_LEN>,
    ir_delay: Delay,
}

impl EventSource for ButtonsIo<'_> {
    fn poll(&mut self, _now_ms: u32) -> Option<UiEvent> {
        // Auto-focus the single button on the very first poll so BtnA activates it directly.
        if !self.primed {
            self.primed = true;
            return Some(UiEvent::FocusNext);
        }
        // IO bridge: the UI requested a send by setting `fire`. Do the blocking transmit here
        // (poll runs between dispatches, with no runtime lock held), then clear the flag.
        if self.fire.get() {
            self.fire.set(false);
            esp_println::println!("AC OFF: sending");
            self.ir.send(&self.pulses, 2, &mut self.ir_delay, 40);
        }
        // Input: active-low press-edge detection, one event per physical press.
        let a = self.a.is_low();
        let b = self.b.is_low();
        let a_edge = a && !self.a_down;
        let b_edge = b && !self.b_down;
        self.a_down = a;
        self.b_down = b;
        if b_edge {
            return Some(UiEvent::FocusNext);
        }
        if a_edge {
            return Some(UiEvent::Activate);
        }
        None
    }
}

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());

    let _bl = Output::new(p.GPIO27, Level::High, OutputConfig::default());

    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(40))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(p.GPIO13)
    .with_mosi(p.GPIO15);

    let cs = Output::new(p.GPIO5, Level::High, OutputConfig::default());
    let dc = Output::new(p.GPIO14, Level::Low, OutputConfig::default());
    let rst = Output::new(p.GPIO12, Level::High, OutputConfig::default());

    let spi_dev = ExclusiveDevice::new(spi, cs, Delay::new()).unwrap();
    let mut buffer = [0u8; 512];
    let di = SpiInterface::new(spi_dev, dc, &mut buffer);
    let mut init_delay = Delay::new();
    let mut display = Builder::new(ST7789, di)
        .display_size(135, 240)
        .display_offset(52, 40)
        .reset_pin(rst)
        .init(&mut init_delay)
        .unwrap();

    // IR transmitter on GPIO19.
    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap();
    let ir = IrTx::new(rmt.channel0, p.GPIO19);
    let frame = build_off_frame();
    let pulses = encode_frame(&frame);

    // Buttons (active-low, internal pull-ups).
    let in_cfg = InputConfig::default().with_pull(Pull::Up);
    let a = Input::new(p.GPIO37, in_cfg);
    let b = Input::new(p.GPIO39, in_cfg);

    // Reactive UI: title + "AC OFF" button. The handler only flips `fire`; ButtonsIo::poll does
    // the actual send.
    let cx = Scope::root();
    let fire = cx.signal(false);
    let root = col(
        cx,
        (
            text_static(cx, "Electrolux AC"),
            button(cx, "AC OFF", move || fire.set(true)),
        ),
    );
    layout(root, Rect::new(0, 0, 135, 240));

    // Clear to the dark background once; the engine draws the tree over it.
    let mut canvas = EgCanvas::new(&mut display);
    canvas.fill_rect(Rect::new(0, 0, 135, 240), Color::rgb(0x10, 0x10, 0x10));

    let theme = TftTheme;
    let mut io = ButtonsIo {
        a,
        b,
        a_down: false,
        b_down: false,
        primed: false,
        fire,
        ir,
        pulses,
        ir_delay: Delay::new(),
    };

    run(root, &mut canvas, &theme, &mut io, 0)
}
