//! Lucide icons adapted to a 16-unit grid, in Zed's sizes, all drawn with one stroke weight.
//!
//! The SVGs live in `gpui/assets/icons` and are compiled into the binary, so an icon can never
//! go missing at run time. GPUI draws an SVG as a mask, so an icon takes the colour it's given.
//!
//! An SVG's stroke grows with the size it's drawn at, so the files' one stroke would make a 22 px
//! rail icon half as heavy again as a 14 px one in a row. Each size is served its own copy with the
//! stroke set so it lands at [`STROKE_PX`] on screen (`icons/<size>/<name>.svg`): every icon in
//! the rail, the title bar, toolbars and rows has the same line.

use std::borrow::Cow;

use gpui::{AssetSource, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Styled, Window, px, svg};

/// The sizes icons come in: Zed's scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconSize {
    /// 10 px: a dot-sized mark beside text.
    Indicator,
    /// 12 px: inside small chips.
    XSmall,
    /// 14 px: navigation, rows, buttons, menus, the composer's picks.
    #[default]
    Nav,
    /// 16 px: icon buttons in the title bar and in toolbars and headers above a view.
    Medium,
    /// 18 px: the title bar's icon buttons.
    Large,
    /// 22 px: the left rail's areas and its bottom buttons.
    Rail,
    /// 20 px: cards and empty states.
    Card,
}

impl IconSize {
    /// Zed's name for the 14 px size (the same as `Nav`).
    pub const SMALL: IconSize = IconSize::Nav;
}

impl IconSize {
    pub fn pixels(self) -> Pixels {
        match self {
            Self::Indicator => px(10.),
            Self::XSmall => px(12.),
            Self::Nav => px(14.),
            Self::Medium => px(16.),
            Self::Large => px(18.),
            Self::Rail => px(22.),
            Self::Card => px(20.),
        }
    }

    /// The width its lines are drawn at on screen: [`STROKE_PX`] from 14 px up; the two sizes
    /// below it, set inside chips and beside small text, thin in step with their size.
    pub fn stroke_px(self) -> f32 {
        let size = f32::from(self.pixels());
        if size >= 14. { STROKE_PX } else { STROKE_PX * size / 14. }
    }

    /// The stroke in the file's 16-unit grid that draws [`Self::stroke_px`] at this size.
    fn stroke_units(self) -> f32 {
        self.stroke_px() * 16. / f32::from(self.pixels())
    }

    const ALL: [IconSize; 7] = [Self::Indicator, Self::XSmall, Self::Nav, Self::Medium, Self::Large, Self::Rail, Self::Card];
}

macro_rules! icons {
    ($($variant:ident => $file:literal,)*) => {
        /// Every icon Kuvo has.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum IconName { $($variant,)* }

        impl IconName {
            pub const ALL: &'static [IconName] = &[$(IconName::$variant,)*];

            /// The icon's name, as its file is called.
            pub fn name(self) -> &'static str {
                match self { $(IconName::$variant => $file,)* }
            }

            /// The asset path of its file as it is, its lines thickening with the size it's
            /// drawn at: only for showing that (the gallery's before). Draw with [`Icon`] or
            /// [`Self::path_at`], which keep one weight.
            pub fn path(self) -> &'static str {
                match self { $(IconName::$variant => concat!("icons/", $file, ".svg"),)* }
            }

            /// The asset path GPUI loads it from at `size`, its lines at [`STROKE_PX`]
            /// (`icons/<size>/<name>.svg`, a static string: icons are drawn every frame).
            pub fn path_at(self, size: IconSize) -> &'static str {
                match size {
                    IconSize::Indicator => match self { $(IconName::$variant => concat!("icons/10/", $file, ".svg"),)* },
                    IconSize::XSmall => match self { $(IconName::$variant => concat!("icons/12/", $file, ".svg"),)* },
                    IconSize::Nav => match self { $(IconName::$variant => concat!("icons/14/", $file, ".svg"),)* },
                    IconSize::Medium => match self { $(IconName::$variant => concat!("icons/16/", $file, ".svg"),)* },
                    IconSize::Large => match self { $(IconName::$variant => concat!("icons/18/", $file, ".svg"),)* },
                    IconSize::Rail => match self { $(IconName::$variant => concat!("icons/22/", $file, ".svg"),)* },
                    IconSize::Card => match self { $(IconName::$variant => concat!("icons/20/", $file, ".svg"),)* },
                }
            }

            fn bytes(self) -> &'static [u8] {
                match self {
                    $(IconName::$variant => include_bytes!(concat!("../../assets/icons/", $file, ".svg")),)*
                }
            }
        }
    };
}

