//! Cached theme resolution, finite transitions and CPU/GPU-independent widget painting.
use std::{collections::HashMap, sync::Arc, time::Instant};
use vel_config::settings::Settings;
use vel_platform::Rect;
use vel_render::{
    nine_slice::{self, Slices},
    scene::{self, FontStyle, Image, Layer, Shadow, SolidRect, Surface, TextRun},
};
use vel_theme::{
    Animated, Appearance, Color, DecodedTexture, ElementPath, Insets, Keyword, LayerProperty as L,
    Palette, Platform, Property as P, PropertyValue as V, Resolver, Theme,
};

/// Renderer-ready sparse widget style. Shared strings/images clone without allocating.
#[derive(Clone, Debug)]
pub struct Style(Arc<StyleData>);
impl std::ops::Deref for Style {
    type Target = StyleData;
    fn deref(&self) -> &StyleData {
        &self.0
    }
}
impl std::ops::DerefMut for Style {
    fn deref_mut(&mut self) -> &mut StyleData {
        Arc::make_mut(&mut self.0)
    }
}
/// Immutable paint data shared by resolved widgets; edits use copy-on-write through `Style`.
#[derive(Clone, Debug)]
pub struct StyleData {
    /// Theme foreground.
    pub foreground: scene::Color,
    /// Matched characters, including selected-row variants.
    pub matched: scene::Color,
    /// Content insets (already UI-scaled).
    pub margin: Insets,
    /// Font family and traits.
    pub family: Option<Arc<str>>,
    /// Size in logical points.
    pub size: f32,
    /// Font weight/slant.
    pub font: FontStyle,
    /// Optional text case transform.
    pub case: Option<Keyword>,
    /// Theme corner radius.
    pub radius: f32,
    /// Requested tab height in logical points.
    pub tab_height: f32,
    /// Requested tab width.
    pub tab_width: f32,
    /// Pointer opacity threshold.
    pub hit_test_level: f32,
    layers: [PaintLayer; 4],
    layer_count: usize,
    scale: f32,
    /// Optional theme background.
    pub background: Option<scene::Color>,
    /// Native title-bar style when explicitly supplied.
    pub title_style: Option<Keyword>,
    /// Close button placement within a tab.
    pub close_left: bool,
    /// Whether a scroll area overlays its content.
    pub overlay: bool,
    /// Minimum widget width/height in logical points.
    pub min_size: [f32; 2],
    /// Optional minimap viewport tint.
    pub viewport_color: Option<scene::Color>,
    /// Minimap viewport opacity.
    pub viewport_opacity: f32,
    border: Option<scene::Color>,
    border_size: f32,
    shadow: Option<scene::Color>,
    shadow_offset: [f32; 2],
}
#[derive(Clone, Debug, Default)]
struct PaintLayer {
    color: scene::Color,
    texture: Option<Arc<str>>,
    slices: [f32; 4],
    center: bool,
    repeat: bool,
    tint: Option<Animated<Color>>,
    alpha: Option<Animated<f32>>,
    modifier: Option<[u8; 4]>,
    scheme_tint: Option<Color>,
    sequence: Option<Arc<vel_theme::Texture>>,
}
#[derive(Clone, Debug)]
struct Texture {
    image: Arc<Image>,
    density: f32,
}
#[derive(Clone, Copy, Debug)]
struct Motion {
    from: Color,
    target: Animated<Color>,
    start: Instant,
}
#[derive(Clone, Copy, Debug)]
struct Alpha {
    from: f32,
    target: Animated<f32>,
    start: Instant,
}
#[derive(Debug, Default)]
struct State {
    colors: [Option<Motion>; 5],
    alphas: [Option<Alpha>; 5],
    key: Option<PaintKey>,
    active: bool,
}
type PaintKey = (usize, u64, u32, u32, bool);
#[derive(Debug)]
struct ThemeSettings {
    values: Settings,
    scale: f32,
    root: f32,
    animations: bool,
}
#[derive(Debug)]
struct Prepared {
    path: Vec<(String, Vec<String>)>,
    style: Style,
}
/// Conservative selector for a rule that depends on settings. A match keeps the full resolver.
/// Parent order and negations may produce extra matches, never an unsafe settings-free choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsGuard {
    classes: Vec<String>,
    attributes: Vec<String>,
    parents: Vec<SettingsGuard>,
}
impl SettingsGuard {
    /// Reads a literal selector. Variable selectors require the ordinary resolver instead.
    pub fn from_rule(rule: &serde_json::Value) -> Option<Self> {
        let class = rule.get("class")?.as_str()?;
        if class.contains("var(") {
            return None;
        }
        let attributes = rule
            .get("attributes")
            .map(|a| {
                a.as_array()?
                    .iter()
                    .map(|v| v.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
            })
            .unwrap_or_else(|| Some(Vec::new()))?;
        let parents = rule
            .get("parents")
            .map(|p| {
                p.as_array()?
                    .iter()
                    .map(Self::from_rule)
                    .collect::<Option<Vec<_>>>()
            })
            .unwrap_or_else(|| Some(Vec::new()))?;
        Some(Self {
            classes: class.split_whitespace().map(str::to_owned).collect(),
            attributes,
            parents,
        })
    }
    fn element(&self, e: vel_theme::Element<'_>) -> bool {
        self.classes.iter().all(|class| {
            e.class.split_whitespace().any(|c| {
                c == class
                    || vel_theme::WIDGET_ALIASES.iter().any(|(alias, target)| {
                        *alias == c && target.split_whitespace().any(|c| c == class)
                    })
            })
        }) && self
            .attributes
            .iter()
            .all(|a| a.starts_with('!') || a.contains("var(") || e.attributes.contains(&a.as_str()))
    }
    fn matches(&self, path: &ElementPath<'_>) -> bool {
        path.0.last().is_some_and(|e| self.element(*e))
            && self.parents.iter().all(|p| {
                path.0[..path.0.len().saturating_sub(1)]
                    .iter()
                    .any(|e| p.element(*e))
            })
    }
}
#[derive(Debug)]
struct Unconditional {
    resolver: Resolver,
    guards: fnv::FnvHashMap<String, Vec<SettingsGuard>>,
}
impl Unconditional {
    fn eligible(&self, path: &ElementPath<'_>) -> bool {
        let Some(leaf) = path.0.last() else {
            return false;
        };
        for class in leaf.class.split_whitespace() {
            if self
                .guards
                .get(class)
                .is_some_and(|guards| guards.iter().any(|g| g.matches(path)))
            {
                return false;
            }
            for (_, target) in vel_theme::WIDGET_ALIASES
                .iter()
                .filter(|(alias, _)| *alias == class)
            {
                if target.split_whitespace().any(|c| {
                    self.guards
                        .get(c)
                        .is_some_and(|guards| guards.iter().any(|g| g.matches(path)))
                }) {
                    return false;
                }
            }
        }
        true
    }
}
impl Prepared {
    fn matches(&self, path: &ElementPath<'_>) -> bool {
        self.path.len() == path.0.len()
            && self.path.iter().zip(path.0).all(|((class, attrs), e)| {
                class == e.class
                    && attrs.len() == e.attributes.len()
                    && attrs.iter().zip(e.attributes).all(|(a, b)| a == b)
            })
    }
    fn new(path: &ElementPath<'_>, style: Style) -> Self {
        Self {
            path: path
                .0
                .iter()
                .map(|e| {
                    (
                        e.class.into(),
                        e.attributes.iter().map(|a| (*a).into()).collect(),
                    )
                })
                .collect(),
            style,
        }
    }
}
impl Style {
    /// Multiplies all paint channels for a finite caller-owned fade.
    pub fn multiply_opacity(&mut self, alpha: f32) {
        self.foreground[3] *= alpha;
        self.matched[3] *= alpha;
        for layer in &mut self.layers {
            layer.color[3] *= alpha;
        }
        if let Some(color) = &mut self.background {
            color[3] *= alpha;
        }
    }
}
/// One UI-thread resolver. Call `begin_frame` before painting, and request another frame only
/// while `animating` is true. Resource loading and decoding happen before `insert_texture`.
#[derive(Debug)]
pub struct ThemeUi {
    resolver: Resolver,
    unconditional: Option<Unconditional>,
    scheme_color: Color,
    accent_color: Color,
    /// Shared scheme variables for minihtml.
    pub palette: Palette,
    /// Increments when palette/theme/resources change; invalidates minihtml layouts.
    pub revision: u64,
    /// Minihtml metrics; native fonts are explicitly enabled by the application at window startup.
    pub metrics: std::rc::Rc<crate::minihtml::Metrics>,
    textures: HashMap<String, [Option<Texture>; 3]>,
    /// Target backing density; decoded variants are selected without frame-path I/O.
    pub density: f32,
    states: fnv::FnvHashMap<u64, State>,
    setting_keys: Option<Vec<String>>,
    settings: fnv::FnvHashMap<u64, ThemeSettings>,
    calls: usize,
    misses: usize,
    unconditional_calls: usize,
    prepared: fnv::FnvHashMap<PaintKey, Prepared>,
    now: Instant,
    origin: Instant,
    active: bool,
}
impl Default for ThemeUi {
    fn default() -> Self {
        static DEFAULT: std::sync::OnceLock<Arc<Theme>> = std::sync::OnceLock::new();
        let palette = Palette::default();
        Self::new(
            DEFAULT
                .get_or_init(|| Arc::new(Theme::velocity(Appearance::Dark, &Palette::default())))
                .clone(),
            palette,
        )
    }
}
impl ThemeUi {
    /// Creates a painter without initializing native fonts or a GPU.
    pub fn new(theme: Arc<Theme>, palette: Palette) -> Self {
        let now = Instant::now();
        Self {
            resolver: Resolver::new(theme),
            unconditional: None,
            scheme_color: palette
                .color("var(--background)")
                .unwrap_or(Color::rgb(20, 35, 46)),
            accent_color: palette
                .color("var(--accent)")
                .unwrap_or(Color::rgb(131, 197, 232)),
            palette,
            revision: 0,
            metrics: std::rc::Rc::new(Default::default()),
            textures: HashMap::new(),
            density: 1.,
            states: Default::default(),
            setting_keys: None,
            settings: Default::default(),
            calls: 0,
            misses: 0,
            unconditional_calls: 0,
            prepared: Default::default(),
            now,
            origin: now,
            active: false,
        }
    }
    /// Replaces the compiled theme and palette, invalidating cached styles/transitions.
    pub fn replace(&mut self, theme: Arc<Theme>, palette: Palette) {
        self.resolver.replace_theme(theme);
        self.unconditional = None;
        self.scheme_color = palette
            .color("var(--background)")
            .unwrap_or(Color::rgb(20, 35, 46));
        self.accent_color = palette
            .color("var(--accent)")
            .unwrap_or(Color::rgb(131, 197, 232));
        self.palette = palette;
        self.revision += 1;
        self.states.clear();
        self.prepared.clear();
        self.active = false;
    }
    /// Installs the same resource graph with settings-dependent rules removed. Guards must cover
    /// every removed selector; uncertain/variable selectors must retain the full resolver.
    pub fn set_unconditional(&mut self, theme: Arc<Theme>, guards: Vec<SettingsGuard>) {
        let mut by_class: fnv::FnvHashMap<String, Vec<SettingsGuard>> = Default::default();
        for guard in guards {
            if let Some(class) = guard.classes.first() {
                by_class.entry(class.clone()).or_default().push(guard);
            }
        }
        self.unconditional = Some(Unconditional {
            resolver: Resolver::new(theme),
            guards: by_class,
        });
        self.prepared.clear();
        self.states.clear();
    }
    /// Supplies selector setting names collected from all loaded theme fragments. Projection is
    /// cached per immutable settings value identity; every widget still calls the engine resolver.
    pub fn set_setting_keys(&mut self, mut keys: Vec<String>) {
        keys.extend(["ui_scale", "ui_font_size", "animation_enabled"].map(str::to_owned));
        keys.sort();
        keys.dedup();
        self.setting_keys = Some(keys);
        self.settings.clear();
        self.prepared.clear();
        self.resolver.clear();
    }
    /// Sets the monotonic frame clock. Reading a cached style never restarts a transition.
    pub fn begin_frame(&mut self, now: Instant) {
        self.now = now;
        self.active = false;
    }
    /// Resolver calls, paint conversion misses and settings-free calls, for diagnostics.
    pub fn cache_stats(&self) -> [usize; 3] {
        [self.calls, self.misses, self.unconditional_calls]
    }
    /// Requests another frame for a caller-owned finite animation, such as a scroll fade.
    pub fn request_animation(&mut self) {
        self.active = true;
    }
    /// Whether a visible transition or texture sequence requires another frame.
    pub fn animating(&self) -> bool {
        self.active
    }
    /// Retains a decoded density variant under its original, unsuffixed resource name.
    pub fn insert_texture(&mut self, name: String, image: DecodedTexture) {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        image.resource.hash(&mut hash);
        image.rgba.hash(&mut hash);
        let texture = Texture {
            density: f32::from(image.scale),
            image: Arc::new(Image {
                logical_size: image.logical_size(),
                id: hash.finish(),
                width: image.width,
                height: image.height,
                rgba: image.rgba.into(),
            }),
        };
        self.textures
            .entry(name.trim_start_matches("Packages/").into())
            .or_default()[image.scale.saturating_sub(1).min(2) as usize] = Some(texture);
    }
    /// Resolves a stack-backed ancestor path each frame, using the engine's allocation-free cache.
    pub fn style(&mut self, id: u64, path: &ElementPath<'_>, settings: &Settings) -> Style {
        let platform = if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        };
        self.calls += 1;
        if self.settings.len() > 128 {
            self.settings.clear();
        }
        let snapshot = self
            .settings
            .entry(settings.cache_identity())
            .or_insert_with(|| {
                let values = if let Some(keys) = &self.setting_keys {
                    settings.project(keys.iter().map(String::as_str))
                } else {
                    let keys = settings.effective();
                    settings.project(keys.keys().map(String::as_str))
                };
                ThemeSettings {
                    scale: ui_scale(&values) as f32,
                    root: values.number("ui_font_size", 12.).clamp(6., 72.) as f32,
                    animations: values.bool("animation_enabled", true),
                    values,
                }
            });
        let settings = &snapshot.values;
        let mut nodes = [vel_theme::Element::new("window", &[]); 34];
        let mut attributes = [""; 16];
        let mut count = 0;
        let path = if let Some(root) = path.0.first().filter(|e| {
            e.class == "window"
                && path.0.len() <= nodes.len()
                && e.attributes.len() < attributes.len()
        }) {
            for attr in root.attributes.iter().take(15) {
                attributes[count] = attr;
                count += 1;
            }
            attributes[count] = vel_theme::luminosity(self.scheme_color);
            count += 1;
            let len = path.0.len();
            nodes[..len].copy_from_slice(&path.0[..len]);
            nodes[0] = vel_theme::Element::new(root.class, &attributes[..count]);
            vel_theme::ElementPath(&nodes[..len])
        } else {
            *path
        };
        let resolved = if let Some(unconditional) = &mut self.unconditional
            && unconditional.eligible(&path)
        {
            self.unconditional_calls += 1;
            unconditional.resolver.resolve(&path, settings, platform)
        } else {
            self.resolver.resolve(&path, settings, platform)
        };
        let scale = snapshot.scale;
        let root = snapshot.root;
        let selected = path.0.iter().any(|e| e.attributes.contains(&"selected"));
        let key = (
            std::ptr::from_ref(resolved) as usize,
            settings.cache_identity(),
            scale.to_bits(),
            root.to_bits(),
            selected,
        );
        if self.prepared.len() > 4096 {
            self.prepared.clear();
            self.states.clear();
        }
        let cached = self.prepared.entry(key).or_insert_with(|| {
            self.misses += 1;
            Prepared::new(
                &path,
                prepare_style(
                    resolved,
                    scale,
                    root,
                    selected,
                    self.scheme_color,
                    self.accent_color,
                ),
            )
        });
        // The borrowed engine address can be reused after eviction. Check complete input paths;
        // settings have a unique immutable value identity and theme replacement clears this cache.
        if !cached.matches(&path) {
            self.states.clear();
            self.misses += 1;
            *cached = Prepared::new(
                &path,
                prepare_style(
                    resolved,
                    scale,
                    root,
                    selected,
                    self.scheme_color,
                    self.accent_color,
                ),
            );
        }
        let mut style = cached.style.clone();
        let enabled = snapshot.animations;
        if self.states.len() > 4096 {
            self.states.clear();
        }
        let state = self.states.entry(id).or_default();
        if state.key == Some(key) && !state.active {
            return style;
        }
        let mut active = false;
        let count = style.layer_count;
        for (index, layer) in style.layers.iter_mut().take(count).enumerate() {
            let tint = layer.tint.unwrap_or(Animated {
                target: Color::rgb(255, 255, 255),
                animation: None,
            });
            let alpha = layer.alpha.unwrap_or(Animated {
                target: if layer.tint.is_some() || layer.sequence.is_some() {
                    1.
                } else {
                    0.
                },
                animation: None,
            });
            let mut color = rgba(sample_color(
                state,
                index as u8,
                tint,
                self.now,
                enabled,
                &mut active,
            ));
            let alpha = sample_alpha(state, index as u8, alpha, self.now, enabled, &mut active);
            if let Some(c) = layer.scheme_tint {
                color = rgba(c);
            }
            if let Some(modifier) = layer.modifier {
                color = modify(color, modifier);
            }
            color[3] *= alpha;
            layer.color = color;
            if let Some(sequence) = &layer.sequence {
                let elapsed = if enabled {
                    self.now
                        .saturating_duration_since(self.origin)
                        .as_secs_f32()
                } else {
                    0.
                };
                let frame = (elapsed / sequence.frame_time) as usize;
                let at = if sequence.looping {
                    frame % sequence.keyframes.len()
                } else {
                    frame.min(sequence.keyframes.len() - 1)
                };
                layer.texture = Some(sequence.keyframes[at].clone());
                active |= enabled
                    && sequence.keyframes.len() > 1
                    && (sequence.looping || at + 1 < sequence.keyframes.len());
            }
        }
        state.key = Some(key);
        state.active = active;
        self.active |= active;
        style
    }
    fn texture(&self, resource: &str) -> Option<&Texture> {
        let variants = self
            .textures
            .get(resource.trim_start_matches("Packages/"))?;
        let index = (self.density.ceil().clamp(1., 3.) as usize) - 1;
        variants[index]
            .as_ref()
            .or_else(|| variants.iter().rev().flatten().next())
    }
    /// Paints theme layers in order, including nine-slice textures and shadows.
    pub fn surface(&self, layer: &mut Layer, style: &Style, rect: Rect, clip: Rect) {
        if let Some(color) = style.shadow {
            layer.shadows.push(Shadow {
                rect: Rect {
                    x: rect.x + f64::from(style.shadow_offset[0]) - 4.,
                    y: rect.y + f64::from(style.shadow_offset[1]) - 4.,
                    width: rect.width + 8.,
                    height: rect.height + 8.,
                },
                radius: style.radius,
                blur: 4.,
                color,
                clip,
            });
        }
        let fill = |layer: &mut Layer, rect, color| {
            layer.surfaces.push(Surface::Fill(SolidRect {
                rect,
                clip,
                color,
                radius: style.radius,
            }))
        };
        if let Some(color) = style.background {
            fill(layer, rect, color);
        }
        for paint in style.layers.iter().take(style.layer_count) {
            if paint.color[3] <= 0. {
                continue;
            }
            if let Some(resource) = &paint.texture {
                if let Some(texture) = self.texture(resource) {
                    nine_slice::paint(
                        layer,
                        &texture.image,
                        rect,
                        clip,
                        paint.color,
                        Slices {
                            density: texture.density,
                            scale: style.scale,
                            margins: paint.slices,
                            center: paint.center,
                            repeat: paint.repeat,
                        },
                    );
                }
            } else {
                fill(layer, rect, paint.color);
            }
        }
        if let Some(color) = style.border {
            let b = f64::from(style.border_size);
            for r in [
                Rect { height: b, ..rect },
                Rect {
                    y: rect.y + rect.height - b,
                    height: b,
                    ..rect
                },
                Rect { width: b, ..rect },
                Rect {
                    x: rect.x + rect.width - b,
                    width: b,
                    ..rect
                },
            ] {
                fill(layer, r, color);
            }
        }
    }
    /// Measures a UI label through the exact shaping provider used by minihtml and painting.
    pub fn advance(&self, style: &Style, text: &str) -> f64 {
        use vel_minihtml::FontMetrics;
        let family = style
            .family
            .clone()
            .unwrap_or_else(|| Arc::from("monospace"));
        let font = vel_minihtml::TextStyle {
            font_family: family,
            font_size: style.size,
            font_weight: if style.font.bold { 700 } else { 400 },
            italic: style.font.italic,
            underline: false,
            color: Color::rgb(0, 0, 0),
        };
        f64::from(self.metrics.measure(text, &font).advance)
    }
    /// Paints a theme-styled label at an explicit baseline, retaining its clipping geometry.
    pub fn text(
        &self,
        layer: &mut Layer,
        style: &Style,
        text: &str,
        x: f64,
        baseline: f64,
        clip: Rect,
    ) {
        let text = match style.case {
            Some(Keyword::Upper) => text.to_uppercase(),
            Some(Keyword::Lower) => text.to_lowercase(),
            _ => text.replace(['\n', '\r', '\t'], " "),
        };
        if let Some(color) = style.shadow {
            layer.text.push(TextRun {
                family: style.family.clone(),
                style: style.font,
                text: text.clone(),
                x: x as f32 + style.shadow_offset[0],
                baseline: baseline as f32 + style.shadow_offset[1],
                size: style.size,
                color,
                clip,
            });
        }
        layer.text.push(TextRun {
            family: style.family.clone(),
            style: style.font,
            text,
            x: x as f32,
            baseline: baseline as f32,
            size: style.size,
            color: style.foreground,
            clip,
        });
    }
    /// Paints a mapped file icon from a resource without another file read or upload allocation.
    pub fn icon(
        &self,
        layer: &mut Layer,
        resource: &str,
        rect: Rect,
        clip: Rect,
        color: scene::Color,
    ) -> bool {
        let Some(texture) = self.texture(resource) else {
            return false;
        };
        nine_slice::paint(
            layer,
            &texture.image,
            rect,
            clip,
            color,
            Slices {
                density: texture.density,
                scale: 1.,
                margins: [0.; 4],
                center: true,
                repeat: false,
            },
        );
        true
    }
}
fn prepare_style(
    style: &vel_theme::ResolvedStyle,
    scale: f32,
    root: f32,
    selected: bool,
    scheme_color: Color,
    accent_color: Color,
) -> Style {
    let foreground = style
        .color(if selected { &P::SelectedFg } else { &P::Fg })
        .or_else(|| style.color(&P::Fg))
        .unwrap_or(Color::rgb(220, 226, 231));
    let opacity = style.number(&P::Opacity).unwrap_or(1.);
    let layers: [PaintLayer; 4] = std::array::from_fn(|index| {
        let i = index as u8;
        let tint = match style.get(&P::Layer(i, L::Tint)) {
            Some(V::Color(c)) => Some(*c),
            _ => None,
        };
        let alpha = match style.get(&P::Layer(i, L::Opacity)) {
            Some(V::Number(n)) => Some(*n),
            _ => None,
        };
        let sequence = match style.get(&P::Layer(i, L::Texture)) {
            Some(V::Texture(t)) => Some(Arc::new(t.clone())),
            _ => None,
        };
        let mut layer = PaintLayer {
            color: [0.; 4],
            texture: None,
            slices: style
                .insets(&P::Layer(i, L::InnerMargin))
                .map_or([0.; 4], |m| [m.left, m.top, m.right, m.bottom]),
            center: style.boolean(&P::Layer(i, L::DrawCenter)).unwrap_or(true),
            repeat: style.boolean(&P::Layer(i, L::Repeat)).unwrap_or(false),
            tint,
            alpha,
            modifier: match style.get(&P::Layer(i, L::TintModifier)).or_else(|| {
                if style.number(&P::TintIndex) == Some(f32::from(i)) {
                    style.get(&P::TintModifier)
                } else if style.number(&P::AccentTintIndex) == Some(f32::from(i)) {
                    style.get(&P::AccentTintModifier)
                } else {
                    None
                }
            }) {
                Some(V::Modifier(m)) => Some(*m),
                _ => None,
            },
            scheme_tint: if style.number(&P::TintIndex) == Some(f32::from(i)) {
                Some(style.color(&P::ColorSchemeTint).unwrap_or(scheme_color))
            } else if style.number(&P::AccentTintIndex) == Some(f32::from(i)) {
                Some(style.color(&P::ColorSchemeTint2).unwrap_or(accent_color))
            } else {
                None
            },
            sequence,
        };
        layer.color = rgba(
            layer
                .scheme_tint
                .or_else(|| layer.tint.map(|t| t.target))
                .unwrap_or(Color::rgb(255, 255, 255)),
        );
        if let Some(modifier) = layer.modifier {
            layer.color = modify(layer.color, modifier);
        }
        layer.color[3] *= layer.alpha.map_or(
            if layer.tint.is_some() || layer.sequence.is_some() {
                1.
            } else {
                0.
            },
            |a| a.target,
        );
        layer.texture = layer
            .sequence
            .as_ref()
            .and_then(|s| s.keyframes.first())
            .cloned();
        layer
    });
    let margin = style.insets(&P::ContentMargin).unwrap_or_default();
    Style(Arc::new(StyleData {
        scale,
        foreground: {
            let mut c = rgba(foreground);
            c[3] *= opacity;
            c
        },
        matched: rgba(
            style
                .color(if selected {
                    &P::SelectedMatchFg
                } else {
                    &P::MatchFg
                })
                .or_else(|| style.color(&P::MatchFg))
                .unwrap_or(foreground),
        ),
        margin: Insets {
            left: margin.left * scale,
            top: margin.top * scale,
            right: margin.right * scale,
            bottom: margin.bottom * scale,
        },
        family: match style.get(&P::FontFace) {
            Some(V::Text(f)) => Some(f.clone()),
            _ => None,
        },
        size: match style.get(&P::FontSize) {
            Some(V::FontSize(s)) => s.pixels(root) * scale,
            _ => root * scale,
        },
        font: FontStyle {
            bold: style.boolean(&P::FontBold).unwrap_or(false),
            italic: style.boolean(&P::FontItalic).unwrap_or(false),
        },
        case: match style.get(&P::Case) {
            Some(V::Keyword(k)) => Some(*k),
            _ => None,
        },
        radius: style.number(&P::Radius).unwrap_or(0.) * scale,
        tab_height: style.number(&P::TabHeight).unwrap_or(32.) * scale,
        tab_width: style.number(&P::TabWidth).unwrap_or(180.) * scale,
        hit_test_level: style.number(&P::HitTestLevel).unwrap_or(0.),
        layer_count: layers
            .iter()
            .rposition(|l| l.tint.is_some() || l.alpha.is_some() || l.sequence.is_some())
            .map_or(0, |i| i + 1),
        layers,
        title_style: match style.get(&P::TitleStyle) {
            Some(V::Keyword(k)) => Some(*k),
            _ => None,
        },
        overlay: style.boolean(&P::Overlay).unwrap_or(true),
        min_size: match style.get(&P::MinSize) {
            Some(V::Point(p)) => p.map(|n| n * scale),
            _ => [0.; 2],
        },
        viewport_color: style.color(&P::ViewportColor).map(rgba),
        viewport_opacity: style.number(&P::ViewportOpacity).unwrap_or(0.15),
        close_left: matches!(
            style.get(&P::CloseButtonSide),
            Some(V::Keyword(Keyword::Left))
        ),
        background: style
            .color(&P::BackgroundColor)
            .or_else(|| style.color(&P::Bg))
            .map(rgba),
        border: style.color(&P::BorderColor).map(rgba),
        border_size: style.number(&P::BorderSize).unwrap_or(1.) * scale,
        shadow: style.color(&P::ShadowColor).map(rgba),
        shadow_offset: match style.get(&P::ShadowOffset) {
            Some(V::Point(p)) => p.map(|n| n * scale),
            _ => [0., 2. * scale],
        },
    }))
}
/// Sanitized UI scale. Zero/automatic use the platform's logical DIP scale.
pub fn ui_scale(settings: &Settings) -> f64 {
    let n = settings.number("ui_scale", 1.);
    if n <= 0. || !n.is_finite() {
        1.
    } else {
        n.clamp(0.5, 3.)
    }
}
/// Converts engine byte RGBA to scene channels.
pub fn rgba(color: Color) -> scene::Color {
    [color.r, color.g, color.b, color.a].map(|c| f32::from(c) / 255.)
}
fn modify(mut color: scene::Color, m: [u8; 4]) -> scene::Color {
    let a = f32::from(m[3]) / 255.;
    for i in 0..3 {
        color[i] = color[i] * (1. - a) + f32::from(m[i]) / 255. * a;
    }
    color
}
fn sample_color(
    state: &mut State,
    key: u8,
    target: Animated<Color>,
    now: Instant,
    enabled: bool,
    active: &mut bool,
) -> Color {
    let entry = state.colors[usize::from(key)].get_or_insert(Motion {
        from: target.target,
        target,
        start: now,
    });
    if entry.target != target {
        entry.from = entry.target.sample(
            entry.from,
            now.saturating_duration_since(entry.start).as_secs_f32(),
        );
        entry.target = target;
        entry.start = now;
    }
    let elapsed = now.saturating_duration_since(entry.start).as_secs_f32();
    *active |= enabled
        && entry.from != target.target
        && target.animation.is_some_and(|a| elapsed * a.speed < 1.);
    if enabled {
        target.sample(entry.from, elapsed)
    } else {
        target.target
    }
}
fn sample_alpha(
    state: &mut State,
    key: u8,
    target: Animated<f32>,
    now: Instant,
    enabled: bool,
    active: &mut bool,
) -> f32 {
    let entry = state.alphas[usize::from(key)].get_or_insert(Alpha {
        from: target.target,
        target,
        start: now,
    });
    if entry.target != target {
        entry.from = entry.target.sample(
            entry.from,
            now.saturating_duration_since(entry.start).as_secs_f32(),
        );
        entry.target = target;
        entry.start = now;
    }
    let elapsed = now.saturating_duration_since(entry.start).as_secs_f32();
    *active |= enabled
        && entry.from != target.target
        && target.animation.is_some_and(|a| elapsed * a.speed < 1.);
    if enabled {
        target.sample(entry.from, elapsed)
    } else {
        target.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_theme::Element;
    #[test]
    fn settings_free_route_matches_full_engine_for_states_parents_aliases_and_settings() {
        let mut source = serde_json::json!({"rules":[
            {"class":"sidebar_label","fg":"#667788"},
            {"class":"sidebar_label","parents":[{"class":"tree_row","attributes":["expandable"]}],"settings":["bold_folder_labels"],"fg":"#ff0000"},
            {"class":"text_line_control","attributes":["focused","!disabled"],"parents":[{"class":"panel_control"}],"settings":{"emphasize":true},"fg":"#00ff00"},
            {"class":"auto_complete_label auto_complete_hint","settings":["emphasize"],"fg":"#0000ff"}
        ]});
        let palette = Palette::default();
        let full = Arc::new(Theme::parse(&source.to_string(), &palette).unwrap());
        let guards = source["rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r.get("settings").is_some())
            .map(|r| SettingsGuard::from_rule(r).unwrap())
            .collect();
        source["rules"]
            .as_array_mut()
            .unwrap()
            .retain(|r| r.get("settings").is_none());
        let mut plain = ThemeUi::new(full.clone(), palette.clone());
        let mut fast = ThemeUi::new(full, palette.clone());
        fast.set_unconditional(
            Arc::new(Theme::parse(&source.to_string(), &palette).unwrap()),
            guards,
        );
        for enabled in [false, true] {
            let mut settings = Settings::default();
            for key in ["bold_folder_labels", "emphasize"] {
                settings.set(
                    vel_config::settings::Layer::User,
                    key,
                    serde_json::json!(enabled),
                );
            }
            for class in [
                "sidebar_label",
                "text_input",
                "auto_complete_label",
                "auto_complete_label auto_complete_hint",
            ] {
                for parent in ["open_file", "tree_row", "panel_control"] {
                    for attrs in [
                        &[][..],
                        &["expandable"][..],
                        &["focused"][..],
                        &["focused", "disabled"][..],
                    ] {
                        let path = ElementPath(&[
                            Element::new("window", &[]),
                            Element::new(parent, attrs),
                            Element::new(class, attrs),
                        ]);
                        assert_eq!(
                            format!("{:?}", plain.style(1, &path, &settings)),
                            format!("{:?}", fast.style(1, &path, &settings)),
                            "{class} {parent} {attrs:?} {enabled}"
                        );
                    }
                }
            }
        }
        assert!(fast.cache_stats()[2] > 0 && fast.cache_stats()[2] < fast.cache_stats()[0]);
    }
    #[test]
    fn cached_styles_keep_continuous_transitions_and_stop_at_the_endpoint() {
        let source = r##"{"rules":[{"class":"button","layer0.tint":"#000000","layer0.opacity":1},{"class":"button","attributes":["hover"],"layer0.tint":{"target":"#ffffff","speed":5,"interpolation":"smoothstep"}}]}"##;
        let palette = Palette::default();
        let mut theme = ThemeUi::new(Arc::new(Theme::parse(source, &palette).unwrap()), palette);
        let settings = Settings::default();
        let now = Instant::now();
        let normal = [Element::new("window", &[]), Element::new("button", &[])];
        let hover = [normal[0], Element::new("button", &["hover"])];
        theme.begin_frame(now);
        theme.style(1, &ElementPath(&normal), &settings);
        theme.style(1, &ElementPath(&hover), &settings);
        theme.begin_frame(now + std::time::Duration::from_millis(100));
        let halfway = theme.style(1, &ElementPath(&hover), &settings);
        assert!(theme.animating());
        assert!((halfway.layers[0].color[0] - 0.5).abs() < 0.01);
        theme.begin_frame(now + std::time::Duration::from_millis(200));
        let end = theme.style(1, &ElementPath(&hover), &settings);
        assert_eq!(end.layers[0].color, [1.; 4]);
        assert!(!theme.animating());
        let mut disabled = settings.clone();
        disabled.set(
            vel_config::settings::Layer::User,
            "animation_enabled",
            serde_json::json!(false),
        );
        theme.style(1, &ElementPath(&normal), &disabled);
        let end = theme.style(1, &ElementPath(&hover), &disabled);
        assert_eq!(end.layers[0].color, [1.; 4]);
        assert!(!theme.animating());
    }
}
