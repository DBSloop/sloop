//! The palette, and the help styling built from it.
//!
//! **Six colours, and they are the website's.** `web/src/styles/tokens.css` defines what a
//! terminal block looks like on the landing page and says in a comment that *"the orange
//! inside it is the orange the real CLI prints"*. That sentence is only true if the two
//! lists are the same list, so this is that list — `--ch-brand` and the four `--ch-term-*`
//! values, transcribed, with a 256-colour approximation beside each for terminals that
//! cannot take twenty-four bits.
//!
//! **Colour means something here; it is not decoration.** Orange is the brand and the thing
//! you are about to do, green is a thing that worked, amber is a thing to be careful about,
//! red is a thing that did not work, grey is a label rather than the value it labels. A
//! seventh hue would need a seventh meaning.
//!
//! *This replaces the single-accent rule in `CLAUDE.md`, at the owner's instruction — see
//! "Colour, and the version under the wordmark" in `docs/OWNER-DECISIONS.md`.*
//!
//! The types come from `clap::builder::styling`, which re-exports `anstyle`. Taking them
//! from clap rather than depending on `anstyle` directly means the styles here and the
//! styles clap renders can never be two incompatible versions of the same type.

use clap::builder::styling::{Ansi256Color, Color, RgbColor, Style, Styles};

/// One of the six, by what it is for rather than by what it looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hue {
    /// Claude Code orange. The mark, the highlight, and the command to type.
    Brand,
    /// Ordinary content.
    Text,
    /// A label, a help line, the phrase beside a menu item.
    Dim,
    /// It worked.
    Ok,
    /// It worked, and there is something to know about it.
    Warn,
    /// It did not work.
    Bad,
}

impl Hue {
    /// The exact colour, the same twenty-four bits the website uses.
    const fn rgb(self) -> RgbColor {
        match self {
            // --ch-brand
            Self::Brand => RgbColor(0xD9, 0x77, 0x57),
            // --ch-term-text
            Self::Text => RgbColor(0xE0, 0xDC, 0xD6),
            // --ch-term-dim
            Self::Dim => RgbColor(0x8C, 0x84, 0x7C),
            // --ch-term-ok
            Self::Ok => RgbColor(0x6F, 0xBF, 0x8A),
            // --ch-term-warn
            Self::Warn => RgbColor(0xE0, 0xB2, 0x56),
            // --ch-term-bad
            Self::Bad => RgbColor(0xE8, 0x70, 0x6A),
        }
    }

    /// The nearest colour in the 256-colour cube. Each was picked by rounding every
    /// channel to the cube's own steps, so none of them is a guess.
    const fn cube(self) -> Ansi256Color {
        match self {
            Self::Brand => Ansi256Color(173), // #D7875F
            Self::Text => Ansi256Color(253),  // #DADADA
            Self::Dim => Ansi256Color(245),   // #8A8A8A
            Self::Ok => Ansi256Color(72),     // #5FAF87
            Self::Warn => Ansi256Color(179),  // #D7AF5F
            Self::Bad => Ansi256Color(167),   // #D75F5F
        }
    }
}

/// True when the terminal has advertised 24-bit colour.
///
/// `COLORTERM` is the de-facto signal: Windows Terminal, iTerm2, kitty, Alacritty,
/// WezTerm and modern VTE set it, and a plain `xterm-256color` does not. Anything that
/// stays quiet gets the 256-colour approximation, which is the safe way round — an
/// approximated orange is a small loss, a terminal printing raw escape bytes is not.
fn truecolor() -> bool {
    matches!(
        std::env::var("COLORTERM").as_deref(),
        Ok("truecolor" | "24bit")
    )
}

/// One colour, as fine as this terminal can render it.
fn colour(hue: Hue) -> Color {
    match ink(hue) {
        Ink::True(r, g, b) => Color::Rgb(RgbColor(r, g, b)),
        Ink::Cube(index) => Color::Ansi256(Ansi256Color(index)),
    }
}

/// A colour as plain numbers, for a renderer that is not `anstyle`'s.
///
/// The interactive shell hands its colours to `inquire`, which has its own colour type, and
/// a second `COLORTERM` check over there would be a second answer to the same question. So
/// the decision is made once, here, and both renderers ask for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    /// 24-bit, the colour exactly.
    True(u8, u8, u8),
    /// The nearest colour in the 256-colour cube.
    Cube(u8),
}

