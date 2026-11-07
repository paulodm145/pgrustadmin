//! Dark and light themes from `docs/design-system/tokens.md`, adapted to the
//! terminal's color support.

use std::ffi::OsStr;

use ratatui::style::{Color, Modifier, Style};

pub const THEME_ENV: &str = "PGRUSTADMIN_TUI_THEME";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeName {
    Dark,
    Light,
}

impl ThemeName {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// Parses `PGRUSTADMIN_TUI_THEME`; `None` means "use the default".
pub fn parse_theme_name(value: Option<&OsStr>) -> Result<Option<ThemeName>, ()> {
    match value.map(|value| value.to_str().map(str::to_ascii_lowercase)) {
        None => Ok(None),
        Some(Some(name)) if name == "dark" => Ok(Some(ThemeName::Dark)),
        Some(Some(name)) if name == "light" => Ok(Some(ThemeName::Light)),
        Some(_) => Err(()),
    }
}

/// How colors can be shown in the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Indexed256,
    /// `NO_COLOR` is set: emphasis uses only bold/reverse/underline.
    Monochrome,
}

/// Picks the color mode from `NO_COLOR` and `COLORTERM`, never assuming true
/// color unless the terminal announces it.
pub fn detect_color_mode(no_color: Option<&OsStr>, colorterm: Option<&OsStr>) -> ColorMode {
    if no_color.is_some_and(|value| !value.is_empty()) {
        return ColorMode::Monochrome;
    }
    match colorterm.and_then(OsStr::to_str) {
        Some(value) if value.eq_ignore_ascii_case("truecolor") => ColorMode::TrueColor,
        Some(value) if value.eq_ignore_ascii_case("24bit") => ColorMode::TrueColor,
        _ => ColorMode::Indexed256,
    }
}

type Rgb = (u8, u8, u8);

/// Token colors; semantic colors meet 4.5:1 contrast on background and surface.
#[derive(Clone, Copy, Debug)]
struct Palette {
    background: Rgb,
    surface: Rgb,
    text_primary: Rgb,
    text_secondary: Rgb,
    /// Background of selected items, under `selection_text`.
    selection: Rgb,
    selection_text: Rgb,
    /// Borders of focused panes; 3:1 against the background.
    focus_border: Rgb,
    accent: Rgb,
    success: Rgb,
    warning: Rgb,
    error: Rgb,
}

const DARK: Palette = Palette {
    background: (0x17, 0x19, 0x1F),
    surface: (0x22, 0x26, 0x31),
    text_primary: (0xF3, 0xF5, 0xF7),
    text_secondary: (0xAA, 0xB2, 0xBF),
    selection: (0x00, 0x5F, 0x6A),
    selection_text: (0xF3, 0xF5, 0xF7),
    focus_border: (0x08, 0x79, 0x85),
    accent: (0x6C, 0xB6, 0xE0),
    success: (0x5C, 0xC9, 0x8F),
    warning: (0xE8, 0xB8, 0x5A),
    error: (0xF2, 0x78, 0x7A),
};

const LIGHT: Palette = Palette {
    background: (0xF5, 0xF7, 0xFA),
    surface: (0xFF, 0xFF, 0xFF),
    text_primary: (0x20, 0x26, 0x31),
    text_secondary: (0x59, 0x65, 0x75),
    selection: (0x00, 0x5F, 0x6A),
    selection_text: (0xFF, 0xFF, 0xFF),
    focus_border: (0x00, 0x5F, 0x6A),
    accent: (0x0B, 0x5F, 0x96),
    success: (0x17, 0x70, 0x3F),
    warning: (0x8A, 0x55, 0x00),
    error: (0xB4, 0x23, 0x18),
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    name: ThemeName,
    mode: ColorMode,
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(ThemeName::Dark, ColorMode::TrueColor)
    }
}

impl Theme {
    pub const fn new(name: ThemeName, mode: ColorMode) -> Self {
        Self { name, mode }
    }

    pub fn name(self) -> ThemeName {
        self.name
    }

