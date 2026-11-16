//! Kuvo's theme: the type scale, the colours for light and dark, radii and spacing.
//!
//! Every block reads its look from here and nowhere else, so changing a token changes every
//! screen. The rules come from the design brief:
//!
//! - SF Pro (the system font) in regular and medium only;
//! - four sizes of type: 12, 13, 14 and 24 px, all scaled together by the text size picked
//!   ([`TextSize`]);
//! - three levels of text: #292929, #5D5D5D and #9E9E9E in light, and their dark equivalents;
//! - icons at 14 px for navigation and 20 px for cards (see [`crate::icon`]);
//! - radii of 8 px for navigation, 16 px for cards, and pills for calls to action.

use std::cell::Cell;

use gpui::{
    App, BoxShadow, Context, FontWeight, Global, Hsla, ListState, Pixels, Styled, Subscription,
    Window, WindowAppearance, hsla, point, px, rgb, rgba,
};

/// Light or dark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    Light,
    Dark,
}

impl Appearance {
    /// The appearance the system asks for in this window.
    pub fn of(window: &Window) -> Self {
        Self::of_system(window.appearance())
    }

    /// Light or dark, for an appearance the system names.
    pub fn of_system(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
        }
    }

    pub fn is_dark(self) -> bool {
        self == Self::Dark
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }
}

/// The four sizes type comes in. Nothing else is allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeSize {
    /// 12 px: captions, badges, timestamps, section labels.
    Caption,
    /// 13 px: the body of dense UI: rows, menus, buttons, chat text.
    Small,
    /// 14 px: the body of reading text and fields.
    Body,
    /// 24 px: page and card titles.
    Title,
}

impl TypeSize {
    /// The size, at the text size picked ([`TextSize`]).
    pub fn size(self) -> Pixels {
        scaled(match self {
            Self::Caption => px(12.),
            Self::Small => px(13.),
            Self::Body => px(14.),
            Self::Title => px(24.),
        })
    }

    /// The line height that goes with the size: roughly 1.35 for text sizes, 1.25 for titles.
    pub fn line_height(self) -> Pixels {
        scaled(match self {
            Self::Caption => px(16.),
            Self::Small => px(18.),
            Self::Body => px(20.),
            Self::Title => px(30.),
        })
    }
}

/// How big all text is, as picked in Your settings › Appearance: four steps around the brief's
/// sizes, each a scale of every size and line height, rounded to whole pixels so text stays
/// crisp. Icons, spacing and radii stay as they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextSize {
    /// Body 13, captions 11.
    Small,
    /// The brief's sizes: body 14, captions 12.
    #[default]
    Default,
    /// Body 15, captions 13.
    Large,
    /// Body 17, captions 14.
    Larger,
}

/// The text size as a global, set only by [`TextSize::set`], which keeps it and the scale
/// ([`SCALE`]) together.
struct TextSizeNow(TextSize);

impl Global for TextSizeNow {}

thread_local! {
    /// The scale of the text size now. Text is laid out on the UI's thread only, so each app
    /// (each test, too) has its own.
    static SCALE: Cell<f32> = const { Cell::new(1.) };
}

impl TextSize {
    pub const ALL: [TextSize; 4] = [TextSize::Small, TextSize::Default, TextSize::Large, TextSize::Larger];

    pub fn scale(self) -> f32 {
        match self {
            Self::Small => 0.92,
            Self::Default => 1.,
            // (0.7's conversation text was 13, 14, 15.5 and 17.)
            Self::Large => 1.08,
            Self::Larger => 1.2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
            Self::Larger => "Larger",
        }
    }

    /// The text size now.
    pub fn of(cx: &App) -> TextSize {
        cx.try_global::<TextSizeNow>().map(|now| now.0).unwrap_or_default()
    }

    /// Sets the text size for every window, redrawn at once (lists that keep their items'
    /// heights measure them again: [`remeasure_on_text_size`]).
    pub fn set(size: TextSize, cx: &mut App) {
        SCALE.with(|scale| scale.set(size.scale()));
        cx.set_global(TextSizeNow(size));
        cx.refresh_windows();
    }
}

