#![no_std]
#![no_main]

//! Reactive landscape Electrolux AC remote for the M5StickC Plus2.
//!
//! Open-loop stateful remote: one `AcState` signal is the single source of truth; a 6-control
//! carousel (3 visible, crude slide animation) adjusts it and a live status line reflects it.
//! Up/Down/Select map to BtnB(G39)/BtnC(G35)/BtnA(G37). On any change the IO layer transmits a
//! full IR snapshot. ALL hardware glue lives here (HARD RULE) — PaperUI stays pin-agnostic.

use core::fmt::Write;

use esp_backtrace as _;

use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::main;
use esp_hal::rmt::Rmt;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;

use embedded_hal_bus::spi::ExclusiveDevice;

use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{Orientation, Rotation};
use mipidsi::Builder;

use paperui::reactive::{
    button, carousel, carousel_select_first, col, layout, run, text, EventSource, Scope, Signal,
    UiEvent, TEXT_CAP,
};
use paperui::{Canvas, Color, EgCanvas, Rect};
use paperui_tft::TftTheme;

use electrolux_ir::{build_frame, encode_frame, next_fan, next_mode, AcState, Fan, Mode, Pulse, PULSE_LEN};

#[path = "../ir.rs"]
mod ir;
use ir::IrTx;

// ESP-IDF app descriptor — the bootloader/espflash require it to recognize & boot the image.
esp_bootloader_esp_idf::esp_app_desc!();

/// Status line is padded to this many chars so a shorter state (e.g. "OFF") fully overwrites a
/// longer previous one ("COOL 24C HIGH SW") — the text node's bounds stay constant.
const STATUS_WIDTH: usize = 16;
/// IR send: two frames with a ~40 ms inter-frame gap so the AC accepts the repeat.
const IR_REPEATS: u8 = 2;
const IR_GAP_MS: u32 = 40;

/// Format the AC state into a fixed-width status string so repaints fully clear.
fn fmt_status(s: AcState) -> heapless::String<TEXT_CAP> {
    let mut out: heapless::String<TEXT_CAP> = heapless::String::new();
    if !s.power {
        let _ = out.push_str("OFF");
    } else {
        let mode = match s.mode {
            Mode::Cool => "COOL", Mode::Heat => "HEAT", Mode::Dry => "DRY",
            Mode::Fan => "FAN", Mode::Auto => "AUTO",
        };
        let fan = match s.fan {
            Fan::Auto => "AUTO", Fan::Low => "LOW", Fan::Mid => "MID", Fan::High => "HIGH",
        };
        let _ = write!(out, "{} {}C {}", mode, s.temp, fan);
        if s.swing {
            let _ = out.push_str(" SW");
        }
    }
    while out.len() < STATUS_WIDTH {
        let _ = out.push(' ');
    }
    out
}

/// Hardware glue: reads the 3 buttons (active-low edges) into `UiEvent`s and performs the queued
/// IR send when the UI sets `fire`. Owns the (non-'static) IR peripherals.
struct ButtonsIo<'d> {
    up: Input<'d>,     // BtnB / G39
    down: Input<'d>,   // BtnC / G35
    select: Input<'d>, // BtnA / G37
    up_held: bool,
    down_held: bool,
    sel_held: bool,
    fire: Signal<bool>,
    state: Signal<AcState>,
    ir: IrTx<'d>,
    pulses: heapless::Vec<Pulse, PULSE_LEN>,
    ir_delay: Delay,
}

