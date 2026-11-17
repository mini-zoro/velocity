//! CSS colors and the documented color-mod adjusters.
use crate::Error;
use std::collections::BTreeMap;

/// Straight-alpha, sRGB color with eight bits per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Opacity (255 is opaque).
    pub a: u8,
}
impl Color {
    /// An opaque sRGB color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    /// Parses CSS, recursive `var()`, and Sublime's `color()` expressions.
    pub fn parse(text: &str, variables: &BTreeMap<String, String>) -> Result<Self, Error> {
        parse(text.trim(), variables, 0)
    }
    /// Four channels in RGBA order.
    pub const fn rgba8(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
    /// WCAG relative luminance, after compositing onto an opaque background.
    pub fn luminance(self) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }
    /// WCAG contrast ratio. Alpha is composited onto `background` first.
    pub fn contrast(self, background: Self) -> f64 {
        let a = self.over(background).luminance();
        let b = background.luminance();
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    /// Composites this foreground over a background in sRGB space.
    pub fn over(self, bg: Self) -> Self {
        let a = f64::from(self.a) / 255.;
        let b = f64::from(bg.a) / 255.;
        let out = a + b * (1. - a);
        if out == 0. {
            return Self {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            };
        }
        let mix =
            |f: u8, g: u8| byte((f64::from(f) * a + f64::from(g) * b * (1. - a)) / out / 255.);
        Self {
            r: mix(self.r, bg.r),
            g: mix(self.g, bg.g),
            b: mix(self.b, bg.b),
            a: byte(out),
        }
    }
    pub(crate) fn hsl(self) -> [f64; 3] {
        let r = f64::from(self.r) / 255.;
        let g = f64::from(self.g) / 255.;
        let b = f64::from(self.b) / 255.;
        let hi = r.max(g).max(b);
        let lo = r.min(g).min(b);
        let d = hi - lo;
        let l = (hi + lo) / 2.;
        if d == 0. {
            return [0., 0., l];
        }
        let h = if hi == r {
            ((g - b) / d).rem_euclid(6.)
        } else if hi == g {
            (b - r) / d + 2.
        } else {
            (r - g) / d + 4.
        };
        [h * 60., d / (1. - (2. * l - 1.).abs()), l]
    }
    pub(crate) fn from_hsl([h, s, l]: [f64; 3], a: u8) -> Self {
        let c = (1. - (2. * l - 1.).abs()) * s;
        let x = c * (1. - ((h / 60.).rem_euclid(2.) - 1.).abs());
        let m = l - c / 2.;
        let (r, g, b) = match (h.rem_euclid(360.) / 60.) as u32 {
            0 => (c, x, 0.),
            1 => (x, c, 0.),
            2 => (0., c, x),
            3 => (0., x, c),
            4 => (x, 0., c),
            _ => (c, 0., x),
        };
        Self {
            r: byte(r + m),
            g: byte(g + m),
            b: byte(b + m),
            a,
        }
    }
    pub(crate) fn lerp(self, other: Self, amount: f64, hsl: bool, alpha: bool) -> Self {
        let mix = |a: f64, b: f64| a + (b - a) * amount;
        let a = if alpha {
            byte(mix(f64::from(self.a) / 255., f64::from(other.a) / 255.))
        } else {
            self.a
        };
        if hsl {
            let x = self.hsl();
            let y = other.hsl();
            Self::from_hsl([mix(x[0], y[0]), mix(x[1], y[1]), mix(x[2], y[2])], a)
        } else {
            Self {
                r: byte(mix(f64::from(self.r), f64::from(other.r)) / 255.),
                g: byte(mix(f64::from(self.g), f64::from(other.g)) / 255.),
                b: byte(mix(f64::from(self.b), f64::from(other.b)) / 255.),
                a,
            }
        }
    }
}
fn byte(v: f64) -> u8 {
    (v.clamp(0., 1.) * 255.).round() as u8
}
/// Splits only at top-level delimiters, leaving nested functions intact.
pub(crate) fn parts(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = None;
    for (i, c) in text.char_indices() {
        if depth == 0 && (c.is_whitespace() || c == ',') {
            if let Some(a) = start.take() {
                out.push(&text[a..i]);
            }
            continue;
        }
        start.get_or_insert(i);
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
        }
    }
    if let Some(a) = start {
        out.push(&text[a..]);
    }
    out
}
fn function(text: &str) -> Option<(&str, &str)> {
    let (name, tail) = text.split_once('(')?;
    Some((name, tail.strip_suffix(')')?))
}
fn parse(text: &str, vars: &BTreeMap<String, String>, depth: usize) -> Result<Color, Error> {
    if depth > 64 {
        return Err(Error::Color("cyclic/deep color expression".into()));
    }
    if let Some((name, body)) = function(text) {
        if name == "var" {
            let (key, fallback) = body
                .split_once(',')
                .map_or((body, None), |(k, v)| (k, Some(v)));
            let value = vars
                .get(key.trim())
                .map(String::as_str)
                .or(fallback)
                .ok_or_else(|| Error::Color(format!("unknown variable {key}")))?;
            return parse(value.trim(), vars, depth + 1);
        }
        if name == "color" {
            let p = parts(body);
            let first = p.first().ok_or_else(|| Error::Color(text.into()))?;
            let mut color = parse(first, vars, depth + 1)?;
            for item in &p[1..] {
                color = adjust(color, item, vars, depth + 1)?;
            }
            return Ok(color);
        }
    }
    let c: csscolorparser::Color = text.parse().map_err(|_| Error::Color(text.into()))?;
    let [r, g, b, a] = c.to_rgba8();
    Ok(Color { r, g, b, a })
}
fn number(text: &str) -> Result<f64, Error> {
    let v = text.parse::<f64>().map_err(|_| Error::Color(text.into()))?;
    if !v.is_finite() {
        return Err(Error::Color(text.into()));
    }
    Ok(v)
}
fn scalar(text: &str) -> Result<f64, Error> {
    if let Some(p) = text.strip_suffix('%') {
        Ok(number(p)? / 100.)
    } else {
        number(text)
    }
}
fn adjusted(old: f64, body: &str) -> Result<f64, Error> {
    let compact = body.split_whitespace().collect::<String>();
    let (op, value) = if compact.starts_with(['+', '-', '*']) {
        (&compact[..1], &compact[1..])
    } else {
        ("", compact.as_str())
    };
    let value = scalar(value)?;
    Ok(match op {
        "+" => old + value,
        "-" => old - value,
        "*" => old * value,
        _ => value,
    }
    .clamp(0., 1.))
}
pub(crate) fn adjust(
    mut c: Color,
    text: &str,
    vars: &BTreeMap<String, String>,
    depth: usize,
) -> Result<Color, Error> {
    let (name, body) = function(text).ok_or_else(|| Error::Color(text.into()))?;
    match name {
        "alpha" | "a" => c.a = byte(adjusted(f64::from(c.a) / 255., body)?),
        "saturation" | "s" | "lightness" | "l" => {
            let mut hsl = c.hsl();
            let i = if name == "saturation" || name == "s" {
                1
            } else {
                2
            };
            hsl[i] = adjusted(hsl[i], body)?;
            c = Color::from_hsl(hsl, c.a);
        }
        "blend" | "blenda" => {
            let p = parts(body);
            if !(2..=3).contains(&p.len()) {
                return Err(Error::Color(text.into()));
            }
            if p.get(2).is_some_and(|s| !["rgb", "hsl"].contains(s)) {
                return Err(Error::Color(text.into()));
            }
            c = c.lerp(
                parse(p[0], vars, depth + 1)?,
                // minihtml follows the July 2016 CSS draft: the percentage weights
                // the base color, with 0% producing the specified color.
                // https://www.w3.org/TR/2016/WD-css-color-4-20160705/#blending
                1. - scalar(p[1])?.clamp(0., 1.),
                p.get(2) == Some(&"hsl"),
                name == "blenda",
            );
        }
        "min-contrast" => {
            let p = parts(body);
            if p.len() != 2 {
                return Err(Error::Color(text.into()));
            }
            let bg = parse(p[0], vars, depth + 1)?;
            let ratio = number(p[1])?;
            if c.contrast(bg) < ratio {
                let black = Color::rgb(0, 0, 0);
                let white = Color::rgb(255, 255, 255);
                let target = if black.contrast(bg) > white.contrast(bg) {
                    black
                } else {
                    white
                };
                let original = c;
                let mut lo = 0.;
                let mut hi = 1.;
                for _ in 0..24 {
                    let mid = (lo + hi) / 2.;
                    if original.lerp(target, mid, false, true).contrast(bg) >= ratio {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                c = original.lerp(target, hi, false, true);
            }
        }
        _ => return Err(Error::Color(format!("unsupported adjuster {name}"))),
    }
    Ok(c)
}