    pub fn toggled(self) -> Self {
        let name = match self.name {
            ThemeName::Dark => ThemeName::Light,
            ThemeName::Light => ThemeName::Dark,
        };
        Self::new(name, self.mode)
    }

    fn palette(self) -> Palette {
        match self.name {
            ThemeName::Dark => DARK,
            ThemeName::Light => LIGHT,
        }
    }

    fn color(self, rgb: Rgb) -> Color {
        match self.mode {
            ColorMode::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
            ColorMode::Indexed256 => Color::Indexed(nearest_256(rgb)),
            ColorMode::Monochrome => Color::Reset,
        }
    }

    fn foreground(self, rgb: Rgb) -> Style {
        match self.mode {
            ColorMode::Monochrome => Style::default(),
            _ => Style::default().fg(self.color(rgb)),
        }
    }

    /// Whole-screen base: primary text on the theme background.
    pub fn base(self) -> Style {
        let palette = self.palette();
        match self.mode {
            ColorMode::Monochrome => Style::default(),
            _ => Style::default()
                .fg(self.color(palette.text_primary))
                .bg(self.color(palette.background)),
        }
    }

    /// Popups drawn above the base background.
    pub fn surface(self) -> Style {
        let palette = self.palette();
        match self.mode {
            ColorMode::Monochrome => Style::default(),
            _ => Style::default()
                .fg(self.color(palette.text_primary))
                .bg(self.color(palette.surface)),
        }
    }

    /// Selected item in lists, tabs and menus.
    pub fn selection(self) -> Style {
        let palette = self.palette();
        match self.mode {
            ColorMode::Monochrome => {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            }
            _ => Style::default()
                .fg(self.color(palette.selection_text))
                .bg(self.color(palette.selection))
                .add_modifier(Modifier::BOLD),
        }
    }

    /// Focused input field or heading.
    pub fn focus(self) -> Style {
        self.foreground(self.palette().accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn focus_border(self) -> Style {
        match self.mode {
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::BOLD),
            _ => self.foreground(self.palette().focus_border),
        }
    }

    pub fn accent(self) -> Style {
        self.foreground(self.palette().accent)
    }

    pub fn muted(self) -> Style {
        match self.mode {
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::DIM),
            _ => self.foreground(self.palette().text_secondary),
        }
    }

    pub fn success(self) -> Style {
        self.foreground(self.palette().success)
    }

    pub fn warning(self) -> Style {
        self.foreground(self.palette().warning)
    }

    pub fn error(self) -> Style {
        match self.mode {
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::BOLD),
            _ => self.foreground(self.palette().error),
        }
    }
}