/// What a piece of text is in the interface, and so how it's set: the type ramp, by role. Every
/// sidebar, row and the title bar set their words through these (`.text_role(..)`), so a list's
/// names, the lines under them and its headers read the same everywhere. The sizes are the
/// brief's four; a role adds the weight and a line height of its own (a row's two lines sit
/// closer than running text's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRole {
    /// The window's title in the title bar: the strongest words there (14 medium).
    WindowTitle,
    /// Beside the title, where it is (the area's name): 13 regular, in the faint level.
    WindowContext,
    /// A sidebar's own heading ("Bots", Settings' computer): 17 medium, a step over the names
    /// in its list (the one size outside the brief's four, as Settings' heading already had it).
    PaneTitle,
    /// The name in a two-line row (a bot, a computer, a session waiting on you): 14 medium, its
    /// line tight to the one under it.
    RowTitle,
    /// The line under a row's name: 12 regular, in the second level (its status, its job).
    RowDetail,
    /// A one-line navigation row's label (a session, a status, a settings page): 13 regular.
    RowLabel,
    /// A section's label in a sidebar, and counts and times at a row's end: 12 regular.
    Meta,
}

impl TextRole {
    /// Its size, at the text size picked.
    pub fn size(self) -> Pixels {
        match self {
            Self::PaneTitle => scaled(px(17.)),
            other => other.type_size().size(),
        }
    }

    /// The size on the brief's scale it's nearest (a heading's 17 is set as its own).
    pub fn type_size(self) -> TypeSize {
        match self {
            Self::WindowTitle | Self::PaneTitle | Self::RowTitle => TypeSize::Body,
            Self::WindowContext | Self::RowLabel => TypeSize::Small,
            Self::RowDetail | Self::Meta => TypeSize::Caption,
        }
    }

    pub fn weight(self) -> FontWeight {
        match self {
            Self::WindowTitle | Self::PaneTitle | Self::RowTitle => MEDIUM,
            _ => REGULAR,
        }
    }

    /// Its line height: a row's two lines (name 18, detail 15) closer than running text's.
    pub fn line_height(self) -> Pixels {
        match self {
            Self::RowTitle => scaled(px(18.)),
            Self::RowDetail => scaled(px(15.)),
            Self::PaneTitle => scaled(px(22.)),
            other => other.type_size().line_height(),
        }
    }
}

/// A size the type scale doesn't name (a page's 17 px heading), at the text size picked.
pub fn scaled(size: Pixels) -> Pixels {
    let scale = SCALE.with(Cell::get);
    if scale == 1. { size } else { px((f32::from(size) * scale).round()) }
}

/// Measures `list`'s items again whenever the text size changes, while the subscription is kept
/// (keep it with the view, so a closed view's list goes with it).
pub fn remeasure_on_text_size<V: 'static>(list: &ListState, cx: &mut Context<V>) -> Subscription {
    let list = list.clone();
    cx.observe_global::<TextSizeNow>(move |_, _| list.remeasure())
}

/// The two weights type comes in.
pub const REGULAR: FontWeight = FontWeight::NORMAL;
/// Medium. On macOS GPUI reads SF's Medium instance as about 520 rather than 500, and CSS
/// matching sends a request for 500 down to Regular; asking for 520 lands on Medium.
#[cfg(target_os = "macos")]
pub const MEDIUM: FontWeight = FontWeight(520.);
#[cfg(not(target_os = "macos"))]
pub const MEDIUM: FontWeight = FontWeight::MEDIUM;

/// The font families. `.SystemUIFont` is SF Pro on macOS and the system's UI font elsewhere.
pub const UI_FONT: &str = ".SystemUIFont";
/// Monospaced text: SF Mono on macOS, the system's monospace elsewhere.
#[cfg(target_os = "macos")]
pub const MONO_FONT: &str = ".AppleSystemUIFontMonospaced";
#[cfg(not(target_os = "macos"))]
pub const MONO_FONT: &str = "monospace";

/// The letter spacing the brief asks for. GPUI has no tracking control yet, so this is
/// recorded here and not applied; see `notes/theme.md`.
pub const LETTER_SPACING: Pixels = px(-0.15);

/// Corner radii.
pub mod radius {
    use gpui::{Pixels, px};
    /// Small things: chips, keycaps, checkboxes.
    pub const SMALL: Pixels = px(6.);
    /// Navigation: sidebar rows, menu items, plain buttons, fields.
    pub const NAV: Pixels = px(8.);
    /// Menus and popovers: a row's 8 plus the menu's 4 px padding, so the corners nest.
    /// (An addition to the brief's three radii, for the user to confirm.)
    pub const POPOVER: Pixels = px(12.);
    /// Cards, hover cards, code blocks, the composer.
    pub const CARD: Pixels = px(16.);
    /// Calls to action, switches, badges and avatars: fully rounded.
    pub const PILL: Pixels = px(999.);
}

