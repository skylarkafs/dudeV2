use nextdude_common::proto::v1;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn from_hex(hex: &str, fallback: Self) -> Self {
        let s = hex.trim().trim_start_matches('#');
        if s.len() < 6 {
            return fallback;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok();
        let g = u8::from_str_radix(&s[2..4], 16).ok();
        let b = u8::from_str_radix(&s[4..6], 16).ok();
        match (r, g, b) {
            (Some(r), Some(g), Some(b)) => Self { r, g, b },
            _ => fallback,
        }
    }
}

pub fn state_color(mode: ThemeMode, state: v1::ProbeState) -> RgbColor {
    match mode {
        ThemeMode::Dark => match state {
            v1::ProbeState::Up => RgbColor::new(48, 209, 88),
            v1::ProbeState::Down => RgbColor::new(255, 69, 58),
            v1::ProbeState::Unreachable => RgbColor::new(255, 159, 10),
            v1::ProbeState::Partial => RgbColor::new(255, 214, 10),
            _ => RgbColor::new(152, 152, 157),
        },
        ThemeMode::Light => match state {
            v1::ProbeState::Up => RgbColor::new(36, 138, 61),
            v1::ProbeState::Down => RgbColor::new(215, 0, 21),
            v1::ProbeState::Unreachable => RgbColor::new(201, 52, 0),
            v1::ProbeState::Partial => RgbColor::new(178, 80, 0),
            _ => RgbColor::new(110, 110, 115),
        },
    }
}

pub fn state_badge_glyph(state: v1::ProbeState) -> &'static str {
    match state {
        v1::ProbeState::Up => "OK",
        v1::ProbeState::Down => "DN",
        v1::ProbeState::Unreachable => "UR",
        v1::ProbeState::Partial => "PT",
        _ => "??",
    }
}

pub fn utilization_color(mode: ThemeMode, util_pct: f64) -> RgbColor {
    let pct = util_pct.clamp(0.0, 100.0);
    let low = match mode {
        ThemeMode::Dark => RgbColor::new(48, 209, 88),
        ThemeMode::Light => RgbColor::new(36, 138, 61),
    };
    let mid = match mode {
        ThemeMode::Dark => RgbColor::new(255, 159, 10),
        ThemeMode::Light => RgbColor::new(201, 52, 0),
    };
    let high = match mode {
        ThemeMode::Dark => RgbColor::new(255, 69, 58),
        ThemeMode::Light => RgbColor::new(215, 0, 21),
    };
    if pct < 50.0 {
        lerp_rgb(low, mid, pct / 50.0)
    } else {
        lerp_rgb(mid, high, (pct - 50.0) / 50.0)
    }
}

fn lerp_rgb(a: RgbColor, b: RgbColor, t: f64) -> RgbColor {
    let t = t.clamp(0.0, 1.0);
    RgbColor {
        r: (f64::from(a.r) + (f64::from(b.r) - f64::from(a.r)) * t) as u8,
        g: (f64::from(a.g) + (f64::from(b.g) - f64::from(a.g)) * t) as u8,
        b: (f64::from(a.b) + (f64::from(b.b) - f64::from(a.b)) * t) as u8,
    }
}
