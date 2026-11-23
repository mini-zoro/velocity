//! Cached theme resolution, finite transitions and CPU/GPU-independent widget painting.
use std::{sync::Arc, time::Instant};
use unicode_segmentation::UnicodeSegmentation;
use vel_config::settings::Settings;
use vel_platform::Rect;
use vel_render::{
    nine_slice::{self, Slices},
    scene::{self, FontStyle, Image, Layer, Shadow, SolidRect, Surface, TextHighlight, TextRun},
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
    settled: Option<Style>,
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
struct WidgetStyle {
    input: Prepared,
    settings: u64,
    key: PaintKey,
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
impl StyleData {
    /// Theme border colour and width in logical points, when the element draws one.
    pub fn border(&self) -> Option<(scene::Color, f32)> {
        self.border.map(|c| (c, self.border_size))
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
    textures: fnv::FnvHashMap<String, [Option<Texture>; 3]>,
    /// Target backing density; decoded variants are selected without frame-path I/O.
    pub density: f32,
    accessibility: vel_platform::accessibility::Preferences,
    states: fnv::FnvHashMap<u64, State>,
    setting_keys: Option<Vec<String>>,
    settings: fnv::FnvHashMap<u64, ThemeSettings>,
    calls: usize,
    misses: usize,
    unconditional_calls: usize,
    prepared: fnv::FnvHashMap<PaintKey, Prepared>,
    widgets: fnv::FnvHashMap<u64, WidgetStyle>,
    now: Instant,
    origin: Instant,
    active: bool,
    /// Ellipsized labels by (text, width, font): None when the label fits as is.
    fitted: std::cell::RefCell<fnv::FnvHashMap<u64, Option<Arc<str>>>>,
}
/// Worker-prepared theme paint data. Native font metrics and live widget caches stay local.
#[derive(Debug)]
pub struct PreparedUi {
    resolver: Resolver,
    unconditional: Option<Unconditional>,
    scheme_color: Color,
    accent_color: Color,
    palette: Palette,
    textures: fnv::FnvHashMap<String, [Option<Texture>; 3]>,
    setting_keys: Option<Vec<String>>,
    revision: u64,
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
    /// The active color scheme's background (the selected tab and the editor sheet), as
    /// straight sRGB components.
    pub fn scheme_background(&self) -> scene::Color {
        rgba(self.scheme_color)
    }
    /// The theme's accent color, as straight sRGB components.
    pub fn accent(&self) -> scene::Color {
        rgba(self.accent_color)
    }
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
            textures: Default::default(),
            density: 1.,
            accessibility: Default::default(),
            states: Default::default(),
            setting_keys: None,
            settings: Default::default(),
            calls: 0,
            misses: 0,
            unconditional_calls: 0,
            prepared: Default::default(),
            widgets: Default::default(),
            now,
            origin: now,
            active: false,
            fitted: Default::default(),
        }
    }
    /// Moves compiled themes and decoded textures across a worker boundary. No native objects move.
    pub fn into_prepared(self) -> PreparedUi {
        PreparedUi {
            resolver: self.resolver,
            unconditional: self.unconditional,
            scheme_color: self.scheme_color,
            accent_color: self.accent_color,
            palette: self.palette,
            textures: self.textures,
            setting_keys: self.setting_keys,
            revision: self.revision,
        }
    }
    /// Publishes worker-prepared paint data with fresh, thread-local metrics and widget caches.
    pub fn from_prepared(prepared: PreparedUi) -> Self {
        Self {
            resolver: prepared.resolver,
            unconditional: prepared.unconditional,
            scheme_color: prepared.scheme_color,
            accent_color: prepared.accent_color,
            palette: prepared.palette,
            textures: prepared.textures,
            setting_keys: prepared.setting_keys,
            revision: prepared.revision,
            ..Self::default()
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
        self.widgets.clear();
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
        self.widgets.clear();
        self.states.clear();
    }
    /// Supplies selector setting names collected from all loaded theme fragments. Projection is
    /// cached per immutable settings value identity; unchanged widget inputs reuse their paint style.
    pub fn set_setting_keys(&mut self, mut keys: Vec<String>) {
        keys.extend(
            [
                "ui_scale",
                "ui_font_size",
                "ui_font_face",
                "animation_enabled",
            ]
            .map(str::to_owned),
        );
        keys.sort();
        keys.dedup();
        self.setting_keys = Some(keys);
        self.settings.clear();
        self.prepared.clear();
        self.widgets.clear();
        self.resolver.clear();
    }
    /// Applies live system Reduce Motion / Increase Contrast preferences.
    pub fn set_accessibility_preferences(
        &mut self,
        preferences: vel_platform::accessibility::Preferences,
    ) {
        if self.accessibility == preferences {
            return;
        }
        self.accessibility = preferences;
        self.states.clear();
        self.widgets.clear();
        self.active = false;
        self.revision += 1;
    }
    /// Whether the native accessibility preference disables motion.
    pub fn reduce_motion(&self) -> bool {
        self.accessibility.reduce_motion
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
        self.active |= !self.accessibility.reduce_motion;
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
    /// Resolves changed widget inputs; unchanged paths/settings reuse paint data while
    /// transitions and texture sequences continue sampling the current frame clock.
    pub fn style(&mut self, id: u64, path: &ElementPath<'_>, settings: &Settings) -> Style {
        if let Some(cached) = self.widgets.get(&id)
            && cached.settings == settings.cache_identity()
            && cached.input.matches(path)
        {
            let (style, key, enabled) = (cached.input.style.clone(), cached.key, cached.animations);
            return self.animate(id, style, key, enabled);
        }
        let input_path = *path;
        let input_settings = settings.cache_identity();
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
        // A resolver eviction may recycle an address held by an older widget input.
        // Preserve its transition history, but sample the new style even if PaintKey collides.
        if self
            .widgets
            .get(&id)
            .is_some_and(|cached| cached.key == key)
            && let Some(state) = self.states.get_mut(&id)
        {
            state.key = None;
        }
        if self.prepared.len() > 4096 {
            self.prepared.clear();
            self.widgets.clear();
            self.states.clear();
        }
        // A UI font override is prepared with the style (not on each paint). Editor
        // text keeps its separate font_face: https://www.sublimetext.com/docs/settings.html
        let prepare = || {
            // Scale the theme's fixed and relative label sizes together without
            // scaling the controls themselves. The default root remains 12 pt.
            let mut style = prepare_style(
                resolved,
                scale,
                12.,
                selected,
                self.scheme_color,
                self.accent_color,
            );
            style.size *= root / 12.;
            let face = settings.string("ui_font_face", "");
            if !face.is_empty() {
                style.family = Some(Arc::from(face));
            }
            Prepared::new(&path, style)
        };
        let cached = self.prepared.entry(key).or_insert_with(|| {
            self.misses += 1;
            prepare()
        });
        // The borrowed engine address can be reused after eviction. Check complete input paths;
        // settings have a unique immutable value identity and theme replacement clears this cache.
        if !cached.matches(&path) {
            self.states.clear();
            self.misses += 1;
            *cached = prepare();
        }
        let style = cached.style.clone();
        let enabled = snapshot.animations;
        if self.widgets.len() >= 4096 {
            self.widgets.clear();
            self.states.clear();
        }
        self.widgets.insert(
            id,
            WidgetStyle {
                input: Prepared::new(&input_path, style.clone()),
                settings: input_settings,
                key,
                animations: enabled,
            },
        );
        self.animate(id, style, key, enabled)
    }
    fn animate(&mut self, id: u64, mut style: Style, key: PaintKey, enabled: bool) -> Style {
        let enabled = enabled && !self.accessibility.reduce_motion;
        if self.states.len() > 4096 {
            self.states.clear();
        }
        let state = self.states.entry(id).or_default();
        if state.key == Some(key) && !state.active {
            return state.settled.as_ref().unwrap_or(&style).clone();
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
        state.settled = (!active).then(|| style.clone());
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
        if self.accessibility.increase_contrast {
            let color = style.foreground;
            for edge in [
                Rect {
                    height: 1.5,
                    ..rect
                },
                Rect {
                    y: rect.y + rect.height - 1.5,
                    height: 1.5,
                    ..rect
                },
                Rect { width: 1.5, ..rect },
                Rect {
                    x: rect.x + rect.width - 1.5,
                    width: 1.5,
                    ..rect
                },
            ] {
                layer.rects.push(SolidRect {
                    rect: edge,
                    clip,
                    color,
                    radius: 0.,
                });
            }
        }
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
        self.border(layer, style, rect, clip);
    }
    /// Paints only the theme border. Rounded containers use one CSS box so corners follow
    /// the radius; hosts may repaint it above content that covers the surface (popups).
    pub fn border(&self, layer: &mut Layer, style: &Style, rect: Rect, clip: Rect) {
        let Some(color) = style.border.filter(|c| c[3] > 0.) else {
            return;
        };
        let edge = scene::BorderEdge {
            width: style.border_size,
            color,
        };
        layer.surfaces.push(Surface::CssBox(scene::CssBox {
            rect,
            clip,
            background: [0.; 4],
            radii: [style.radius; 4],
            borders: [edge; 4],
        }));
    }
    /// Paints an icon-like element in `rect`: solid layers fill the whole rect (hover and
    /// pressed backgrounds), while plain textures keep their intrinsic size, centered and
    /// snapped to device pixels instead of being stretched. Nine-sliced layers still stretch.
    pub fn icon_surface(&self, layer: &mut Layer, style: &Style, rect: Rect, clip: Rect) {
        let fill = |layer: &mut Layer, color| {
            layer.surfaces.push(Surface::Fill(SolidRect {
                rect,
                clip,
                color,
                radius: style.radius,
            }))
        };
        if let Some(color) = style.background {
            fill(layer, color);
        }
        for paint in style.layers.iter().take(style.layer_count) {
            if paint.color[3] <= 0. {
                continue;
            }
            let Some(resource) = &paint.texture else {
                fill(layer, paint.color);
                continue;
            };
            let Some(texture) = self.texture(resource) else {
                continue;
            };
            let sliced = paint.slices.iter().any(|m| *m > 0.);
            let target = if sliced {
                rect
            } else {
                let density = f64::from(texture.density.max(1.));
                let ui = f64::from(style.scale.max(0.01));
                let natural = (
                    f64::from(texture.image.width) / density * ui,
                    f64::from(texture.image.height) / density * ui,
                );
                let fit = (rect.width / natural.0)
                    .min(rect.height / natural.1)
                    .min(1.);
                let (width, height) = (natural.0 * fit, natural.1 * fit);
                let device = f64::from(self.density.max(1.));
                let snap = |v: f64| (v * device).round() / device;
                Rect {
                    x: snap(rect.x + (rect.width - width) / 2.),
                    y: snap(rect.y + (rect.height - height) / 2.),
                    width,
                    height,
                }
            };
            nine_slice::paint(
                layer,
                &texture.image,
                target,
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
        self.border(layer, style, rect, clip);
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
    /// Paints a label no wider than `max_width`, replacing the clipped tail with an ellipsis
    /// instead of cutting a glyph in half. Labels that fit are painted unchanged.
    #[allow(clippy::too_many_arguments)]
    pub fn text_fit(
        &self,
        layer: &mut Layer,
        style: &Style,
        text: &str,
        x: f64,
        baseline: f64,
        max_width: f64,
        clip: Rect,
    ) {
        if max_width <= 0. {
            return;
        }
        let shown = self.fitted(style, text, max_width);
        let shown = shown.as_deref().unwrap_or(text);
        if !shown.is_empty() {
            self.text(layer, style, shown, x, baseline, clip);
        }
    }
    /// Fits and paints one shaped label, using the match color for the graphemes
    /// that intersect the original label's UTF-8 match positions.
    #[allow(clippy::too_many_arguments)]
    pub fn text_fit_highlighted(
        &self,
        layer: &mut Layer,
        style: &Style,
        text: &str,
        matches: &[usize],
        x: f64,
        baseline: f64,
        max_width: f64,
        clip: Rect,
    ) {
        if matches.is_empty() {
            self.text_fit(layer, style, text, x, baseline, max_width, clip);
            return;
        }
        if max_width <= 0. {
            return;
        }
        let fitted = self.fitted(style, text, max_width);
        let shown = fitted.as_deref().unwrap_or(text);
        if shown.is_empty() {
            return;
        }
        let visible = fitted
            .as_deref()
            .and_then(|s| s.strip_suffix('…'))
            .unwrap_or(text);
        let highlight = highlight_ranges(style, &[(0, visible)], matches);
        self.draw_text(layer, style, shown, [x, baseline], clip, highlight);
    }
    /// Like [`ThemeUi::text_fit_highlighted`], but a label too long for `max_width` keeps its
    /// last segment — from the last of `separators` on, such as "：上" or ": UTF-8", the
    /// argument that tells sibling command rows apart — and shortens the part before it, the
    /// way [`ThemeUi::fit_label`] keeps a qualifier. When the segment would leave the rest
    /// less than three ems, or there is none, the end is cut as usual. Labels that fit are
    /// painted exactly as [`ThemeUi::text_fit_highlighted`] paints them.
    #[allow(clippy::too_many_arguments)]
    pub fn text_fit_highlighted_keeping(
        &self,
        layer: &mut Layer,
        style: &Style,
        text: &str,
        matches: &[usize],
        separators: &[&str],
        x: f64,
        baseline: f64,
        max_width: f64,
        clip: Rect,
    ) {
        let plain = |layer: &mut Layer| {
            self.text_fit_highlighted(layer, style, text, matches, x, baseline, max_width, clip)
        };
        if max_width <= 0. || separators.is_empty() || self.fitted(style, text, max_width).is_none()
        {
            return plain(layer);
        }
        let Some(tail) = separators
            .iter()
            .filter_map(|separator| text.rfind(separator))
            .max()
            .filter(|&tail| tail > 0)
        else {
            return plain(layer);
        };
        let (stem, segment) = text.split_at(tail);
        let room = max_width - self.advance(style, segment);
        if room < 3. * f64::from(style.size) {
            return plain(layer);
        }
        let Some(stem_shown) = self
            .fitted(style, stem, room)
            .filter(|s| s.len() > '…'.len_utf8())
        else {
            return plain(layer);
        };
        let shown = format!("{stem_shown}{segment}");
        // Shaped as one run the pieces may kern differently; never let that overflow.
        if self.advance(style, &shown) > max_width {
            return plain(layer);
        }
        let visible = stem_shown.strip_suffix('…').unwrap_or(&stem_shown);
        let highlight = highlight_ranges(
            style,
            &[(0, visible), (usize::MAX, "…"), (tail, segment)],
            matches,
        );
        self.draw_text(layer, style, &shown, [x, baseline], clip, highlight);
    }
    /// The label as [`ThemeUi::text_fit`] would paint it, keeping a trailing parenthesized
    /// qualifier such as " (Default)" or " (User)" and shortening the part before it.
    /// Empty when not even an ellipsis fits.
    pub fn fit_label<'a>(
        &self,
        style: &Style,
        text: &'a str,
        max_width: f64,
    ) -> std::borrow::Cow<'a, str> {
        if max_width <= 0. {
            return "".into();
        }
        let Some(fitted) = self.fitted(style, text, max_width) else {
            return text.into();
        };
        if let Some(at) = text.rfind(" (").filter(|_| text.ends_with(')')) {
            let (stem, suffix) = text.split_at(at);
            let room = max_width - self.advance(style, suffix);
            if room >= 3. * f64::from(style.size) {
                let stem_shown = self.fitted(style, stem, room);
                return format!("{}{suffix}", stem_shown.as_deref().unwrap_or(stem)).into();
            }
        }
        fitted.to_string().into()
    }
    /// The text as [`ThemeUi::text_fit`] would paint it: borrowed (unchanged) when it fits,
    /// otherwise owned: its longest prefix plus an ellipsis, or empty when not even an ellipsis
    /// fits.
    pub fn fit_text<'a>(
        &self,
        style: &Style,
        text: &'a str,
        max_width: f64,
    ) -> std::borrow::Cow<'a, str> {
        if max_width <= 0. {
            return String::new().into();
        }
        match self.fitted(style, text, max_width) {
            Some(shown) => shown.to_string().into(),
            None => text.into(),
        }
    }
    /// Cached fitting decision: None when `text` fits `max_width` as is.
    fn fitted(&self, style: &Style, text: &str, max_width: f64) -> Option<Arc<str>> {
        // UI glyphs (including colour emoji) stay below 1.4 em, so short labels fit unmeasured;
        // that bound is the only shortcut, however short the label is.
        let bound = text.chars().count() as f64 * f64::from(style.size) * 1.4;
        if bound <= max_width {
            return None;
        }
        // Labels are painted every frame; remember each fitting decision.
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = fnv::FnvHasher::default();
            text.hash(&mut h);
            max_width.to_bits().hash(&mut h);
            style.family.hash(&mut h);
            style.size.to_bits().hash(&mut h);
            (style.font.bold, style.font.italic).hash(&mut h);
            (std::rc::Rc::as_ptr(&self.metrics) as usize).hash(&mut h);
            #[cfg(target_os = "macos")]
            self.metrics.native_enabled().hash(&mut h);
            h.finish()
        };
        let cached = self.fitted.borrow().get(&key).cloned();
        match cached {
            Some(shown) => shown,
            None => {
                let shown = self.fit(style, text, max_width);
                let mut fitted = self.fitted.borrow_mut();
                if fitted.len() >= 2048 {
                    fitted.clear();
                }
                fitted.insert(key, shown.clone());
                shown
            }
        }
    }
    /// The ellipsized form of `text` that fits `max_width`, or None if it fits as is.
    fn fit(&self, style: &Style, text: &str, max_width: f64) -> Option<Arc<str>> {
        if self.advance(style, text) <= max_width {
            return None;
        }
        // Keep the longest complete grapheme prefix that fits with an ellipsis.
        let bounds: Vec<usize> = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .skip(1)
            .collect();
        let (mut low, mut high) = (0, bounds.len());
        while low < high {
            let mid = (low + high).div_ceil(2);
            let candidate = format!("{}\u{2026}", text[..bounds[mid - 1]].trim_end());
            if self.advance(style, &candidate) <= max_width {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        Some(if low == 0 {
            // Not even one character: an ellipsis if it fits, otherwise nothing at all
            // (never a glyph spilling over a neighbouring button or badge).
            if self.advance(style, "\u{2026}") <= max_width {
                Arc::from("\u{2026}")
            } else {
                Arc::from("")
            }
        } else {
            Arc::from(format!("{}\u{2026}", text[..bounds[low - 1]].trim_end()))
        })
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
        self.draw_text(layer, style, text, [x, baseline], clip, None);
    }
    fn draw_text(
        &self,
        layer: &mut Layer,
        style: &Style,
        text: &str,
        [x, baseline]: [f64; 2],
        clip: Rect,
        highlight: Option<Box<TextHighlight>>,
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
                highlight: None,
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
            highlight,
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

/// Match-colour ranges, in bytes of the text as painted (after the style's case transform),
/// for `pieces` painted one after another: each is `(start, text)` where `start` is the
/// piece's byte offset in the original label (`usize::MAX` for inserted text such as an
/// ellipsis) and `matches` are byte offsets of matched characters in that label. A grapheme
/// is matched as a whole when any of its bytes is.
fn highlight_ranges(
    style: &Style,
    pieces: &[(usize, &str)],
    matches: &[usize],
) -> Option<Box<TextHighlight>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let mut offset = 0;
    for &(base, piece) in pieces {
        for (start, cluster) in piece.grapheme_indices(true) {
            let next = offset
                + match style.case {
                    Some(Keyword::Upper) => cluster
                        .chars()
                        .flat_map(char::to_uppercase)
                        .map(char::len_utf8)
                        .sum::<usize>(),
                    Some(Keyword::Lower) => cluster
                        .chars()
                        .flat_map(char::to_lowercase)
                        .map(char::len_utf8)
                        .sum::<usize>(),
                    _ => cluster.len(),
                };
            let matched = base != usize::MAX && {
                let source = base + start..base + start + cluster.len();
                matches.iter().any(|byte| source.contains(byte))
            };
            if matched {
                if let Some(last) = ranges.last_mut().filter(|range| range.end == offset) {
                    last.end = next;
                } else {
                    ranges.push(offset..next);
                }
            }
            offset = next;
        }
    }
    (!ranges.is_empty()).then(|| {
        Box::new(TextHighlight {
            ranges,
            color: style.matched,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vel_theme::Element;

    /// A command caption too long for its row keeps its last segment and shortens the part
    /// before it, with the match colour on the right characters of both parts; one that fits
    /// is unchanged, and one whose segment would crowd out the rest is cut at the end.
    #[test]
    fn fitted_captions_keep_their_last_segment() {
        let mut theme = ThemeUi::default();
        let mut style = theme.style(
            1,
            &ElementPath(&[Element::new("quick_panel_label", &[])]),
            &Settings::default(),
        );
        style.family = Some(Arc::from("system-ui"));
        style.shadow = None;
        let row = Rect {
            x: 0.,
            y: 0.,
            width: 600.,
            height: 32.,
        };
        let separators = ["：", ": "];
        let paint = |text: &str, matches: &[usize], width: f64| {
            let mut layer = Layer::default();
            theme.text_fit_highlighted_keeping(
                &mut layer,
                &style,
                text,
                matches,
                &separators,
                10.,
                20.,
                width,
                row,
            );
            let run = layer.text.pop().expect("a caption");
            let ranges = run
                .highlight
                .as_ref()
                .map(|h| h.ranges.clone())
                .unwrap_or_default();
            (run.text, ranges)
        };
        for (text, separator) in [
            ("显示：编辑组：切换到相邻编辑组：上", "：上"),
            ("View: Groups: Focus Neighboring Group: Up", ": Up"),
        ] {
            let tail = text.rfind(separator).unwrap();
            // The segment's first letter ("上", "U").
            let last = tail + separator.find(char::is_alphanumeric).unwrap();
            let full = theme.advance(&style, text);
            // Fits: painted as is, matches where they are.
            let (shown, ranges) = paint(text, &[0, last], full + 1.);
            assert_eq!(shown, text);
            assert_eq!(ranges.first().map(|r| r.start), Some(0));
            // Too long: the stem is cut, the segment stays, both matches keep their colour.
            let width = full * 0.7;
            let (shown, ranges) = paint(text, &[0, last], width);
            let stem = shown.strip_suffix(separator).expect(&shown);
            let cut = stem.strip_suffix('…').expect(&shown);
            assert!(!cut.is_empty() && text.starts_with(cut), "{shown}");
            assert!(theme.advance(&style, &shown) <= width, "{shown}");
            let painted_last = shown.len() - (text.len() - last);
            assert_eq!(ranges.len(), 2, "{shown}: {ranges:?}");
            assert_eq!(ranges[0].start, 0);
            assert_eq!(ranges[1].start, painted_last, "{shown}: {ranges:?}");
            // The ellipsis itself is never in the match colour.
            let dots = stem.len() - '…'.len_utf8();
            assert!(
                ranges
                    .iter()
                    .all(|r| r.end <= dots || r.start >= stem.len())
            );
            // Room for the segment but not three ems before it: the end is cut instead.
            let segment = theme.advance(&style, separator);
            let (shown, _) = paint(text, &[], segment + 2. * f64::from(style.size));
            assert!(
                shown.ends_with('…') && !shown.ends_with(separator),
                "{shown}"
            );
        }
        // No segment to keep: the usual cut.
        let (shown, _) = paint("保存录制的宏以便稍后再次运行", &[], 60.);
        assert!(shown.ends_with('…'), "{shown}");
    }
    #[test]
    fn mixed_caption_highlighting_preserves_one_run_and_whole_graphemes() {
        let mut theme = ThemeUi::default();
        let mut style = theme.style(
            1,
            &ElementPath(&[Element::new("quick_panel_label", &[])]),
            &Settings::default(),
        );
        style.family = Some(Arc::from("system-ui"));
        style.shadow = None;
        let mut plain = Layer::default();
        let mut matched = Layer::default();
        let row = Rect {
            x: 12.,
            y: 10.,
            width: 450.,
            height: 32.,
        };
        let text = "文件：Cafe\u{301} 保存：UTF-8";
        theme.text_fit(&mut plain, &style, text, 15., 30., 440., row);
        theme.text_fit_highlighted(
            &mut matched,
            &style,
            text,
            &[
                text.find('\u{301}').unwrap(),
                text.find("保存").unwrap(),
                text.find("UTF-8").unwrap(),
            ],
            15.,
            30.,
            440.,
            row,
        );
        assert_eq!(plain.text.len(), 1);
        assert_eq!(matched.text.len(), 1);
        let (plain, matched) = (&plain.text[0], &matched.text[0]);
        assert_eq!(
            (&plain.text, plain.x, plain.baseline, plain.clip),
            (&matched.text, matched.x, matched.baseline, matched.clip)
        );
        let highlight = matched.highlight.as_ref().unwrap();
        assert_eq!(
            highlight
                .ranges
                .iter()
                .map(|span| &text[span.clone()])
                .collect::<Vec<_>>(),
            ["e\u{301}", "保", "U"]
        );
        #[cfg(target_os = "macos")]
        {
            use vel_render::font::Fonts;
            let mut fonts = Fonts::load().unwrap();
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            for scale in [1., 2.] {
                let shape = |fonts: &mut Fonts, run: &TextRun| {
                    fonts.shape_ui(
                        &run.text,
                        run.family.as_deref().unwrap(),
                        run.size * scale,
                        run.style,
                    )
                };
                let mut a = shape(&mut fonts, plain);
                while fonts.fallback_pending() {
                    assert!(Instant::now() < deadline, "font fallback did not settle");
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    fonts.poll_fallbacks();
                    a = shape(&mut fonts, plain);
                }
                let b = shape(&mut fonts, matched);
                assert_eq!(a.advance.to_bits(), b.advance.to_bits());
                assert_eq!(a.glyphs.len(), b.glyphs.len());
                for (a, b) in a.glyphs.iter().zip(&b.glyphs) {
                    assert_eq!((a.face, a.id, a.x, a.y), (b.face, b.id, b.x, b.y));
                    let [start, end] = b.source;
                    assert!(text.get(start..end).is_some());
                }
                assert!(
                    b.glyphs
                        .iter()
                        .any(|g| highlight.color_at(g.source).is_some())
                );
                assert!(f64::from(b.advance / scale + matched.x) < row.x + row.width);
            }
        }
    }
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
    fn recycled_resolver_address_cannot_reuse_a_settled_widget_style() {
        let palette = Palette::default();
        let source =
            r##"{"rules":[{"class":"button","fg":"#ff0000"},{"class":"label","fg":"#00ff00"}]}"##;
        let mut theme = ThemeUi::new(Arc::new(Theme::parse(source, &palette).unwrap()), palette);
        let settings = Settings::default();
        let button = ElementPath(&[Element::new("button", &[])]);
        let label = ElementPath(&[Element::new("label", &[])]);
        theme.style(1, &button, &settings);
        theme.style(2, &label, &settings);
        // Model the address of button's evicted engine style being reused by label's style.
        let recycled = theme.widgets[&2].key;
        theme.widgets.get_mut(&1).unwrap().key = recycled;
        theme.states.get_mut(&1).unwrap().key = Some(recycled);
        assert_eq!(
            theme.style(1, &label, &settings).foreground,
            rgba(Color::rgb(0, 255, 0))
        );
        for id in 3..=4096 {
            theme.style(id, &button, &settings);
        }
        theme.style(1, &button, &settings);
        assert!(
            theme.states.keys().all(|id| theme.widgets.contains_key(id)),
            "capacity eviction must discard states whose input paths are no longer retained"
        );
    }
    #[test]
    fn retained_widgets_invalidate_on_ancestor_settings_and_theme_changes() {
        let source = r##"{"rules":[
            {"class":"label","fg":"#112233"},
            {"class":"label","parents":[{"class":"row","attributes":["selected"]}],"fg":"#ff0000"},
            {"class":"label","settings":["bright"],"fg":"#ffffff"}
        ]}"##;
        let palette = Palette::default();
        let compiled = Arc::new(Theme::parse(source, &palette).unwrap());
        let mut retained = ThemeUi::new(compiled.clone(), palette.clone());
        let mut reference = ThemeUi::new(compiled, palette.clone());
        let mut settings = Settings::default();
        for selected in [false, true, false] {
            for bright in [false, true, false] {
                settings.set(
                    vel_config::settings::Layer::User,
                    "bright",
                    serde_json::json!(bright),
                );
                let path = ElementPath(&[
                    Element::new("window", &[]),
                    Element::new("row", if selected { &["selected"] } else { &[] }),
                    Element::new("label", &[]),
                ]);
                for _ in 0..3 {
                    reference.widgets.clear();
                    assert_eq!(
                        format!("{:?}", retained.style(7, &path, &settings)),
                        format!("{:?}", reference.style(7, &path, &settings))
                    );
                }
            }
        }
        // Replacing resources must invalidate even when settings and widget identity are retained.
        let replacement = Arc::new(
            Theme::parse(
                r##"{"rules":[{"class":"label","fg":"#00ff00"}]}"##,
                &palette,
            )
            .unwrap(),
        );
        retained.replace(replacement, palette);
        let path = ElementPath(&[Element::new("label", &[])]);
        assert_eq!(
            retained.style(7, &path, &settings).foreground,
            rgba(Color::rgb(0, 255, 0))
        );
        let calls = retained.cache_stats()[0];
        retained.style(7, &path, &settings);
        assert_eq!(retained.cache_stats()[0], calls);
        retained.set_setting_keys(vec!["bright".into()]);
        retained.style(7, &path, &settings);
        assert_eq!(retained.cache_stats()[0], calls + 1);
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
        theme.begin_frame(now + std::time::Duration::from_millis(300));
        assert_eq!(
            theme.style(1, &ElementPath(&hover), &settings).layers[0].color,
            [1.; 4]
        );
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

#[cfg(test)]
mod accessibility_tests {
    use super::*;
    #[test]
    fn reduce_motion_settles_cached_transitions_and_contrast_adds_visible_borders() {
        let palette = Palette::default();
        let theme=Theme::parse(r##"{"rules":[{"class":"button","layer0.tint":"#000000","layer0.opacity":1},{"class":"button","attributes":["hover"],"layer0.tint":{"target":"#ffffff","speed":5}}]}"##,&palette).unwrap();
        let mut ui = ThemeUi::new(Arc::new(theme), palette);
        let settings = Settings::default();
        ui.style(
            1,
            &ElementPath(&[vel_theme::Element::new("button", &[])]),
            &settings,
        );
        let hover = [vel_theme::Element::new("button", &["hover"])];
        ui.style(1, &ElementPath(&hover), &settings);
        assert!(ui.animating());
        ui.set_accessibility_preferences(vel_platform::accessibility::Preferences {
            reduce_motion: true,
            increase_contrast: true,
        });
        let style = ui.style(1, &ElementPath(&hover), &settings);
        assert!(!ui.animating());
        ui.request_animation();
        assert!(!ui.animating());
        let mut layer = Layer::default();
        ui.surface(
            &mut layer,
            &style,
            Rect {
                x: 10.,
                y: 10.,
                width: 100.,
                height: 30.,
            },
            Rect {
                x: 0.,
                y: 0.,
                width: 200.,
                height: 200.,
            },
        );
        assert!(layer.rects.len() >= 4);
        ui.set_accessibility_preferences(Default::default());
        ui.request_animation();
        assert!(ui.animating());
    }
}
