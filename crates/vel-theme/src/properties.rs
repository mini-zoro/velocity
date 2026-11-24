//! Typed theme values. Unknown extensions are retained and reported by the compiler.
use crate::{Error, Palette};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use vel_color_scheme::Color;

/// Device-independent margins in theme order: left, top, right, bottom (not CSS order).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Insets {
    /// Left edge.
    pub left: f32,
    /// Top edge.
    pub top: f32,
    /// Right edge.
    pub right: f32,
    /// Bottom edge.
    pub bottom: f32,
}
impl Insets {
    /// Uniform margins.
    pub const fn all(value: f32) -> Self {
        Self {
            left: value,
            top: value,
            right: value,
            bottom: value,
        }
    }
    fn parse(v: &Value) -> Result<Self, Error> {
        if v.is_number() {
            return Ok(Self::all(number(v)?));
        }
        let a = v.as_array().ok_or_else(|| invalid("margin"))?;
        match a.as_slice() {
            [x, y] => Ok(Self {
                left: number(x)?,
                right: number(x)?,
                top: number(y)?,
                bottom: number(y)?,
            }),
            [l, t, r, b] => Ok(Self {
                left: number(l)?,
                top: number(t)?,
                right: number(r)?,
                bottom: number(b)?,
            }),
            _ => Err(invalid("margin must have 1, 2 or 4 values")),
        }
    }
}