icons! {
    Activity => "activity",
    Alarm => "alarm",
    Archive => "archive",
    ArrowDown => "arrow-down",
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    ArrowUp => "arrow-up",
    Bell => "bell",
    Bolt => "bolt",
    Bot => "bot",
    Branch => "branch",
    Check => "check",
    ChevronDown => "chevron-down",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ChevronUp => "chevron-up",
    Clock => "clock",
    Close => "close",
    Compose => "compose",
    Computer => "computer",
    Copy => "copy",
    Drive => "drive",
    External => "external",
    Eye => "eye",
    EyeOff => "eye-off",
    File => "file",
    Folder => "folder",
    FolderOpen => "folder-open",
    Globe => "globe",
    Hand => "hand",
    Image => "image",
    Info => "info",
    Link => "link",
    Message => "message",
    Mic => "mic",
    Minus => "minus",
    Moon => "moon",
    More => "more",
    PanelRight => "panel-right",
    Paperclip => "paperclip",
    Pencil => "pencil",
    Pin => "pin",
    Plus => "plus",
    Question => "question",
    Queue => "queue",
    QueuedMessage => "queued-message",
    Refresh => "refresh",
    Quote => "quote",
    Return => "return",
    Search => "search",
    Send => "send",
    Settings => "settings",
    Shield => "shield",
    Sidebar => "sidebar",
    SidebarLeft => "sidebar",
    Sliders => "sliders",
    Sparkle => "sparkle",
    Star => "star",
    StarFilled => "star-filled",
    Spinner => "spinner",
    Steer => "steer",
    Stop => "stop",
    Sun => "sun",
    Terminal => "terminal",
    TerminalWindow => "terminal-window",
    Trash => "trash",
    Undo => "undo",
    User => "user",
    Warning => "warning",
}

/// The services' marks, by the box's `service` id (a plan's limits): OpenUsage's provider icons
/// (MIT), as Kuvo Code ships them (`gpui/LICENSES-DEPS.md`). One colour each, drawn as a mask.
const MARKS: &[(&str, &str, &[u8])] = &[
    ("antigravity", "marks/antigravity.svg", include_bytes!("../../assets/marks/antigravity.svg")),
    ("claude", "marks/claude.svg", include_bytes!("../../assets/marks/claude.svg")),
    ("codex", "marks/codex.svg", include_bytes!("../../assets/marks/codex.svg")),
    ("copilot", "marks/copilot.svg", include_bytes!("../../assets/marks/copilot.svg")),
    ("cursor", "marks/cursor.svg", include_bytes!("../../assets/marks/cursor.svg")),
    ("grok", "marks/grok.svg", include_bytes!("../../assets/marks/grok.svg")),
    ("ollama", "marks/ollama.svg", include_bytes!("../../assets/marks/ollama.svg")),
    ("opencode", "marks/opencode.svg", include_bytes!("../../assets/marks/opencode.svg")),
    ("openrouter", "marks/openrouter.svg", include_bytes!("../../assets/marks/openrouter.svg")),
    ("zai", "marks/zai.svg", include_bytes!("../../assets/marks/zai.svg")),
];

