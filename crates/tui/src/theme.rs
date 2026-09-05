//! Semantic theme tokens (ADR-0006, spec ticket 01): a token layer whose
//! names and hex values mirror pi's `dark.json`, resolved to ratatui styles
//! at render time. Dark theme only; no runtime switching.
//!
//! Keeping pi's token names lets the palette stay comparable to pi and lets a
//! future theme file reuse the same vocabulary. The token → hex mapping is
//! exact against pi `dark.json` and is pinned by the tests below.

use ratatui::style::{Color, Modifier, Style};

/// Foreground semantic tokens (names mirror pi `dark.json` `colors`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Accent,
    Border,
    BorderAccent,
    BorderMuted,
    Success,
    Error,
    Warning,
    Muted,
    Dim,
    Text,
    ThinkingText,
    MdHeading,
    MdLink,
    MdLinkUrl,
    MdCode,
    MdCodeBlock,
    MdCodeBlockBorder,
    MdQuote,
    MdQuoteBorder,
    MdHr,
    MdListBullet,
    ToolTitle,
    ToolOutput,
    /// Fullscreen scrollbar/bg variants that only need fg styling.
    BashMode,
}

/// Background semantic tokens (pi `dark.json` `bg` + scrollbar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BgToken {
    SelectedBg,
    UserMessageBg,
    ToolPendingBg,
    ToolSuccessBg,
    ToolErrorBg,
}

impl Token {
    /// The exact pi `dark.json` hex color for this token.
    pub const fn color(self) -> Color {
        use Token::*;
        match self {
            Accent => Color::Rgb(0x8a, 0xbe, 0xb7),
            Border => Color::Rgb(0x5f, 0x87, 0xff),
            BorderAccent => Color::Rgb(0x00, 0xd7, 0xff),
            BorderMuted => Color::Rgb(0x50, 0x50, 0x50),
            Success => Color::Rgb(0xb5, 0xbd, 0x68),
            Error => Color::Rgb(0xcc, 0x66, 0x66),
            Warning => Color::Rgb(0xff, 0xff, 0x00),
            Muted => Color::Rgb(0x80, 0x80, 0x80),
            Dim => Color::Rgb(0x66, 0x66, 0x66),
            Text => Color::Rgb(0xd4, 0xd4, 0xd4),
            ThinkingText => Color::Rgb(0x80, 0x80, 0x80),
            MdHeading => Color::Rgb(0xf0, 0xc6, 0x74),
            MdLink => Color::Rgb(0x81, 0xa2, 0xbe),
            MdLinkUrl => Color::Rgb(0x66, 0x66, 0x66),
            MdCode => Color::Rgb(0x8a, 0xbe, 0xb7),
            MdCodeBlock => Color::Rgb(0xb5, 0xbd, 0x68),
            MdCodeBlockBorder => Color::Rgb(0x80, 0x80, 0x80),
            MdQuote => Color::Rgb(0x80, 0x80, 0x80),
            MdQuoteBorder => Color::Rgb(0x80, 0x80, 0x80),
            MdHr => Color::Rgb(0x80, 0x80, 0x80),
            MdListBullet => Color::Rgb(0x8a, 0xbe, 0xb7),
            ToolTitle => Color::Rgb(0xd4, 0xd4, 0xd4),
            ToolOutput => Color::Rgb(0x80, 0x80, 0x80),
            BashMode => Color::Rgb(0xb5, 0xbd, 0x68),
        }
    }
}

impl BgToken {
    /// The exact pi `dark.json` background hex for this token.
    pub const fn color(self) -> Color {
        use BgToken::*;
        match self {
            SelectedBg => Color::Rgb(0x3a, 0x3a, 0x4a),
            UserMessageBg => Color::Rgb(0x34, 0x35, 0x41),
            ToolPendingBg => Color::Rgb(0x28, 0x28, 0x32),
            ToolSuccessBg => Color::Rgb(0x28, 0x32, 0x28),
            ToolErrorBg => Color::Rgb(0x3c, 0x28, 0x28),
        }
    }
}

/// A foreground style for `token`.
pub fn fg(token: Token) -> Style {
    Style::default().fg(token.color())
}

/// A background style for `token`.
pub fn bg(token: BgToken) -> Style {
    Style::default().bg(token.color())
}

/// Style for a foreground token with `modifier` (e.g. BOLD, ITALIC, DIM).
pub fn fg_mod(token: Token, modifier: Modifier) -> Style {
    fg(token).add_modifier(modifier)
}