/// Spacing, on a 4 px grid (with 2 and 6 for the tight spots).
pub mod space {
    use gpui::{Pixels, px};
    pub const S0_5: Pixels = px(2.);
    pub const S1: Pixels = px(4.);
    pub const S1_5: Pixels = px(6.);
    pub const S2: Pixels = px(8.);
    pub const S3: Pixels = px(12.);
    pub const S4: Pixels = px(16.);
    pub const S5: Pixels = px(20.);
    pub const S6: Pixels = px(24.);
    pub const S8: Pixels = px(32.);
    pub const S10: Pixels = px(40.);
}

/// Every colour a block may use. Blocks never write a colour literal of their own.
#[derive(Clone, Debug)]
pub struct Colors {
    /// The window behind everything: the sidebar sits straight on it.
    pub background: Hsla,
    /// The dots of the dotted backdrop.
    pub backdrop_dot: Hsla,
    /// Main content and cards.
    pub surface: Hsla,
    /// Things lifted above the content: menus, popovers, hover cards.
    pub elevated: Hsla,
    /// Wells sunk into a surface: fields, code blocks, chips, icon tiles.
    pub sunken: Hsla,
    /// The soft grey the areas' sidebars and the Settings pages sit on, a step lighter than
    /// `background` (the rail and title bar); white cards and a selected row's card sit on it.
    pub sidebar: Hsla,
    /// A card of rows on the sidebar grey (a Settings group): white in light, a step up in dark.
    pub card: Hsla,
    /// A selected row or tile in the rail or a sidebar: a card of its own, not a grey wash, with
    /// a hairline (`selected_edge`) and a soft shadow ([`Theme::selected_shadow`]).
    pub selected_card: Hsla,
    pub selected_edge: Hsla,

    /// The first level of text: titles, labels, what you read.
    pub text: Hsla,
    /// The second level: descriptions, secondary labels, icons at rest.
    pub text_muted: Hsla,
    /// The third level: hints, placeholders, timestamps, disabled text.
    pub text_faint: Hsla,
    /// Outline icons at rest (the rail, sidebars, rows, ghost buttons): a mid grey between muted
    /// and faint text, every size drawn with one line ([`crate::icon::STROKE_PX`]).
    pub icon: Hsla,
    /// An icon whose place is open or on (the rail's open area, a selected row): a step up from
    /// `icon`, never the text's near-black, so it doesn't read as a bolder icon; its wash says
    /// it's the one. Dark needs a bigger step than light for the same difference.
    pub icon_on: Hsla,
    /// Text on the primary (inverted) call to action.
    pub text_on_primary: Hsla,
    /// Text on the accent colour.
    pub text_on_accent: Hsla,

    /// Hairlines between and around things.
    pub border: Hsla,
    /// A stronger edge: fields and secondary buttons.
    pub border_strong: Hsla,

    /// Washes laid over a surface under the pointer, while pressed, and when selected.
    pub hover: Hsla,
    pub pressed: Hsla,
    pub selected: Hsla,

    /// The primary call to action: inverted from the surface, dark on light and light on dark.
    pub primary: Hsla,
    pub primary_hover: Hsla,
    pub primary_pressed: Hsla,

    /// The accent: focus rings, links, the text cursor, switches that are on, unread badges.
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_pressed: Hsla,
    /// What waits on you, in words ("1 waiting on you"): the accent's hue, calmer, so it marks
    /// what's actionable without outshouting the name above it. Status that isn't actionable is
    /// never coloured.
    pub attention: Hsla,
    /// The accent, faint, behind selected text.
    pub selection: Hsla,
    /// The focus ring around a focused field.
    pub focus_ring: Hsla,
    /// Behind `inline code` in running text.
    pub inline_code: Hsla,

    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    /// A destructive button (Discard, Delete): its fill, under the pointer and pressed, each dark
    /// enough for white words on it (4.5:1).
    pub danger_fill: Hsla,
    pub danger_hover: Hsla,
    pub danger_pressed: Hsla,

    /// Behind a diff's added and removed lines, and their signs and counts.
    pub diff_added_wash: Hsla,
    pub diff_removed_wash: Hsla,
    pub diff_added: Hsla,
    pub diff_removed: Hsla,

    /// Fills for avatars without a picture, picked by name so a person keeps theirs.
    pub avatars: [Hsla; 6],

    /// Syntax colours for code.
    pub syntax: Syntax,
}