/// Which of the two this terminal gets, for `hue`.
#[must_use]
pub fn ink(hue: Hue) -> Ink {
    if truecolor() {
        let RgbColor(r, g, b) = hue.rgb();
        Ink::True(r, g, b)
    } else {
        Ink::Cube(hue.cube().0)
    }
}

/// The accent as a style, for text.
#[must_use]
pub fn accent() -> Style {
    Style::new().fg_color(Some(colour(Hue::Brand)))
}

/// Any of the six as a style.
#[must_use]
pub fn styled(hue: Hue) -> Style {
    Style::new().fg_color(Some(colour(hue)))
}

/// Wrap `text` in `hue` and close it again.
#[must_use]
pub fn in_hue(hue: Hue, text: &str) -> String {
    let style = styled(hue);
    format!("{style}{text}{style:#}")
}

/// Wrap `text` in the accent and close it again.
///
/// The escapes are written into the string rather than applied to the stream, because
/// clap renders help text verbatim. That is safe: clap writes help through `anstream`,
/// which strips every escape in the buffer — these included — when the destination is a
/// pipe, a file, or a terminal the user has told to stay monochrome with `NO_COLOR`.
#[must_use]
pub fn heading(text: &str) -> String {
    let style = accent().bold();
    format!("{style}{text}{style:#}")
}

/// Text in the accent, with no weight added — the wordmark, and values worth noticing.
#[must_use]
pub fn paint(text: &str) -> String {
    let style = accent();
    format!("{style}{text}{style:#}")
}

/// The `error:` prefix.
#[must_use]
pub fn error_prefix() -> String {
    let style = styled(Hue::Bad).bold();
    format!("{style}error:{style:#}")
}

/// A label beside a value: present, but never louder than what it labels.
#[must_use]
pub fn label(text: &str) -> String {
    dim(text)
}

/// Quieter than the text around it — labels, help, the phrase beside a menu item.
///
/// **A colour rather than SGR 2.** The dim attribute is the least consistently implemented
/// thing in the whole escape vocabulary: some terminals ignore it, some render it as a
/// different font weight, and Windows' own console did nothing with it at all. A grey that
/// was chosen against the background is grey everywhere.
#[must_use]
pub fn dim(text: &str) -> String {
    in_hue(Hue::Dim, text)
}

/// How clap paints `--help` and its errors.
///
/// Headings and usage carry the accent. Commands and flags are set in bold rather than a
/// second colour, so the eye still has exactly one thing to follow. Errors stay red,
/// because red there is meaning and not decoration.
#[must_use]
pub fn clap_styles() -> Styles {
    Styles::styled()
        .header(accent().bold())
        .usage(accent().bold())
        .literal(Style::new().bold())
        .placeholder(Style::new().dimmed())
        .valid(accent())
}

#[cfg(test)]
mod tests {
    use super::{Hue, accent, dim, error_prefix, heading, paint};

    /// Strip every SGR sequence, the way `anstream` does when the destination is a pipe
    /// or the user has said `NO_COLOR`. What is left has to be the text we asked for —
    /// if it is not, redirecting `--help` into a file produces something unreadable.
    fn without_escapes(painted: &str) -> String {
        let mut plain = String::new();
        let mut rest = painted;
        while let Some(start) = rest.find('\x1b') {
            plain.push_str(&rest[..start]);
            let after = &rest[start..];
            let end = after.find('m').expect("an SGR sequence ends in `m`");
            rest = &after[end + 1..];
        }
        plain.push_str(rest);
        plain
    }

    /// **The palette is the website's, and this is the thing that keeps it so.**
    /// `web/src/styles/tokens.css` tells every reader that the orange in a terminal block
    /// on the landing page is the orange the real CLI prints. Change either list without
    /// the other and that sentence becomes marketing.
    #[test]
    fn the_palette_is_the_websites_palette() {
        for (hue, rgb) in [
            // --ch-brand
            (Hue::Brand, (0xD9, 0x77, 0x57)),
            // --ch-term-text
            (Hue::Text, (0xE0, 0xDC, 0xD6)),
            // --ch-term-dim
            (Hue::Dim, (0x8C, 0x84, 0x7C)),
            // --ch-term-ok
            (Hue::Ok, (0x6F, 0xBF, 0x8A)),
            // --ch-term-warn
            (Hue::Warn, (0xE0, 0xB2, 0x56)),
            // --ch-term-bad
            (Hue::Bad, (0xE8, 0x70, 0x6A)),
        ] {
            let super::RgbColor(r, g, b) = hue.rgb();
            assert_eq!((r, g, b), rgb, "{hue:?} drifted from tokens.css");
        }
    }

