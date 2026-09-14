//! Colored `note:` / `help:` / `warning:` prefixes for the CLI's
//! cargo-shaped report lines.
//!
//! Every such label the CLI prints must come from [`LabelStyle`]. A
//! hardcoded `"note: "` literal silently loses its color, which is exactly
//! how the watch notes went plain (issue #514) while the neighbouring
//! `error:` / `help:` block stayed styled.
//!
//! This lives in the library rather than next to the binary's `diagnostics`
//! module because warning sites exist on both sides of that split (browser
//! auto-open warnings are emitted from `browser.rs`, which the binary cannot
//! lend its private modules to).

use std::io::IsTerminal;

/// Whether colored output is wanted at all, independent of the stream: the
/// same gate the terminal diagnostic renderer uses, so a single `NO_COLOR`
/// setting governs every styled byte the CLI writes.
pub fn colors_enabled_by_env() -> bool {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    let dumb_term = std::env::var_os("TERM").is_some_and(|term| term == "dumb");
    !no_color && !dumb_term
}

/// Colored `label: ` prefixes, gated on the target stream being a TTY plus
/// [`colors_enabled_by_env`].
///
/// Sites that write to a `&mut dyn Write` cannot derive the style themselves
/// (a trait object carries no `IsTerminal`), so they take a `LabelStyle`
/// threaded down from wherever the concrete stream was created.
#[derive(Clone, Copy)]
pub struct LabelStyle {
    colors: bool,
}

impl LabelStyle {
    pub const PLAIN: Self = Self { colors: false };

    pub const COLORED: Self = Self { colors: true };

    pub fn for_stream(stream: &impl IsTerminal) -> Self {
        Self {
            colors: stream.is_terminal() && colors_enabled_by_env(),
        }
    }

    /// The style for the process's real stderr, for `eprintln!` sites that
    /// write there directly instead of through a passed-in writer.
    pub fn for_stderr() -> Self {
        Self::for_stream(&std::io::stderr())
    }

    pub fn warning(self) -> &'static str {
        self.pick("warning: ", "\x1b[1;33mwarning:\x1b[0m ")
    }

    pub fn note(self) -> &'static str {
        self.pick("note: ", "\x1b[1;32mnote:\x1b[0m ")
    }

    pub fn help(self) -> &'static str {
        self.pick("help: ", "\x1b[1;33mhelp:\x1b[0m ")
    }

    fn pick(self, plain: &'static str, styled: &'static str) -> &'static str {
        if self.colors {
            styled
        } else {
            plain
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_style_emits_bare_labels() {
        assert_eq!(LabelStyle::PLAIN.note(), "note: ");
        assert_eq!(LabelStyle::PLAIN.help(), "help: ");
        assert_eq!(LabelStyle::PLAIN.warning(), "warning: ");
    }

    #[test]
    fn colored_style_wraps_each_label_in_sgr() {
        assert_eq!(LabelStyle::COLORED.note(), "\x1b[1;32mnote:\x1b[0m ");
        assert_eq!(LabelStyle::COLORED.help(), "\x1b[1;33mhelp:\x1b[0m ");
        assert_eq!(LabelStyle::COLORED.warning(), "\x1b[1;33mwarning:\x1b[0m ");
    }

    #[test]
    fn a_non_terminal_stream_is_never_colored() {
        // A plain file handle is never a terminal, standing in for the piped
        // stdout and the in-memory writers the tests use.
        let file = std::fs::File::open("Cargo.toml").expect("crate manifest");
        assert_eq!(LabelStyle::for_stream(&file).note(), "note: ");
    }
}