/// Syntax colours, chosen to sit calmly on [`Colors::sunken`].
#[derive(Clone, Debug)]
pub struct Syntax {
    pub plain: Hsla,
    pub comment: Hsla,
    pub keyword: Hsla,
    pub string: Hsla,
    pub number: Hsla,
    pub function: Hsla,
    pub type_name: Hsla,
    pub constant: Hsla,
    pub property: Hsla,
    pub punctuation: Hsla,
    pub line_number: Hsla,
}

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

fn hexa(value: u32) -> Hsla {
    rgba(value).into()
}

impl Colors {
    pub fn light() -> Self {
        Self {
            background: hex(0xF3F3F3),
            backdrop_dot: hexa(0x2929291F),
            surface: hex(0xFFFFFF),
            elevated: hex(0xFFFFFF),
            sunken: hex(0xF4F4F4),
            sidebar: hex(0xF8F8F8),
            card: hex(0xFFFFFF),
            selected_card: hex(0xFFFFFF),
            selected_edge: hexa(0x2929290F),

            text: hex(0x292929),
            text_muted: hex(0x5D5D5D),
            text_faint: hex(0x9E9E9E),
            icon: hex(0x7D7D7D),
            icon_on: hex(0x5D5D5D),
            text_on_primary: hex(0xFFFFFF),
            text_on_accent: hex(0xFFFFFF),

            border: hex(0xEBEBEB),
            border_strong: hex(0xDEDEDE),

            // (Selected a step over hover, light enough that the row's words lead, not its wash.)
            hover: hexa(0x29292908),
            pressed: hexa(0x2929291F),
            selected: hexa(0x29292914),

            primary: hex(0x292929),
            primary_hover: hex(0x3B3B3B),
            primary_pressed: hex(0x1C1C1C),

            // The macOS system blue, which is also the Mac app's accent unless the user picks
            // another; the accent itself is a brand decision left to the user.
            accent: hex(0x0A7AFF),
            accent_hover: hex(0x086DE6),
            accent_pressed: hex(0x0761CC),
            attention: hex(0x3466B2),
            selection: hexa(0x0A7AFF2E),
            focus_ring: hexa(0x0A7AFF66),
            inline_code: hex(0xF0F0F0),

            success: hex(0x2E9E5B),
            warning: hex(0xC7820E),
            danger: hex(0xD93B3B),
            danger_fill: hex(0xD93B3B),
            danger_hover: hex(0xC43030),
            danger_pressed: hex(0xAD2828),

            diff_added_wash: hexa(0x2E9E5B1A),
            diff_removed_wash: hexa(0xD93B3B17),
            diff_added: hex(0x237A46),
            diff_removed: hex(0xB52E2E),

            avatars: [hex(0xD9E4F5), hex(0xF5E1D3), hex(0xDCEFDF), hex(0xEDDDF3), hex(0xF3E9C9), hex(0xD7ECEE)],

            syntax: Syntax {
                plain: hex(0x292929),
                comment: hex(0x858585),
                keyword: hex(0xA6366F),
                string: hex(0x2F7D4F),
                number: hex(0xB25B12),
                function: hex(0x2F5FB3),
                type_name: hex(0x7A4BB0),
                constant: hex(0xB25B12),
                property: hex(0x3D6E8C),
                punctuation: hex(0x7A7A7A),
                line_number: hex(0xA3A3A3),
            },
        }
    }