impl EventSource for ButtonsIo<'_> {
    fn poll(&mut self, _now_ms: u32) -> Option<UiEvent> {
        // IO bridge: a control set `fire` — transmit the current full snapshot.
        if self.fire.get() {
            self.fire.set(false);
            let frame = build_frame(self.state.get());
            self.pulses = encode_frame(&frame);
            esp_println::println!("IR: sending snapshot");
            // Two frames with a ~40 ms inter-frame gap so the AC accepts the repeat.
            self.ir.send(&self.pulses, IR_REPEATS, &mut self.ir_delay, IR_GAP_MS);
        }
        let u = self.up.is_low();
        let d = self.down.is_low();
        let s = self.select.is_low();
        let u_edge = u && !self.up_held;
        let d_edge = d && !self.down_held;
        let s_edge = s && !self.sel_held;
        self.up_held = u;
        self.down_held = d;
        self.sel_held = s;
        if u_edge {
            return Some(UiEvent::FocusPrev);
        }
        if d_edge {
            return Some(UiEvent::FocusNext);
        }
        if s_edge {
            return Some(UiEvent::Activate);
        }
        None
    }
}

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());

    // Latch power so the board stays on when USB is unplugged (Plus2 holds power via GPIO4).
    let _hold = Output::new(p.GPIO4, Level::High, OutputConfig::default());

    let _bl = Output::new(p.GPIO27, Level::High, OutputConfig::default());

    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(40))
            .with_mode(SpiMode::_0),
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
    // Landscape via Deg90. mipidsi takes the NATIVE panel placement (135x240 @ 52,40 — same as
    // the portrait `main.rs`) and derives the rotated address-window offset itself; the logical
    // drawable area becomes 240x135. (Passing pre-rotated 240x135 @ 40,52 trips mipidsi's
    // `width + offset_x <= framebuffer_width` assertion, since the framebuffer stays 240x320.)
    let mut display = Builder::new(ST7789, di)
        .display_size(135, 240)
        .display_offset(52, 40)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .reset_pin(rst)
        .init(&mut init_delay)
        .unwrap();

    // IR transmitter on GPIO19.
    let rmt = Rmt::new(p.RMT, Rate::from_mhz(80)).unwrap();
    let ir = IrTx::new(rmt.channel0, p.GPIO19);

    // Buttons (active-low; G35/G37/G39 are input-only — rely on board external pull-ups).
    let in_cfg = InputConfig::default().with_pull(Pull::Up);
    let up = Input::new(p.GPIO39, in_cfg);
    let down = Input::new(p.GPIO35, in_cfg);
    let select = Input::new(p.GPIO37, in_cfg);

    // Reactive UI.
    let cx = Scope::root();
    let state = cx.signal(AcState { power: false, mode: Mode::Cool, temp: 24, fan: Fan::Low, swing: false });
    let fire = cx.signal(false);

    let status = text(cx, move || fmt_status(state.get()));
    let controls = [
        button(cx, "Power", move || { state.update(|s| s.power = !s.power); fire.set(true); }),
        button(cx, "Mode", move || { state.update(|s| s.mode = next_mode(s.mode)); fire.set(true); }),
        button(cx, "Temp +", move || { state.update(|s| s.temp = (s.temp + 1).min(32)); fire.set(true); }),
        button(cx, "Temp -", move || { state.update(|s| s.temp = (s.temp - 1).max(16)); fire.set(true); }),
        button(cx, "Fan", move || { state.update(|s| s.fan = next_fan(s.fan)); fire.set(true); }),
        button(cx, "Swing", move || { state.update(|s| s.swing = !s.swing); fire.set(true); }),
    ];
    let car = carousel(cx, &controls);
    let root = col(cx, (status, car));
    layout(root, Rect::new(0, 0, 240, 135));
    carousel_select_first(car);

    // Black wallpaper.
    let mut canvas = EgCanvas::new(&mut display);
    canvas.fill_rect(Rect::new(0, 0, 240, 135), Color::BLACK);

    let theme = TftTheme;
    let mut io = ButtonsIo {
        up,
        down,
        select,
        up_held: false,
        down_held: false,
        sel_held: false,
        fire,
        state,
        ir,
        pulses: heapless::Vec::new(),
        ir_delay: Delay::new(),
    };

    run(root, &mut canvas, &theme, &mut io, 0)
}