/// The providers' marks, by models.dev's provider ids (MIT, as Kuvo Code's shared client design
/// package ships them: `swift/KuvoClient/Sources/ClientDesign/Resources/Marks.xcassets`). One
/// colour each, drawn as a mask.
const PROVIDER_MARKS: &[(&str, &str, &[u8])] = &[
    ("ai21", "marks/providers/ai21.svg", include_bytes!("../../assets/marks/providers/ai21.svg")),
    ("alibaba", "marks/providers/alibaba.svg", include_bytes!("../../assets/marks/providers/alibaba.svg")),
    ("amazon-bedrock", "marks/providers/amazon-bedrock.svg", include_bytes!("../../assets/marks/providers/amazon-bedrock.svg")),
    ("amd", "marks/providers/amd.svg", include_bytes!("../../assets/marks/providers/amd.svg")),
    ("anthropic", "marks/providers/anthropic.svg", include_bytes!("../../assets/marks/providers/anthropic.svg")),
    ("arcee", "marks/providers/arcee.svg", include_bytes!("../../assets/marks/providers/arcee.svg")),
    ("azure", "marks/providers/azure.svg", include_bytes!("../../assets/marks/providers/azure.svg")),
    ("baseten", "marks/providers/baseten.svg", include_bytes!("../../assets/marks/providers/baseten.svg")),
    ("cerebras", "marks/providers/cerebras.svg", include_bytes!("../../assets/marks/providers/cerebras.svg")),
    ("cloudflare-workers-ai", "marks/providers/cloudflare-workers-ai.svg", include_bytes!("../../assets/marks/providers/cloudflare-workers-ai.svg")),
    ("cohere", "marks/providers/cohere.svg", include_bytes!("../../assets/marks/providers/cohere.svg")),
    ("databricks", "marks/providers/databricks.svg", include_bytes!("../../assets/marks/providers/databricks.svg")),
    ("deepinfra", "marks/providers/deepinfra.svg", include_bytes!("../../assets/marks/providers/deepinfra.svg")),
    ("deepseek", "marks/providers/deepseek.svg", include_bytes!("../../assets/marks/providers/deepseek.svg")),
    ("digitalocean", "marks/providers/digitalocean.svg", include_bytes!("../../assets/marks/providers/digitalocean.svg")),
    ("fireworks-ai", "marks/providers/fireworks-ai.svg", include_bytes!("../../assets/marks/providers/fireworks-ai.svg")),
    ("friendli", "marks/providers/friendli.svg", include_bytes!("../../assets/marks/providers/friendli.svg")),
    ("github-copilot", "marks/providers/github-copilot.svg", include_bytes!("../../assets/marks/providers/github-copilot.svg")),
    ("gitlab", "marks/providers/gitlab.svg", include_bytes!("../../assets/marks/providers/gitlab.svg")),
    ("google", "marks/providers/google.svg", include_bytes!("../../assets/marks/providers/google.svg")),
    ("google-vertex", "marks/providers/google-vertex.svg", include_bytes!("../../assets/marks/providers/google-vertex.svg")),
    ("groq", "marks/providers/groq.svg", include_bytes!("../../assets/marks/providers/groq.svg")),
    ("helicone", "marks/providers/helicone.svg", include_bytes!("../../assets/marks/providers/helicone.svg")),
    ("huggingface", "marks/providers/huggingface.svg", include_bytes!("../../assets/marks/providers/huggingface.svg")),
    ("inception", "marks/providers/inception.svg", include_bytes!("../../assets/marks/providers/inception.svg")),
    ("io-net", "marks/providers/io-net.svg", include_bytes!("../../assets/marks/providers/io-net.svg")),
    ("kilo", "marks/providers/kilo.svg", include_bytes!("../../assets/marks/providers/kilo.svg")),
    ("liquid", "marks/providers/liquid.svg", include_bytes!("../../assets/marks/providers/liquid.svg")),
    ("llmgateway", "marks/providers/llmgateway.svg", include_bytes!("../../assets/marks/providers/llmgateway.svg")),
    ("lmstudio", "marks/providers/lmstudio.svg", include_bytes!("../../assets/marks/providers/lmstudio.svg")),
    ("meta", "marks/providers/meta.svg", include_bytes!("../../assets/marks/providers/meta.svg")),
    ("minimax", "marks/providers/minimax.svg", include_bytes!("../../assets/marks/providers/minimax.svg")),
    ("mistral", "marks/providers/mistral.svg", include_bytes!("../../assets/marks/providers/mistral.svg")),
    ("modal", "marks/providers/modal.svg", include_bytes!("../../assets/marks/providers/modal.svg")),
    ("moonshotai", "marks/providers/moonshotai.svg", include_bytes!("../../assets/marks/providers/moonshotai.svg")),
    ("nebius", "marks/providers/nebius.svg", include_bytes!("../../assets/marks/providers/nebius.svg")),
    ("neon", "marks/providers/neon.svg", include_bytes!("../../assets/marks/providers/neon.svg")),
    ("novita-ai", "marks/providers/novita-ai.svg", include_bytes!("../../assets/marks/providers/novita-ai.svg")),
    ("nvidia", "marks/providers/nvidia.svg", include_bytes!("../../assets/marks/providers/nvidia.svg")),
    ("oci", "marks/providers/oci.svg", include_bytes!("../../assets/marks/providers/oci.svg")),
    ("ollama-cloud", "marks/providers/ollama-cloud.svg", include_bytes!("../../assets/marks/providers/ollama-cloud.svg")),
    ("openai", "marks/providers/openai.svg", include_bytes!("../../assets/marks/providers/openai.svg")),
    ("opencode", "marks/providers/opencode.svg", include_bytes!("../../assets/marks/providers/opencode.svg")),
    ("openrouter", "marks/providers/openrouter.svg", include_bytes!("../../assets/marks/providers/openrouter.svg")),
    ("ovhcloud", "marks/providers/ovhcloud.svg", include_bytes!("../../assets/marks/providers/ovhcloud.svg")),
    ("perplexity", "marks/providers/perplexity.svg", include_bytes!("../../assets/marks/providers/perplexity.svg")),
    ("poe", "marks/providers/poe.svg", include_bytes!("../../assets/marks/providers/poe.svg")),
    ("scaleway", "marks/providers/scaleway.svg", include_bytes!("../../assets/marks/providers/scaleway.svg")),
    ("siliconflow", "marks/providers/siliconflow.svg", include_bytes!("../../assets/marks/providers/siliconflow.svg")),
    ("snowflake-cortex", "marks/providers/snowflake-cortex.svg", include_bytes!("../../assets/marks/providers/snowflake-cortex.svg")),
    ("stepfun", "marks/providers/stepfun.svg", include_bytes!("../../assets/marks/providers/stepfun.svg")),
    ("togetherai", "marks/providers/togetherai.svg", include_bytes!("../../assets/marks/providers/togetherai.svg")),
    ("venice", "marks/providers/venice.svg", include_bytes!("../../assets/marks/providers/venice.svg")),
    ("vercel", "marks/providers/vercel.svg", include_bytes!("../../assets/marks/providers/vercel.svg")),
    ("volcengine", "marks/providers/volcengine.svg", include_bytes!("../../assets/marks/providers/volcengine.svg")),
    ("wandb", "marks/providers/wandb.svg", include_bytes!("../../assets/marks/providers/wandb.svg")),
    ("watsonx", "marks/providers/watsonx.svg", include_bytes!("../../assets/marks/providers/watsonx.svg")),
    ("xai", "marks/providers/xai.svg", include_bytes!("../../assets/marks/providers/xai.svg")),
    ("xiaomi", "marks/providers/xiaomi.svg", include_bytes!("../../assets/marks/providers/xiaomi.svg")),
    ("zai", "marks/providers/zai.svg", include_bytes!("../../assets/marks/providers/zai.svg")),
];