/// Nearest color in the xterm 256-color palette (6×6×6 cube or gray ramp).
fn nearest_256(rgb: Rgb) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let distance = |candidate: Rgb| {
        let delta = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        delta(rgb.0, candidate.0) + delta(rgb.1, candidate.1) + delta(rgb.2, candidate.2)
    };
    let nearest_level = |value: u8| {
        (0..LEVELS.len())
            .min_by_key(|index| (i32::from(LEVELS[*index]) - i32::from(value)).abs())
            .unwrap_or(0)
    };
    let (red, green, blue) = (
        nearest_level(rgb.0),
        nearest_level(rgb.1),
        nearest_level(rgb.2),
    );
    let cube_index = 16 + 36 * red + 6 * green + blue;
    let cube = (LEVELS[red], LEVELS[green], LEVELS[blue]);

    let (gray_index, gray) = (0..24_u8)
        .map(|step| {
            let level = 8 + 10 * step;
            (232 + step, (level, level, level))
        })
        .min_by_key(|(_, gray)| distance(*gray))
        .unwrap_or((232, (8, 8, 8)));
    if distance(gray) < distance(cube) {
        gray_index
    } else {
        u8::try_from(cube_index).unwrap_or(16)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use ratatui::style::{Color, Modifier};

    use super::{
        ColorMode, DARK, LIGHT, Palette, Rgb, Theme, ThemeName, detect_color_mode, nearest_256,
        parse_theme_name,
    };

    fn luminance(rgb: Rgb) -> f64 {
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgb.0) + 0.7152 * channel(rgb.1) + 0.0722 * channel(rgb.2)
    }

    fn contrast(a: Rgb, b: Rgb) -> f64 {
        let (high, low) = {
            let (a, b) = (luminance(a), luminance(b));
            if a > b { (a, b) } else { (b, a) }
        };
        (high + 0.05) / (low + 0.05)
    }

    #[test]
    fn palettes_meet_wcag_contrast() {
        for (name, palette) in [("dark", DARK), ("light", LIGHT)] {
            let Palette {
                background,
                surface,
                ..
            } = palette;
            for (role, color) in [
                ("text_primary", palette.text_primary),
                ("text_secondary", palette.text_secondary),
                ("accent", palette.accent),
                ("success", palette.success),
                ("warning", palette.warning),
                ("error", palette.error),
            ] {
                for (surface_name, under) in [("background", background), ("surface", surface)] {
                    let ratio = contrast(color, under);
                    assert!(ratio >= 4.5, "{name} {role} on {surface_name}: {ratio:.2}");
                }
            }
            let selection = contrast(palette.selection_text, palette.selection);
            assert!(selection >= 4.5, "{name} selection: {selection:.2}");
            let border = contrast(palette.focus_border, background);
            assert!(border >= 3.0, "{name} focus border: {border:.2}");
        }
    }

    #[test]
    fn color_mode_honours_no_color_and_colorterm() {
        let os = |value: &'static str| Some(OsStr::new(value));
        assert_eq!(
            detect_color_mode(os("1"), os("truecolor")),
            ColorMode::Monochrome
        );
        assert_eq!(
            detect_color_mode(os(""), os("truecolor")),
            ColorMode::TrueColor
        );
        assert_eq!(detect_color_mode(None, os("24BIT")), ColorMode::TrueColor);
        assert_eq!(
            detect_color_mode(None, os("256color")),
            ColorMode::Indexed256
        );
        assert_eq!(detect_color_mode(None, None), ColorMode::Indexed256);
    }

    #[test]
    fn theme_names_parse_case_insensitively() {
        assert_eq!(parse_theme_name(None), Ok(None));
        assert_eq!(
            parse_theme_name(Some(OsStr::new("Light"))),
            Ok(Some(ThemeName::Light))
        );
        assert_eq!(
            parse_theme_name(Some(OsStr::new("dark"))),
            Ok(Some(ThemeName::Dark))
        );
        assert_eq!(parse_theme_name(Some(OsStr::new("solarized"))), Err(()));
    }

    #[test]
    fn colors_fall_back_to_the_256_palette_and_monochrome() {
        assert_eq!(nearest_256((0, 0, 0)), 16);
        assert_eq!(nearest_256((255, 255, 255)), 231);
        assert_eq!(nearest_256((0x17, 0x19, 0x1F)), 234);
        assert_eq!(nearest_256((0x00, 0x5F, 0x6A)), 23);

        let indexed = Theme::new(ThemeName::Dark, ColorMode::Indexed256);
        assert_eq!(indexed.base().bg, Some(Color::Indexed(234)));
        let true_color = Theme::new(ThemeName::Light, ColorMode::TrueColor);
        assert_eq!(true_color.base().bg, Some(Color::Rgb(0xF5, 0xF7, 0xFA)));

        let monochrome = Theme::new(ThemeName::Dark, ColorMode::Monochrome);
        assert_eq!(monochrome.base().bg, None);
        assert!(
            monochrome
                .selection()
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert_eq!(monochrome.warning().fg, None);
    }

    #[test]
    fn toggling_switches_name_and_keeps_mode() {
        let theme = Theme::new(ThemeName::Dark, ColorMode::Indexed256);
        let toggled = theme.toggled();
        assert_eq!(toggled.name(), ThemeName::Light);
        assert_eq!(toggled, Theme::new(ThemeName::Light, ColorMode::Indexed256));
        assert_eq!(toggled.toggled(), theme);
    }
}