/// Font sizes preserve their unit until a widget supplies the appropriate root size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontSize {
    /// Absolute device-independent pixels.
    Pixels(f32),
    /// Multiple of the application's (or completion view's) font size.
    Rem(f32),
}
impl FontSize {
    /// Evaluates a size without reparsing on the render path.
    pub fn pixels(self, root: f32) -> f32 {
        match self {
            Self::Pixels(n) => n,
            Self::Rem(n) => n * root,
        }
    }
}
/// Transition easing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    /// Constant rate.
    #[default]
    Linear,
    /// Smoothstep, with zero slope at both ends.
    Smoothstep,
}
/// Transition specification. The caller keeps the start value and elapsed monotonic time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Animation {
    /// Rate multiplier (at least one); duration is `1 / speed` seconds.
    pub speed: f32,
    /// Easing function.
    pub interpolation: Interpolation,
}
impl Animation {
    fn progress(self, elapsed: f32) -> f32 {
        let t = (elapsed.max(0.) * self.speed).clamp(0., 1.);
        match self.interpolation {
            Interpolation::Linear => t,
            Interpolation::Smoothstep => t * t * (3. - 2. * t),
        }
    }
}
/// An immediate value or an animated destination.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Animated<T> {
    /// Destination value.
    pub target: T,
    /// None applies the destination immediately.
    pub animation: Option<Animation>,
}
impl Animated<f32> {
    /// Samples a transition from the caller's previous value.
    pub fn sample(self, from: f32, elapsed: f32) -> f32 {
        self.animation.map_or(self.target, |a| {
            from + (self.target - from) * a.progress(elapsed)
        })
    }
}
impl Animated<Color> {
    /// Samples straight-alpha sRGB channels; no clock or allocations are needed.
    pub fn sample(self, from: Color, elapsed: f32) -> Color {
        let Some(a) = self.animation else {
            return self.target;
        };
        let t = a.progress(elapsed);
        let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
        Color {
            r: mix(from.r, self.target.r),
            g: mix(from.g, self.target.g),
            b: mix(from.b, self.target.b),
            a: mix(from.a, self.target.a),
        }
    }
}
/// Static PNG or a timed PNG sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    /// Resource names relative to Packages/.
    pub keyframes: Vec<Arc<str>>,
    /// Repeat after the last frame.
    pub looping: bool,
    /// Seconds per frame.
    pub frame_time: f32,
}
impl Texture {
    /// Selects a frame from elapsed monotonic time.
    pub fn frame(&self, elapsed: f32) -> &str {
        let frame = (elapsed.max(0.) / self.frame_time) as usize;
        &self.keyframes[if self.looping {
            frame % self.keyframes.len()
        } else {
            frame.min(self.keyframes.len() - 1)
        }]
    }
}
/// Enumeration-valued properties use validated keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    /// Follow the operating system.
    System,
    /// Dark title bar.
    Dark,
    /// Light title bar.
    Light,
    /// Uppercase heading.
    Upper,
    /// Lowercase heading.
    Lower,
    /// Title-case heading.
    Title,
    /// Left side.
    Left,
    /// Right side.
    Right,
}
/// Layer properties are separately typed; layers zero through three are supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LayerProperty {
    /// Fill or texture tint.
    Tint,
    /// Master alpha.
    Opacity,
    /// PNG or frame sequence.
    Texture,
    /// Nine-slice insets.
    InnerMargin,
    /// Draw the center patch.
    DrawCenter,
    /// Tile rather than stretch.
    Repeat,
    /// Optional per-layer tint blend, accepted for renderer extensions.
    TintModifier,
}
#[derive(Debug, Clone, Copy)]
enum Kind {
    Color,
    Number,
    Integer,
    Bool,
    Insets,
    Point,
    Text,
    FontSize,
    Modifier,
    TitleStyle,
    Case,
    Side,
}
macro_rules! properties {
    ($($id:ident => $name:literal : $kind:ident),* $(,)?) => {
        /// Property identifiers. `color` canonicalizes to `Fg`, so aliases obey rule order.
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Property {
            $(#[doc = $name] $id,)*
            /// One of the four raster/fill layers.
            Layer(u8, LayerProperty),
            /// Unrecognized property, retained losslessly for future renderers.
            Extension(Arc<str>),
        }
        /// Documented non-layer properties, plus compatible renderer extension properties.
        pub const PROPERTIES: &[&str] = &[$($name),*];
        impl Property {
            /// Parses a name, retaining unknown extensions.
            pub fn named(name: &str) -> Self {
                if name == "color" { return Self::Fg; }
                match name { $($name => return Self::$id,)* _ => {} }
                if let Some(rest) = name.strip_prefix("layer")
                    && let Some((index, key)) = rest.split_once('.')
                    && let Ok(index @ 0..=3) = index.parse::<u8>() {
                    let key = match key {
                        "tint" => LayerProperty::Tint, "opacity" => LayerProperty::Opacity,
                        "texture" => LayerProperty::Texture, "inner_margin" => LayerProperty::InnerMargin,
                        "draw_center" => LayerProperty::DrawCenter, "repeat" => LayerProperty::Repeat,
                        "tint_modifier" => LayerProperty::TintModifier,
                        _ => return Self::Extension(name.into()),
                    };
                    return Self::Layer(index,key);
                }
                Self::Extension(name.into())
            }
            fn kind(&self) -> Option<Kind> {
                Some(match self {
                    $(Self::$id => Kind::$kind,)*
                    Self::Layer(_, LayerProperty::Tint) => Kind::Color,
                    Self::Layer(_, LayerProperty::Opacity) => Kind::Number,
                    Self::Layer(_, LayerProperty::InnerMargin) => Kind::Insets,
                    Self::Layer(_, LayerProperty::DrawCenter | LayerProperty::Repeat) => Kind::Bool,
                    Self::Layer(_, LayerProperty::TintModifier) => Kind::Modifier,
                    _ => return None,
                })
            }
        }
    }
}
properties! {
    HitTestLevel => "hit_test_level": Number,
    ContentMargin => "content_margin": Insets,
    Fg => "fg": Color, Bg => "bg": Color,
    FontFace => "font.face": Text, FontSize => "font.size": FontSize,
    FontBold => "font.bold": Bool, FontItalic => "font.italic": Bool,
    ShadowColor => "shadow_color": Color, ShadowOffset => "shadow_offset": Point,
    Opacity => "opacity": Number, TintIndex => "tint_index": Integer,
    TintModifier => "tint_modifier": Modifier,
    MatchFg => "match_fg": Color, SelectedFg => "selected_fg": Color,
    SelectedMatchFg => "selected_match_fg": Color, SelectedBg => "selected_bg": Color,
    DarkContent => "dark_content": Bool, RowPadding => "row_padding": Insets,
    BorderColor => "border_color": Color, BackgroundColor => "background_color": Color,
    TitleStyle => "style": TitleStyle, Indent => "indent": Integer,
    IndentOffset => "indent_offset": Integer, IndentTopLevel => "indent_top_level": Bool,
    SpacerRows => "spacer_rows": Bool, Case => "case": Case, Spacing => "spacing": Integer,
    TabOverlap => "tab_overlap": Integer, TabWidth => "tab_width": Integer,
    TabMinWidth => "tab_min_width": Integer, TabHeight => "tab_height": Integer,
    MouseWheelSwitch => "mouse_wheel_switch": Bool, MaxMarginTrim => "max_margin_trim": Integer,
    AccentTintIndex => "accent_tint_index": Integer, AccentTintModifier => "accent_tint_modifier": Modifier,
    CloseButtonSide => "close_button_side": Side,
    LinkColor => "link_color": Color, MonospaceColor => "monospace_color": Color,
    MonospaceBackgroundColor => "monospace_background_color": Color,
    BackgroundModifier => "background_modifier": Text,
    BorderSize => "border_size": Number, ViewportColor => "viewport_color": Color,
    ViewportOpacity => "viewport_opacity": Number, FgBlend => "fg_blend": Bool,
    InsideSpacing => "inside_spacing": Integer, OutsideVspacing => "outside_vspacing": Integer,
    OutsideHspacing => "outside_hspacing": Integer, MinSize => "min_size": Point,
    Overlay => "overlay": Bool,
    LeftShadow => "left_shadow": Color, LeftShadowSize => "left_shadow_size": Integer,
    TopShadow => "top_shadow": Color, TopShadowSize => "top_shadow_size": Integer,
    RightShadow => "right_shadow": Color, RightShadowSize => "right_shadow_size": Integer,
    BottomShadow => "bottom_shadow": Color, BottomShadowSize => "bottom_shadow_size": Integer,
    ColorSchemeTint => "color_scheme_tint": Color, ColorSchemeTint2 => "color_scheme_tint_2": Color,
    IconColor => "icon_color": Color, IconSize => "icon_size": Point,
    IconMargin => "icon_margin": Insets, Radius => "radius": Number,
    // Velocity extensions. Sublime Text themes do not set them, so they render as before.
    SurfaceInset => "surface_inset": Insets, ShadowBlur => "shadow_blur": Number,
    ShadowSpread => "shadow_spread": Number, Pill => "pill": Bool,
    ViewportRadius => "viewport_radius": Number
}

/// Validated values used by the renderer without JSON interpretation.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    /// Explicit null removes a previously specified value, restoring the widget default.
    Reset,
    /// Color (optionally animated).
    Color(Animated<Color>),
    /// Float (optionally animated).
    Number(Animated<f32>),
    /// Integer DIP count or layer index.
    Integer(i32),
    /// Boolean switch.
    Bool(bool),
    /// Theme-order margins.
    Insets(Insets),
    /// Coordinates or minimum width/height.
    Point([f32; 2]),
    /// Font face, modifier expression, or resource-independent text.
    Text(Arc<str>),
    /// Root-relative or absolute size.
    FontSize(FontSize),
    /// Four byte tint blend.
    Modifier([u8; 4]),
    /// Validated keyword.
    Keyword(Keyword),
    /// PNG/frame sequence.
    Texture(Texture),
    /// Unrecognized renderer extension.
    Extension(Value),
}
/// Sparse resolved style: absence means the widget's documented default.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ResolvedStyle {
    pub(crate) values: BTreeMap<Property, PropertyValue>,
}
impl ResolvedStyle {
    /// Fetches a typed property without allocation (use an identifier, not `Property::named`).
    pub fn get(&self, property: &Property) -> Option<&PropertyValue> {
        self.values.get(property)
    }
    /// All explicit properties, in stable identifier order.
    pub fn iter(&self) -> impl Iterator<Item = (&Property, &PropertyValue)> {
        self.values.iter()
    }
    /// Color destination, ignoring transition metadata.
    pub fn color(&self, property: &Property) -> Option<Color> {
        match self.get(property) {
            Some(PropertyValue::Color(c)) => Some(c.target),
            _ => None,
        }
    }
    /// Float destination, or an integer converted to a float.
    pub fn number(&self, property: &Property) -> Option<f32> {
        match self.get(property) {
            Some(PropertyValue::Number(n)) => Some(n.target),
            Some(PropertyValue::Integer(n)) => Some(*n as f32),
            _ => None,
        }
    }
    /// Boolean value.
    pub fn boolean(&self, property: &Property) -> Option<bool> {
        match self.get(property) {
            Some(PropertyValue::Bool(b)) => Some(*b),
            _ => None,
        }
    }
    /// Margin value.
    pub fn insets(&self, property: &Property) -> Option<Insets> {
        match self.get(property) {
            Some(PropertyValue::Insets(i)) => Some(*i),
            _ => None,
        }
    }
}
fn invalid(s: &str) -> Error {
    Error::Format(s.into())
}
fn number(v: &Value) -> Result<f32, Error> {
    v.as_f64()
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .ok_or_else(|| invalid("expected finite number"))
}
fn text(v: &Value) -> Result<&str, Error> {
    v.as_str().ok_or_else(|| invalid("expected string"))
}
fn animation(v: &Value) -> Result<(Value, Option<Animation>), Error> {
    if let Some(target) = v.get("target") {
        let speed = v.get("speed").map(number).transpose()?.unwrap_or(1.);
        if speed < 1. {
            return Err(invalid("animation speed must be >= 1"));
        }
        let interpolation = match v
            .get("interpolation")
            .map(text)
            .transpose()?
            .unwrap_or("linear")
        {
            "linear" => Interpolation::Linear,
            "smoothstep" => Interpolation::Smoothstep,
            _ => return Err(invalid("unknown interpolation")),
        };
        Ok((
            target.clone(),
            Some(Animation {
                speed,
                interpolation,
            }),
        ))
    } else {
        Ok((v.clone(), None))
    }
}
pub(crate) fn resolve_variable(
    v: &Value,
    vars: &BTreeMap<String, Value>,
    depth: usize,
) -> Result<Value, Error> {
    if depth > 64 {
        return Err(invalid("cyclic/deep theme variable"));
    }
    if let Some(s) = v
        .as_str()
        .and_then(|s| s.strip_prefix("var("))
        .and_then(|s| s.strip_suffix(')'))
    {
        if let Some(found) = vars.get(s.trim()) {
            return resolve_variable(found, vars, depth + 1);
        }
        if !s.trim().starts_with("--") {
            return Err(invalid(&format!("unknown theme variable {s}")));
        }
    }
    Ok(v.clone())
}
/// What a theme's values resolve against: its variables, and the table that `var()` in string
/// colors uses (the palette's variables, then the theme's). The table is the same for every
/// value, so it is built once per compile rather than once per color, which made compiling
/// grow with colors times variables.
pub(crate) struct Scope<'a> {
    vars: &'a BTreeMap<String, Value>,
    palette: &'a Palette,
    colors: BTreeMap<String, String>,
}
impl<'a> Scope<'a> {
    pub(crate) fn new(
        vars: &'a BTreeMap<String, Value>,
        palette: &'a Palette,
    ) -> Result<Self, Error> {
        let mut colors = palette.variables.clone();
        for (k, v) in vars {
            let v = resolve_variable(v, vars, 0)?;
            if let Some(s) = v.as_str() {
                colors.insert(k.clone(), s.into());
            } else if let Some(a) = v.as_array()
                && let Ok(color) = array_color(a, palette)
            {
                colors.insert(k.clone(), crate::palette::css(color));
            }
        }
        Ok(Self {
            vars,
            palette,
            colors,
        })
    }
}
pub(crate) fn parse_value(
    p: &Property,
    value: &Value,
    scope: &Scope<'_>,
) -> Result<PropertyValue, Error> {
    let v = resolve_variable(value, scope.vars, 0)?;
    if v.is_null() {
        return Ok(PropertyValue::Reset);
    }
    if matches!(p, Property::Layer(_, LayerProperty::Texture)) {
        let (frames, looping, frame_time) = if let Some(s) = v.as_str() {
            (vec![Arc::from(s)], false, 1.)
        } else {
            let frames = v
                .get("keyframes")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("texture keyframes"))?
                .iter()
                .map(|v| Ok(Arc::from(text(v)?)))
                .collect::<Result<Vec<_>, Error>>()?;
            let looping = v
                .get("loop")
                .map(|v| v.as_bool().ok_or_else(|| invalid("texture loop")))
                .transpose()?
                .unwrap_or(false);
            let time = v
                .get("frame_time")
                .map(number)
                .transpose()?
                .unwrap_or(0.0333);
            (frames, looping, time)
        };
        if frames.is_empty() || frame_time <= 0. {
            return Err(invalid("texture needs frames and positive frame_time"));
        }
        return Ok(PropertyValue::Texture(Texture {
            keyframes: frames,
            looping,
            frame_time,
        }));
    }
    let Some(kind) = p.kind() else {
        return Ok(PropertyValue::Extension(v));
    };
    Ok(match kind {
        Kind::Color => {
            let (target, animation) = animation(&v)?;
            PropertyValue::Color(Animated {
                target: parse_color(&target, scope)?,
                animation,
            })
        }
        Kind::Number => {
            let (target, animation) = animation(&v)?;
            let n = number(&target)?;
            if matches!(
                p,
                Property::Opacity
                    | Property::ViewportOpacity
                    | Property::HitTestLevel
                    | Property::Layer(_, LayerProperty::Opacity)
            ) && !(0. ..=1.).contains(&n)
            {
                return Err(invalid("opacity/hit threshold must be in 0..1"));
            }
            PropertyValue::Number(Animated {
                target: n,
                animation,
            })
        }
        Kind::Integer => {
            let n = v
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| invalid("expected integer"))?;
            if matches!(p, Property::TintIndex | Property::AccentTintIndex)
                && !(-1..=3).contains(&n)
            {
                return Err(invalid("tint layer index"));
            }
            PropertyValue::Integer(n)
        }
        Kind::Bool => PropertyValue::Bool(v.as_bool().ok_or_else(|| invalid("expected boolean"))?),
        Kind::Insets => PropertyValue::Insets(Insets::parse(&v)?),
        Kind::Point => {
            let a = v
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| invalid("expected pair"))?;
            PropertyValue::Point([number(&a[0])?, number(&a[1])?])
        }
        Kind::Text => {
            let s = text(&v)?;
            if *p == Property::BackgroundModifier {
                scope.palette.color(&format!("color(white {s})"))?;
            }
            PropertyValue::Text(s.into())
        }
        Kind::FontSize => {
            let size = if v.is_number() {
                FontSize::Pixels(number(&v)?)
            } else {
                let s = text(&v)?.trim();
                let (n, rem) = if let Some(s) = s.strip_suffix("rem") {
                    (s, true)
                } else {
                    (
                        s.strip_suffix("px")
                            .ok_or_else(|| invalid("font size unit"))?,
                        false,
                    )
                };
                let n = n.trim().parse::<f32>().map_err(|_| invalid("font size"))?;
                if rem {
                    FontSize::Rem(n)
                } else {
                    FontSize::Pixels(n)
                }
            };
            if !size.pixels(1.).is_finite() || size.pixels(1.) <= 0. {
                return Err(invalid("font size must be positive"));
            }
            PropertyValue::FontSize(size)
        }
        Kind::Modifier => {
            if v.is_string() {
                return Ok(PropertyValue::Modifier(parse_color(&v, scope)?.rgba8()));
            }
            let a = v
                .as_array()
                .filter(|a| a.len() == 4)
                .ok_or_else(|| invalid("tint modifier"))?;
            let mut out = [0; 4];
            for (dst, src) in out.iter_mut().zip(a) {
                *dst = src
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| invalid("tint modifier byte"))?;
            }
            PropertyValue::Modifier(out)
        }
        Kind::TitleStyle | Kind::Case | Kind::Side => {
            PropertyValue::Keyword(match (kind, text(&v)?) {
                (Kind::TitleStyle, "system") => Keyword::System,
                (Kind::TitleStyle, "dark") => Keyword::Dark,
                (Kind::TitleStyle, "light") => Keyword::Light,
                (Kind::Case, "upper") => Keyword::Upper,
                (Kind::Case, "lower") => Keyword::Lower,
                (Kind::Case, "title") => Keyword::Title,
                (Kind::Side, "left") => Keyword::Left,
                (Kind::Side, "right") => Keyword::Right,
                _ => return Err(invalid("invalid property keyword")),
            })
        }
    })
}
fn parse_color(v: &Value, scope: &Scope<'_>) -> Result<Color, Error> {
    let v = resolve_variable(v, scope.vars, 0)?;
    if let Some(s) = v.as_str() {
        return Ok(Color::parse(s, &scope.colors)?);
    }
    array_color(
        v.as_array().ok_or_else(|| invalid("expected color"))?,
        scope.palette,
    )
}
/// A color written as an array: bytes, `hsl`, or a derived palette color.
fn array_color(a: &[Value], palette: &Palette) -> Result<Color, Error> {
    if let Some(base @ ("background" | "foreground" | "accent")) = a.first().and_then(Value::as_str)
    {
        let c = palette.color(&format!("var(--{base})"))?;
        let expression = match a {
            [_, alpha] => format!("color({} a({}))", crate::palette::css(c), number(alpha)?),
            [_, mode, saturation] if mode == "grayscale" => format!(
                "color({} s(* {}%))",
                crate::palette::css(c),
                number(saturation)?
            ),
            [_, tail @ ..] if tail.len() == 4 || tail.len() == 5 => {
                let tint = array_color(tail, palette)?;
                format!(
                    "color({} blend({} {}))",
                    crate::palette::css(c),
                    crate::palette::css(tint),
                    // Array tints weight the tint by alpha; blend() weights the base.
                    1. - f32::from(tint.a) / 255.
                )
            }
            _ => return Err(invalid("derived color")),
        };
        return palette.color(&expression);
    }
    let (rgb, alpha) =
        if a.first().and_then(Value::as_str) == Some("hsl") && (a.len() == 4 || a.len() == 5) {
            (
                palette.color(&format!(
                    "hsl({},{}%,{}%)",
                    number(&a[1])?,
                    number(&a[2])?,
                    number(&a[3])?
                ))?,
                a.get(4),
            )
        } else if a.len() == 3 || a.len() == 4 {
            let mut rgb = [0; 3];
            for (dst, v) in rgb.iter_mut().zip(a) {
                *dst = v
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| invalid("color byte"))?;
            }
            (Color::rgb(rgb[0], rgb[1], rgb[2]), a.get(3))
        } else {
            return Err(invalid("color array length"));
        };
    let alpha = alpha.map(number).transpose()?.unwrap_or(1.);
    let alpha = if alpha > 1. && alpha <= 255. {
        alpha / 255.
    } else {
        alpha
    };
    if !(0. ..=1.).contains(&alpha) {
        return Err(invalid("color alpha"));
    }
    Ok(Color {
        a: (alpha * 255.).round() as u8,
        ..rgb
    })
}