/// Providers drawn with another's mark: the same provider in another region or plan, and a coding
/// agent by its product's (as `BrandMark.aliases`).
const PROVIDER_ALIASES: &[(&str, &str)] = &[
    ("google-vertex-anthropic", "google-vertex"),
    ("azure-cognitive-services", "azure"),
    ("alibaba-cn", "alibaba"),
    ("alibaba-coding-plan", "alibaba"),
    ("alibaba-coding-plan-cn", "alibaba"),
    ("alibaba-token-plan", "alibaba"),
    ("alibaba-token-plan-cn", "alibaba"),
    ("minimax-cn", "minimax"),
    ("minimax-coding-plan", "minimax"),
    ("minimax-cn-coding-plan", "minimax"),
    ("moonshotai-cn", "moonshotai"),
    ("kimi-code-plan-global", "moonshotai"),
    ("kimi-code-plan-cn", "moonshotai"),
    ("xiaomi-token-plan-ams", "xiaomi"),
    ("xiaomi-token-plan-cn", "xiaomi"),
    ("xiaomi-token-plan-sgp", "xiaomi"),
    ("zai-coding-plan", "zai"),
    ("zhipuai", "zai"),
    ("zhipuai-coding-plan", "zai"),
    ("cloudflare-ai-gateway", "cloudflare-workers-ai"),
    ("perplexity-agent", "perplexity"),
    ("siliconflow-cn", "siliconflow"),
    ("stepfun-ai", "stepfun"),
    ("stepfun-step-plan", "stepfun"),
    ("stepfun-ai-step-plan", "stepfun"),
    ("volcengine-coding-plan", "volcengine"),
    ("opencode-go", "opencode"),
];

