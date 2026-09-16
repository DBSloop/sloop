//! One accent colour, and the help styling built from it.
//!
//! `#D97757` is the only colour this tool decorates with. Everything else is weight,
//! dimming and space — a second hue would have to earn its place and none has.
//!
//! The types come from `clap::builder::styling`, which re-exports `anstyle`. Taking them
//! from clap rather than depending on `anstyle` directly means the styles here and the
//! styles clap renders can never be two incompatible versions of the same type.

use clap::builder::styling::{Ansi256Color, AnsiColor, Color, RgbColor, Style, Styles};

/// Claude Code orange, the single accent.
const ACCENT_RGB: RgbColor = RgbColor(0xD9, 0x77, 0x57);

/// xterm-256 index 173, `#D7875F` — the closest the 6×6×6 cube gets to the accent, and
/// close enough that the two are hard to tell apart side by side.
const ACCENT_256: Ansi256Color = Ansi256Color(173);

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

/// The accent, as fine as this terminal can render it.
fn accent_color() -> Color {
    if truecolor() {
        Color::Rgb(ACCENT_RGB)
    } else {
        Color::Ansi256(ACCENT_256)
    }
}

/// The accent as a style, for text.
#[must_use]
pub fn accent() -> Style {
    Style::new().fg_color(Some(accent_color()))
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

/// The `error:` prefix, red and bold, exactly as clap sets its own. Red here is meaning
/// rather than decoration, which is why it is the one colour allowed beside the accent.
#[must_use]
pub fn error_prefix() -> String {
    let style = Style::new()
        .fg_color(Some(Color::Ansi(AnsiColor::Red)))
        .bold();
    format!("{style}error:{style:#}")
}

/// A label beside a value: present, but never louder than what it labels.
#[must_use]
pub fn label(text: &str) -> String {
    dim(text)
}

/// Dim text, for the lines that support a heading rather than compete with it.
#[must_use]
pub fn dim(text: &str) -> String {
    let style = Style::new().dimmed();
    format!("{style}{text}{style:#}")
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
    use super::{ACCENT_256, ACCENT_RGB, accent, dim, error_prefix, heading, paint};

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

    #[test]
    fn the_accent_is_the_projects_one_colour() {
        assert_eq!(
            (ACCENT_RGB.0, ACCENT_RGB.1, ACCENT_RGB.2),
            (0xD9, 0x77, 0x57)
        );
        assert_eq!(ACCENT_256.0, 173);
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