    pub fn dark() -> Self {
        Self {
            background: hex(0x0F0F0F),
            backdrop_dot: hexa(0xFFFFFF1A),
            surface: hex(0x181818),
            elevated: hex(0x242424),
            sunken: hex(0x121212),
            sidebar: hex(0x141414),
            card: hex(0x1E1E1E),
            selected_card: hex(0x232323),
            selected_edge: hexa(0xFFFFFF12),

            // The same three steps as light, mirrored: bright, mid and dim greys.
            text: hex(0xECECEC),
            text_muted: hex(0xA3A3A3),
            text_faint: hex(0x6E6E6E),
            icon: hex(0x8F8F8F),
            icon_on: hex(0xCFCFCF),
            text_on_primary: hex(0x181818),
            text_on_accent: hex(0xFFFFFF),

            border: hex(0x262626),
            border_strong: hex(0x333333),

            hover: hexa(0xFFFFFF0A),
            pressed: hexa(0xFFFFFF1F),
            selected: hexa(0xFFFFFF17),

            primary: hex(0xECECEC),
            primary_hover: hex(0xFFFFFF),
            primary_pressed: hex(0xD4D4D4),

            accent: hex(0x2E8BFF),
            accent_hover: hex(0x4799FF),
            accent_pressed: hex(0x2479E0),
            attention: hex(0x85AEE0),
            selection: hexa(0x2E8BFF47),
            focus_ring: hexa(0x2E8BFF73),
            inline_code: hex(0x2A2A2A),

            success: hex(0x4CC27A),
            warning: hex(0xE0A43A),
            danger: hex(0xF06363),
            danger_fill: hex(0xC43030),
            danger_hover: hex(0xCC3636),
            danger_pressed: hex(0xA82727),

            diff_added_wash: hexa(0x4CC27A1F),
            diff_removed_wash: hexa(0xF0636324),
            diff_added: hex(0x6FD394),
            diff_removed: hex(0xF58080),

            avatars: [hex(0x2B3A52), hex(0x4A3528), hex(0x2A4232), hex(0x3E2F4A), hex(0x463D24), hex(0x24404A)],

            syntax: Syntax {
                plain: hex(0xE3E3E3),
                comment: hex(0x858585),
                keyword: hex(0xE884B5),
                string: hex(0x8BCF9F),
                number: hex(0xF0A86B),
                function: hex(0x82AEF0),
                type_name: hex(0xC3A0F0),
                constant: hex(0xF0A86B),
                property: hex(0x8EC3DE),
                punctuation: hex(0x8C8C8C),
                line_number: hex(0x666666),
            },
        }
    }
}

/// The theme: an appearance and its colours. Kept as a GPUI global.
#[derive(Clone, Debug)]
pub struct Theme {
    pub appearance: Appearance,
    pub colors: Colors,
}

impl Global for Theme {}

impl Theme {
    pub fn new(appearance: Appearance) -> Self {
        let colors = match appearance {
            Appearance::Light => Colors::light(),
            Appearance::Dark => Colors::dark(),
        };
        Self { appearance, colors }
    }

    /// Installs the theme for the given appearance.
    pub fn init(appearance: Appearance, cx: &mut App) {
        cx.set_global(Self::new(appearance));
    }

    /// Switches every window to the other appearance.
    pub fn set_appearance(appearance: Appearance, cx: &mut App) {
        cx.set_global(Self::new(appearance));
        cx.refresh_windows();
    }

    /// The soft shadow under things lifted above the content (menus, popovers, hover cards).
    pub fn elevated_shadow(&self) -> Vec<BoxShadow> {
        let (near, far) = if self.appearance.is_dark() {
            (hsla(0., 0., 0., 0.40), hsla(0., 0., 0., 0.35))
        } else {
            (hsla(0., 0., 0., 0.06), hsla(0., 0., 0., 0.08))
        };
        vec![
            BoxShadow {
                color: near,
                offset: point(px(0.), px(1.)),
                blur_radius: px(2.),
                spread_radius: px(0.),
                inset: false,
            },
            BoxShadow {
                color: far,
                offset: point(px(0.), px(8.)),
                blur_radius: px(24.),
                spread_radius: px(-4.),
                inset: false,
            },
        ]
    }

    /// What lifts a selected row's card ([`Colors::selected_card`]) off a sidebar: a hairline
    /// and a soft shadow under it.
    pub fn selected_shadow(&self) -> Vec<BoxShadow> {
        let soft = if self.appearance.is_dark() { hsla(0., 0., 0., 0.35) } else { hsla(0., 0., 0., 0.05) };
        vec![
            BoxShadow { color: self.colors.selected_edge, offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.), inset: false },
            BoxShadow { color: soft, offset: point(px(0.), px(1.)), blur_radius: px(3.), spread_radius: px(0.), inset: false },
        ]
    }

    /// The faint shadow that sets a card off a surface of the same colour.
    pub fn card_shadow(&self) -> Vec<BoxShadow> {
        let color = if self.appearance.is_dark() {
            hsla(0., 0., 0., 0.30)
        } else {
            hsla(0., 0., 0., 0.04)
        };
        vec![BoxShadow {
            color,
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
            inset: false,
        }]
    }
}

/// Reads the theme from anything that can see the app.
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}

/// Type helpers every styled element gets: `.text_ui(TypeSize::Small)`, `.medium()`.
pub trait StyledType: Styled + Sized {
    /// Sets the size and the line height that goes with it, in the UI font.
    fn text_ui(self, size: TypeSize) -> Self {
        self.font_family(UI_FONT)
            .text_size(size.size())
            .line_height(size.line_height())
    }