/// The coding agents, by the bridge's provider ids, and the service whose mark they're drawn with.
const AGENT_MARKS: &[(&str, &str)] = &[("claude-code", "claude"), ("codex-cli", "codex"), ("grok-cli", "grok"), ("cursor", "cursor"), ("antigravity", "antigravity")];

/// The mark a model's provider is drawn with (an engine provider's, or a coding agent's), and the
/// one it shares it with, so providers drawn alike sit together; none for one Kuvo has no mark for.
pub fn provider_mark(provider: &str) -> Option<(&'static str, &'static str)> {
    let provider = provider.to_lowercase();
    if let Some((_, service)) = AGENT_MARKS.iter().find(|(id, _)| *id == provider) {
        return MARKS.iter().find(|(id, ..)| id == service).map(|(id, path, _)| (*id, *path));
    }
    let name = PROVIDER_ALIASES.iter().find(|(id, _)| *id == provider).map(|(_, to)| *to).unwrap_or(provider.as_str());
    PROVIDER_MARKS.iter().find(|(id, ..)| *id == name).map(|(id, path, _)| (*id, *path))
}

/// The colour a provider's mark is drawn in, where its maker's has one (Claude Code's orange).
pub fn provider_mark_color(provider: &str) -> Option<Hsla> {
    AGENT_MARKS.iter().find(|(id, _)| *id == provider).and_then(|(_, service)| mark_color(service))
}

/// The colour a service's mark is drawn in, where its maker's mark has one (Claude's orange);
/// `None` for a mark that's black or white, drawn in the text's colour. The user asked for marks
/// in their makers' colours (a brand question: `notes/phase-5-handover.md`).
pub fn mark_color(service: &str) -> Option<Hsla> {
    match service {
        "claude" => Some(gpui::rgb(0xD97757).into()),
        _ => None,
    }
}

/// The asset path of `service`'s mark, if Kuvo has one (a service a newer box names may not).
pub fn mark_path(service: &str) -> Option<&'static str> {
    MARKS.iter().find(|(id, ..)| *id == service).map(|(_, path, _)| *path)
}

/// Outline width in the files' 16-unit grid: what every file holds, rewritten per size at load
/// ([`STROKE_PX`]).
pub const STROKE: &str = "1.2";

/// The width every icon's lines are drawn at on screen, at 14 px and up: one weight for the rail,
/// the title bar, toolbars and rows, as SF Symbols keep one weight across sizes.
pub const STROKE_PX: f32 = 1.4;

/// An icon's file at `size`: its stroke set to draw [`STROKE_PX`] there.
fn at_size(icon: IconName, size: IconSize) -> Vec<u8> {
    let text = std::str::from_utf8(icon.bytes()).expect("an icon is UTF-8");
    let stroke = format!("stroke-width=\"{:.3}\"", size.stroke_units());
    text.replace(&format!("stroke-width=\"{STROKE}\""), &stroke).into_bytes()
}

