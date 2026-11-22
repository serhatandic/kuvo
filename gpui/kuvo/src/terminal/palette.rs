//! A terminal's colours, from the app's theme: its 16 ANSI colours are the theme's own calm colours
//! for light and dark (no palette of the terminal's own), its text the theme's text, and its
//! background the panel's surface, left unpainted so the terminal is part of the panel rather than a
//! slab in it. The 256-colour cube, its greys and true colour are what the program asked for.

use alacritty_terminal::term::color::Colors as Overrides;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui::{Hsla, Rgba};
use kuvo_ui::Theme;

/// The colours a terminal is drawn in, for one appearance.
#[derive(Clone, Debug)]
pub(crate) struct Palette {
    /// The 16 ANSI colours: black, red, green, yellow, blue, magenta, cyan, white, then bright.
    pub ansi: [Hsla; 16],
    pub foreground: Hsla,
    /// The panel's surface, behind cells that ask for no colour of their own.
    pub background: Hsla,
    pub cursor: Hsla,
    /// Text drawn on the cursor's block.
    pub on_cursor: Hsla,
    pub selection: Hsla,
}

impl Palette {
    pub fn of(theme: &Theme) -> Palette {
        let c = &theme.colors;
        let s = &c.syntax;
        let dark = theme.appearance.is_dark();
        // Black is the theme's text on light; on dark, a step above the surface, so a block drawn
        // in it still shows. White is the muted text on light (white on white wouldn't read).
        let (black, white, bright_white) = if dark { (c.border_strong, c.text, c.text) } else { (c.text, c.text_muted, c.text_faint) };
        Palette {
            ansi: [
                black,
                c.diff_removed,
                c.diff_added,
                c.warning,
                s.function,
                s.keyword,
                s.property,
                white,
                c.text_faint,
                c.danger,
                c.success,
                s.number,
                c.accent,
                s.type_name,
                s.property,
                bright_white,
            ],
            foreground: c.text,
            background: c.surface,
            cursor: c.accent,
            on_cursor: c.text_on_accent,
            selection: c.selection,
        }
    }

    /// What `color` is drawn in: the program's own setting of it first (OSC 4, 10, 11), else ours.
    pub fn resolve(&self, color: Color, overrides: &Overrides) -> Hsla {
        match color {
            Color::Spec(rgb) => hsla(rgb),
            Color::Indexed(index) => match overrides[index as usize] {
                Some(rgb) => hsla(rgb),
                None => self.indexed(index),
            },
            Color::Named(named) => match overrides[named] {
                Some(rgb) => hsla(rgb),
                None => self.named(named),
            },
        }
    }

    fn named(&self, named: NamedColor) -> Hsla {
        match named {
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::DimForeground => dim(self.foreground),
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            other if (other as usize) < 16 => self.ansi[other as usize],
            // Dim black … dim white.
            other => dim(self.ansi[(other as usize).saturating_sub(NamedColor::DimBlack as usize) % 8]),
        }
    }

    /// The 256 colours: the 16, then the 6×6×6 cube, then 24 greys, as xterm has them.
    fn indexed(&self, index: u8) -> Hsla {
        match index {
            0..16 => self.ansi[index as usize],
            16..232 => {
                let at = index - 16;
                let level = |step: u8| if step == 0 { 0 } else { 55 + step * 40 };
                hsla(Rgb { r: level(at / 36), g: level((at / 6) % 6), b: level(at % 6) })
            }
            _ => {
                let grey = 8 + (index - 232) * 10;
                hsla(Rgb { r: grey, g: grey, b: grey })
            }
        }
    }

    /// The colour a program asks the terminal about (OSC 4, 10, 11), as it would be drawn.
    pub fn rgb_of(&self, index: usize, overrides: &Overrides) -> Rgb {
        if let Some(rgb) = overrides[index] {
            return rgb;
        }
        let color = match index {
            0..256 => self.indexed(index as u8),
            256 => self.foreground,
            257 => self.background,
            _ => self.cursor,
        };
        let rgba: Rgba = color.into();
        let byte = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
        Rgb { r: byte(rgba.r), g: byte(rgba.g), b: byte(rgba.b) }
    }
}

/// Faint text (SGR 2): the colour, less of it.
pub(crate) fn dim(color: Hsla) -> Hsla {
    color.opacity(0.62)
}

fn hsla(rgb: Rgb) -> Hsla {
    Rgba { r: rgb.r as f32 / 255., g: rgb.g as f32 / 255., b: rgb.b as f32 / 255., a: 1. }.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuvo_ui::Appearance;

    #[test]
    fn the_ansi_colours_are_the_themes_own_in_light_and_dark() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let theme = Theme::new(appearance);
            let palette = Palette::of(&theme);
            let c = &theme.colors;
            assert_eq!(palette.ansi[1], c.diff_removed, "red");
            assert_eq!(palette.ansi[2], c.diff_added, "green");
            assert_eq!(palette.ansi[12], c.accent, "bright blue");
            assert_eq!(palette.foreground, c.text);
            assert_eq!(palette.background, c.surface, "the panel's surface, not a slab");
        }
        // Black text reads on light; a black block shows on dark.
        assert_eq!(Palette::of(&Theme::new(Appearance::Light)).ansi[0], Theme::new(Appearance::Light).colors.text);
        assert_ne!(Palette::of(&Theme::new(Appearance::Dark)).ansi[0], Theme::new(Appearance::Dark).colors.surface);
    }

    #[test]
    fn the_256_colours_and_true_colour_are_what_the_program_asked_for() {
        let palette = Palette::of(&Theme::new(Appearance::Light));
        let none = Overrides::default();
        assert_eq!(palette.resolve(Color::Indexed(196), &none), hsla(Rgb { r: 255, g: 0, b: 0 }));
        assert_eq!(palette.resolve(Color::Indexed(16), &none), hsla(Rgb { r: 0, g: 0, b: 0 }));
        assert_eq!(palette.resolve(Color::Indexed(244), &none), hsla(Rgb { r: 128, g: 128, b: 128 }));
        assert_eq!(palette.resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &none), hsla(Rgb { r: 1, g: 2, b: 3 }));
        assert_eq!(palette.resolve(Color::Indexed(4), &none), palette.ansi[4]);
        let mut set = Overrides::default();
        set[4] = Some(Rgb { r: 9, g: 9, b: 9 });
        assert_eq!(palette.resolve(Color::Named(NamedColor::Blue), &set), hsla(Rgb { r: 9, g: 9, b: 9 }), "OSC 4 set it");
    }
}