/// Style for a background token with `modifier`.
pub fn bg_mod(token: BgToken, modifier: Modifier) -> Style {
    bg(token).add_modifier(modifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every foreground token resolves to its exact pi `dark.json` hex.
    #[test]
    fn fg_tokens_match_pi_dark_json() {
        use Token::*;
        assert_eq!(Accent.color(), Color::Rgb(0x8a, 0xbe, 0xb7));
        assert_eq!(Border.color(), Color::Rgb(0x5f, 0x87, 0xff));
        assert_eq!(BorderAccent.color(), Color::Rgb(0x00, 0xd7, 0xff));
        assert_eq!(BorderMuted.color(), Color::Rgb(0x50, 0x50, 0x50));
        assert_eq!(Success.color(), Color::Rgb(0xb5, 0xbd, 0x68));
        assert_eq!(Error.color(), Color::Rgb(0xcc, 0x66, 0x66));
        assert_eq!(Warning.color(), Color::Rgb(0xff, 0xff, 0x00));
        assert_eq!(Muted.color(), Color::Rgb(0x80, 0x80, 0x80));
        assert_eq!(Dim.color(), Color::Rgb(0x66, 0x66, 0x66));
        assert_eq!(Text.color(), Color::Rgb(0xd4, 0xd4, 0xd4));
        assert_eq!(ThinkingText.color(), Color::Rgb(0x80, 0x80, 0x80));
        assert_eq!(MdHeading.color(), Color::Rgb(0xf0, 0xc6, 0x74));
        assert_eq!(MdLink.color(), Color::Rgb(0x81, 0xa2, 0xbe));
        assert_eq!(MdLinkUrl.color(), Color::Rgb(0x66, 0x66, 0x66));
        assert_eq!(MdCode.color(), Token::MdCode.color());
        assert_eq!(MdCodeBlock.color(), Color::Rgb(0xb5, 0xbd, 0x68));
        assert_eq!(MdCodeBlockBorder.color(), Color::Rgb(0x80, 0x80, 0x80));
        assert_eq!(MdQuote.color(), Color::Rgb(0x80, 0x80, 0x80));
        assert_eq!(MdQuoteBorder.color(), Color::Rgb(0x80, 0x80, 0x80));
        assert_eq!(MdHr.color(), Color::Rgb(0x80, 0x80, 0x80));
        assert_eq!(MdListBullet.color(), Color::Rgb(0x8a, 0xbe, 0xb7));
        assert_eq!(ToolTitle.color(), Color::Rgb(0xd4, 0xd4, 0xd4));
        assert_eq!(ToolOutput.color(), Color::Rgb(0x80, 0x80, 0x80));
        assert_eq!(BashMode.color(), Color::Rgb(0xb5, 0xbd, 0x68));
        // mdCode is defined as pi's accent, mdLinkUrl as pi's dimGray.
        assert_eq!(MdCode.color(), Accent.color());
        assert_eq!(MdLinkUrl.color(), Dim.color());
    }

    /// Every background token resolves to its exact pi `dark.json` hex.
    #[test]
    fn bg_tokens_match_pi_dark_json() {
        use BgToken::*;
        assert_eq!(SelectedBg.color(), Color::Rgb(0x3a, 0x3a, 0x4a));
        assert_eq!(UserMessageBg.color(), Color::Rgb(0x34, 0x35, 0x41));
        assert_eq!(ToolPendingBg.color(), Color::Rgb(0x28, 0x28, 0x32));
        assert_eq!(ToolSuccessBg.color(), Color::Rgb(0x28, 0x32, 0x28));
        assert_eq!(ToolErrorBg.color(), Color::Rgb(0x3c, 0x28, 0x28));
    }

    /// fg/bg produce styles carrying exactly the token color.
    #[test]
    fn fg_and_bg_produce_expected_styles() {
        assert_eq!(fg(Token::Accent).fg, Some(Token::Accent.color()));
        assert_eq!(
            bg(BgToken::ToolPendingBg).bg,
            Some(BgToken::ToolPendingBg.color())
        );
        // Unmodified styles keep defaults elsewhere.
        assert_eq!(fg(Token::Text).bg, None);
        assert_eq!(bg(BgToken::UserMessageBg).fg, None);
        assert_eq!(fg(Token::Text).add_modifier, Modifier::empty());
    }

    /// Modifier variants carry token color + modifier.
    #[test]
    fn mod_variants_apply_modifier() {
        let bold = fg_mod(Token::Accent, Modifier::BOLD);
        assert_eq!(bold.fg, Some(Token::Accent.color()));
        assert!(bold.add_modifier.contains(Modifier::BOLD));

        let dimbg = bg_mod(BgToken::SelectedBg, Modifier::DIM);
        assert_eq!(dimbg.bg, Some(BgToken::SelectedBg.color()));
        assert!(dimbg.add_modifier.contains(Modifier::DIM));
    }

    /// The palette is distinct enough that tokens do not silently alias
    /// (spot-check the tokens most likely to co-occur in one line).
    #[test]
    fn co_occurring_tokens_are_distinct() {
        assert_ne!(Token::Accent.color(), Token::Border.color());
        assert_ne!(Token::Accent.color(), Token::Success.color());
        assert_ne!(Token::Error.color(), Token::Success.color());
        assert_ne!(Token::Border.color(), Token::BorderAccent.color());
    }
}