/// The icon and size an `icons/<size>/<name>.svg` path names.
fn sized(path: &str) -> Option<(IconName, IconSize)> {
    let rest = path.strip_prefix("icons/")?;
    let (size, file) = rest.split_once('/')?;
    let size: u32 = size.parse().ok()?;
    let size = IconSize::ALL.into_iter().find(|s| f32::from(s.pixels()) as u32 == size)?;
    let name = file.strip_suffix(".svg")?;
    Some((*IconName::ALL.iter().find(|icon| icon.name() == name)?, size))
}

/// Kuvo's own mark as Kuvo Helper draws it in the menu bar (the app icon's artwork in one colour,
/// the user's pick, 11-16): black for a light theme, white for a dark one.
pub fn kuvo_mark(dark: bool) -> std::sync::Arc<gpui::Image> {
    static LIGHT: std::sync::OnceLock<std::sync::Arc<gpui::Image>> = std::sync::OnceLock::new();
    static DARK: std::sync::OnceLock<std::sync::Arc<gpui::Image>> = std::sync::OnceLock::new();
    let (cell, bytes): (_, &'static [u8]) = if dark { (&DARK, include_bytes!("../../assets/marks/kuvo-dark.png")) } else { (&LIGHT, include_bytes!("../../assets/marks/kuvo-light.png")) };
    cell.get_or_init(|| std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes.to_vec()))).clone()
}

/// The assets Kuvo's blocks need: hand this to `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, _, bytes)) = MARKS.iter().chain(PROVIDER_MARKS).find(|(_, mark, _)| *mark == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some((icon, size)) = sized(path) {
            return Ok(Some(Cow::Owned(at_size(icon, size))));
        }
        Ok(IconName::ALL
            .iter()
            .find(|icon| icon.path() == path)
            .map(|icon| Cow::Borrowed(icon.bytes())))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(IconName::ALL
            .iter()
            .map(|icon| icon.path())
            .chain(MARKS.iter().chain(PROVIDER_MARKS).map(|(_, mark, _)| *mark))
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

/// An icon, tinted with the colour it's given (the theme's `icon` grey unless told otherwise).
#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: IconSize,
    color: Option<Hsla>,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self { name, size: IconSize::Nav, color: None }
    }

    pub fn size(mut self, size: IconSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        use crate::theme::ActiveTheme;
        let color = self.color.unwrap_or(cx.theme().colors.icon);
        svg()
            .path(self.name.path_at(self.size))
            .size(self.size.pixels())
            .flex_none()
            .text_color(color)
    }
}

/// How much of a round (or square) tile's width a mark's longest side takes: the ring's inner
/// circle, an account's tile, a bot's badge. Zed puts a 16 px icon in a 24 px button, two thirds.
pub const MARK_IN_TILE: f32 = 0.63;

/// A provider's or service's mark: the one way a logo is drawn, wherever it shows.
///
/// Every file in `assets/marks` is framed to one standard (`examples/normalize_marks.rs`, held by
/// `tests/marks.rs`): a square whose size is the size the mark is drawn at, with the artwork
/// centred in it, its box never larger than the square and its optical size (a round mark looks
/// bigger than a square one of the same width) the same for every logo. So a mark's `size` is
/// like an icon's: [`IconSize`]'s for a bare mark in a row or a rail, or
/// [`Mark::in_tile`] for one inside a round or square tile.
#[derive(IntoElement)]
pub struct Mark {
    path: &'static str,
    size: Pixels,
    color: Option<Hsla>,
}

impl Mark {
    pub fn new(path: &'static str) -> Self {
        Self { path, size: IconSize::Nav.pixels(), color: None }
    }

    /// A service's mark, in its maker's colour where it has one; `None` for a service Kuvo has none for.
    pub fn of_service(service: &str) -> Option<Self> {
        let mark = Self::new(mark_path(service)?);
        Some(match mark_color(service) {
            Some(color) => mark.color(color),
            None => mark,
        })
    }