    /// Monospaced text. SF Mono runs larger than SF Pro, so it's set a step down (14 → 13,
    /// 13 → 12), never below 12.
    fn text_mono(self, size: TypeSize) -> Self {
        let mono = match size {
            TypeSize::Body => TypeSize::Small.size(),
            TypeSize::Small | TypeSize::Caption => TypeSize::Caption.size(),
            TypeSize::Title => TypeSize::Title.size(),
        };
        self.font_family(MONO_FONT).text_size(mono).line_height(size.line_height())
    }

    /// Sets the words as their role in the interface says ([`TextRole`]): size, line height and
    /// weight, in the UI font. The colour stays the caller's (selected, muted, faint).
    fn text_role(self, role: TextRole) -> Self {
        self.font_family(UI_FONT)
            .text_size(role.size())
            .line_height(role.line_height())
            .font_weight(role.weight())
    }

    fn medium(self) -> Self {
        self.font_weight(MEDIUM)
    }

    fn regular(self) -> Self {
        self.font_weight(REGULAR)
    }
}

impl<T: Styled> StyledType for T {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The brief's three text colours, exactly.
    #[test]
    fn light_text_levels_are_the_briefs() {
        let colors = Colors::light();
        assert_eq!(colors.text, hex(0x292929));
        assert_eq!(colors.text_muted, hex(0x5D5D5D));
        assert_eq!(colors.text_faint, hex(0x9E9E9E));
    }

    fn luminance(color: Hsla) -> f32 {
        let rgba = color.to_rgb();
        let channel = |c: f32| {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgba.r) + 0.7152 * channel(rgba.g) + 0.0722 * channel(rgba.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// Text people read stays readable on every surface it sits on, in both appearances: the
    /// first two levels meet WCAG AA for body text (4.5:1, and 7:1 for the first). The faint
    /// level is the brief's #9E9E9E, about 2.5:1 on light surfaces: it's kept for hints and
    /// times, never for anything that must be read.
    #[test]
    fn text_levels_stay_readable_in_both_appearances() {
        for colors in [Colors::light(), Colors::dark()] {
            for surface in [colors.surface, colors.background, colors.sunken, colors.elevated, colors.sidebar, colors.card] {
                assert!(contrast(colors.text, surface) >= 7.0);
                assert!(contrast(colors.text_muted, surface) >= 4.5);
                assert!(contrast(colors.text_faint, surface) >= 2.4);
                // An icon is a graphic: 3:1 (WCAG 1.4.11), lighter than muted text.
                assert!(contrast(colors.icon, surface) >= 3.0);
                assert!(contrast(colors.icon, surface) < contrast(colors.text_muted, surface));
            }
            assert!(contrast(colors.text_on_primary, colors.primary) >= 7.0);
            // A destructive button's words, at rest, under the pointer and pressed.
            for fill in [colors.danger_fill, colors.danger_hover, colors.danger_pressed] {
                assert!(contrast(colors.text_on_accent, fill) >= 4.5, "{fill:?}");
            }
            // What waits on you reads as words (4.5:1) on every surface it's written on, and is
            // calmer than the accent (less saturated).
            for surface in [colors.surface, colors.sidebar, colors.card, colors.background] {
                assert!(contrast(colors.attention, surface) >= 4.5, "{surface:?}");
            }
            assert!(colors.attention.s < colors.accent.s);
            // A selected row's wash is lighter than pressing one, yet still told apart from the
            // surface it's on and from a hovered row beside it, on the rail and in a sidebar.
            assert!(colors.hover.a < colors.selected.a && colors.selected.a < colors.pressed.a);
            for surface in [colors.background, colors.sidebar] {
                let (selected, hover) = (surface.blend(colors.selected), surface.blend(colors.hover));
                assert!(contrast(selected, surface) >= 1.15, "selected on {surface:?}: {}", contrast(selected, surface));
                assert!(contrast(selected, hover) >= 1.08, "selected beside hover on {surface:?}: {}", contrast(selected, hover));
            }
            // An open place's icon is told from those at rest (on its wash), without the text's
            // near-black.
            assert!(contrast(colors.icon_on, colors.icon) >= 1.4, "{}", contrast(colors.icon_on, colors.icon));
            assert!(colors.icon_on != colors.text);
            // Code stays readable on its well: comments 3:1, line numbers 2:1 at least.
            assert!(contrast(colors.syntax.comment, colors.sunken) >= 3.0);
            assert!(contrast(colors.syntax.line_number, colors.sunken) >= 2.0);
        }
    }
}