    /// Six meanings, six colours, and no two of them the same — a palette with a
    /// duplicate in it is a palette with a meaning nobody can see.
    #[test]
    fn no_two_of_the_six_are_the_same_colour() {
        let hues = [
            Hue::Brand,
            Hue::Text,
            Hue::Dim,
            Hue::Ok,
            Hue::Warn,
            Hue::Bad,
        ];
        for (index, hue) in hues.iter().enumerate() {
            for other in &hues[index + 1..] {
                assert_ne!(
                    hue.rgb(),
                    other.rgb(),
                    "{hue:?} and {other:?} are one colour"
                );
                assert_ne!(
                    hue.cube().0,
                    other.cube().0,
                    "{hue:?} and {other:?} share an index"
                );
            }
        }
    }

    #[test]
    fn styled_text_is_the_plain_text_plus_escapes() {
        for (painted, text) in [
            (heading("Commands"), "Commands"),
            (dim("optional"), "optional"),
            (paint("sloop"), "sloop"),
            (error_prefix(), "error:"),
        ] {
            assert!(painted.starts_with('\x1b'), "{painted:?} never opened");
            assert!(painted.ends_with("\x1b[0m"), "{painted:?} never reset");
            assert_eq!(without_escapes(&painted), text);
        }
    }

    #[test]
    fn the_accent_is_a_foreground_colour_and_nothing_else() {
        let style = accent();
        assert!(style.get_fg_color().is_some());
        assert!(style.get_bg_color().is_none());
    }
}

/// A name as it has to be typed back at a shell.
///
/// **Every message that suggests a command has to survive being pasted into one.** A label
/// may hold spaces — `sloop db create 'Test Sloop DB 2'` is a perfectly good registration —
/// and a message that then says *"`sloop db remove Test Sloop DB 2` forgets it"* is a message
/// that produces four arguments and an error when somebody does exactly what it told them to.
///
/// **Single quotes, because all four shells this runs under read them the same way.**
/// PowerShell, `cmd` through a shim, `bash` and `zsh` all take `'…'` as a literal run with no
/// interpolation. A name holding an apostrophe goes in double quotes instead — none of the
/// characters that are special inside `"…"` can be in a name that got past
/// [`crate::registry::file::check_name`] except `$` and a backtick, so the rare name carrying
/// one of those alongside an apostrophe is the one case this cannot render for every shell at
/// once, and it takes PowerShell's spelling.
///
/// Anything made only of the characters a shell never touches is returned untouched, so the
/// ordinary message reads exactly as it did.
#[must_use]
pub fn as_argument(name: &str) -> String {
    let plain = |letter: char| letter.is_ascii_alphanumeric() || matches!(letter, '.' | '_' | '-');

    if !name.is_empty() && name.chars().all(plain) {
        return name.to_owned();
    }
    if !name.contains('\'') {
        return format!("'{name}'");
    }
    if !name.contains('"') {
        return format!("\"{name}\"");
    }
    format!("'{}'", name.replace('\'', "''"))
}

#[cfg(test)]
mod argument_tests {
    use super::as_argument;

    /// An ordinary name is left exactly as it was — the common message must not change.
    #[test]
    fn a_name_a_shell_would_not_touch_is_untouched() {
        for plain in ["orders", "orders_live", "app-1", "db.two", "A1"] {
            assert_eq!(as_argument(plain), plain);
        }
    }

    /// **The bug this exists for.** A label with spaces has to come back quotable.
    #[test]
    fn a_name_with_spaces_comes_back_ready_to_paste() {
        assert_eq!(as_argument("Test Sloop DB 2"), "'Test Sloop DB 2'");
        assert_eq!(as_argument(" leading"), "' leading'");
        assert_eq!(as_argument(""), "''");
    }

    /// Anything a shell reads as syntax is quoted too, not just a space.
    #[test]
    fn anything_a_shell_would_read_is_quoted() {
        for awkward in ["a;b", "a|b", "a&b", "a$b", "a>b", "a*b", "a(b)", "a#b"] {
            assert_eq!(as_argument(awkward), format!("'{awkward}'"));
        }
    }

    /// An apostrophe cannot sit inside single quotes, so that name takes double ones.
    #[test]
    fn an_apostrophe_moves_it_to_double_quotes() {
        assert_eq!(as_argument("Ada's db"), "\"Ada's db\"");
        // Both kinds at once is the one case no single spelling suits; it takes
        // PowerShell's, which is doubling the apostrophe.
        assert_eq!(as_argument("Ada's \"db\""), "'Ada''s \"db\"'");
    }
}