    /// Drawn at an icon's size.
    pub fn icon_size(mut self, size: IconSize) -> Self {
        self.size = size.pixels();
        self
    }

    /// Drawn at `size` (the square its artwork is framed in).
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// Drawn to sit in a round or square tile `tile` wide: [`MARK_IN_TILE`] of it.
    pub fn in_tile(mut self, tile: Pixels) -> Self {
        self.size = tile * MARK_IN_TILE;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Mark {
    fn render(self, _window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        use crate::theme::ActiveTheme;
        svg().path(self.path).size(self.size).flex_none().text_color(self.color.unwrap_or(cx.theme().colors.text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon loads through the asset source the app is given, and is an SVG GPUI can read.
    #[test]
    fn every_icon_loads_as_an_svg() {
        for icon in IconName::ALL {
            let bytes = Assets.load(icon.path()).unwrap().unwrap_or_else(|| panic!("{}", icon.name()));
            let text = std::str::from_utf8(&bytes).unwrap();
            usvg::Tree::from_data(&bytes, &usvg::Options::default()).unwrap_or_else(|error| panic!("{}: {error}", icon.name()));
            assert!(text.starts_with("<svg") && text.contains("viewBox=\"0 0 16 16\""));
        }
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
    }

    /// Every icon's file keeps the shared Lucide outline style, one stroke for the sizes' copies to
    /// rewrite.
    #[test]
    fn every_outline_icon_loads_thin() {
        for icon in IconName::ALL {
            let bytes = Assets.load(icon.path()).unwrap().unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(!text.contains("stroke-width=\"1.3\""), "{}", icon.name());
            assert!(!text.contains("transform=") && !text.contains("clipPath") && !text.contains("<mask"));
            if text.contains("stroke-width") {
                assert!(text.contains(&format!("stroke-width=\"{STROKE}\"")), "{}", icon.name());
            }
        }
    }

    /// Every icon, at every size, is drawn with the same line on screen: a 22 px rail icon's stroke
    /// is as wide as a 14 px row icon's (the reviews' "one weight").
    #[test]
    fn every_size_draws_one_line_weight() {
        for icon in IconName::ALL {
            for size in IconSize::ALL {
                let bytes = Assets.load(&icon.path_at(size)).unwrap().unwrap_or_else(|| panic!("{} at {size:?}", icon.name()));
                let text = std::str::from_utf8(&bytes).unwrap();
                usvg::Tree::from_data(&bytes, &usvg::Options::default()).unwrap_or_else(|error| panic!("{}: {error}", icon.name()));
                if let Some(at) = text.find("stroke-width=\"") {
                    let units: f32 = text[at + 14..].split('"').next().unwrap().parse().unwrap();
                    let on_screen = units * f32::from(size.pixels()) / 16.;
                    let px = f32::from(size.pixels());
                    // 1.4 px from 14 px up; at 12 and 10 px, 1.2 and 1.0.
                    let want = if px >= 14. { 1.4 } else { 1.4 * px / 14. };
                    assert!((on_screen - want).abs() < 0.01, "{} at {size:?}: {on_screen}", icon.name());
                }
            }
        }
        assert!(Assets.load("icons/23/bot.svg").unwrap().is_none());
        assert!(Assets.load("icons/14/nope.svg").unwrap().is_none());
    }

    /// Every service the box reads limits of has its mark, and it loads as an SVG.
    #[test]
    fn every_service_has_a_mark() {
        for service in ["claude", "codex", "grok", "cursor", "antigravity", "copilot", "openrouter", "zai", "ollama", "opencode"] {
            let path = mark_path(service).unwrap_or_else(|| panic!("{service}"));
            let bytes = Assets.load(path).unwrap().unwrap();
            assert!(std::str::from_utf8(&bytes).unwrap().starts_with("<svg"));
        }
        assert_eq!(mark_path("newer"), None);
    }
}
