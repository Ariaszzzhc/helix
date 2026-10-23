// Welcome screen adapted from helix-editor/helix#15173 (nik-rev and NSPC911).
use helix_loader::VERSION_AND_GIT_HASH;
use helix_view::{
    graphics::{Modifier, Rect},
    Theme,
};
use tui::{
    buffer::Buffer as Surface,
    text::{Span, Spans},
};

pub(super) fn render(theme: &Theme, area: Rect, surface: &mut Surface) {
    // Work entirely within the document's visible text area, including in splits.
    let area = area.intersection(surface.area);
    const LOGO_STR: &str = "\
**             
*****        ::
 ******** :::::
     **::::::: 
   ::::::::***=
:::::::    ====
::::    =======
:---========   
 =======--     
===== -------- 
==        -----
             --";

    let empty_line = || (Spans::from(""), false);

    let raw_help_lines: [(Spans, bool); 12] = [
        (
            vec![
                Span::raw("helix "),
                Span::styled(VERSION_AND_GIT_HASH, theme.get("comment")),
            ]
            .into(),
            true,
        ),
        empty_line(),
        (
            Span::styled(
                "A post-modern modal text editor",
                theme.get("ui.text").add_modifier(Modifier::ITALIC),
            )
            .into(),
            true,
        ),
        empty_line(),
        (
            vec![
                Span::styled(":tutor", theme.get("markup.raw")),
                Span::styled("<enter>", theme.get("comment")),
                Span::raw("       learn helix"),
            ]
            .into(),
            false,
        ),
        (
            vec![
                Span::styled(":theme", theme.get("markup.raw")),
                Span::styled("<space><tab>", theme.get("comment")),
                Span::raw("  choose a theme"),
            ]
            .into(),
            false,
        ),
        (
            vec![
                Span::styled("<space>e", theme.get("markup.raw")),
                Span::raw("            file explorer"),
            ]
            .into(),
            false,
        ),
        (
            vec![
                Span::styled("<space>?", theme.get("markup.raw")),
                Span::raw("            see all commands"),
            ]
            .into(),
            false,
        ),
        (
            vec![
                Span::styled(":quit", theme.get("markup.raw")),
                Span::styled("<enter>", theme.get("comment")),
                Span::raw("        quit helix"),
            ]
            .into(),
            false,
        ),
        empty_line(),
        (
            vec![
                Span::styled("docs: ", theme.get("ui.text")),
                Span::styled("docs.helix-editor.com", theme.get("markup.link.url")),
            ]
            .into(),
            true,
        ),
        empty_line(),
    ];

    let help_width = raw_help_lines
        .iter()
        .map(|(line, _)| line.width())
        .max()
        .unwrap_or(0);
    let logo_width = LOGO_STR.lines().map(str::len).max().unwrap_or(0);
    let gap = 6;
    let full_height = raw_help_lines.len();
    let text_style = theme.get("ui.text");

    let lines: Vec<(Spans, bool)> =
        if help_width <= usize::from(area.width) && full_height <= usize::from(area.height) {
            raw_help_lines.into()
        } else {
            // Keep the essential first-run actions visible in short or narrow panes.
            ["Helix", ":tutor  learn helix", ":quit   quit helix"]
                .into_iter()
                .filter(|line| line.len() <= usize::from(area.width))
                .take(usize::from(area.height))
                .map(|line| (Spans::from(Span::styled(line, text_style)), true))
                .collect()
        };
    if lines.is_empty() {
        return;
    }

    let help_width = lines
        .iter()
        .map(|(line, _)| line.width())
        .max()
        .unwrap_or(0);
    let show_logo =
        lines.len() == full_height && logo_width + gap + help_width <= usize::from(area.width);
    let width = help_width + if show_logo { logo_width + gap } else { 0 };
    let x = area.x + (area.width - width as u16) / 2;
    let y = area.y + (area.height - lines.len() as u16) / 2;
    let help_x = x + if show_logo {
        (logo_width + gap) as u16
    } else {
        0
    };

    // Existing syntax scopes supply colors suited to both light and dark themes.
    // Patch only the foreground so syntax backgrounds don't create colored blocks.
    let logo_styles = ["keyword", "function", "type", "constant"].map(|scope| {
        let mut style = text_style;
        style.fg = theme.get(scope).fg.or(text_style.fg);
        style
    });
    let logo: Vec<Spans> = LOGO_STR
        .lines()
        .map(|line| {
            line.chars()
                .map(|ch| {
                    let style = match ch {
                        '*' => logo_styles[0],
                        ':' => logo_styles[1],
                        '=' => logo_styles[2],
                        '-' => logo_styles[3],
                        _ => text_style,
                    };
                    Span::styled(ch.to_string(), style)
                })
                .collect()
        })
        .collect();

    for (row, (line, centered)) in lines.iter().enumerate() {
        let line_x = help_x
            + if *centered {
                (help_width - line.width()) as u16 / 2
            } else {
                0
            };
        surface.set_spans(line_x, y + row as u16, line, line.width() as u16);
        if show_logo {
            surface.set_spans(x, y + row as u16, &logo[row], logo_width as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::theme::{Loader, DEFAULT_THEME};

    #[test]
    fn welcome_stays_inside_offset_panes_at_all_sizes() {
        for width in 0..100 {
            for height in 0..20 {
                let area = Rect::new(9, 4, width, height);
                let mut surface = Surface::empty(Rect::new(3, 2, 110, 25));
                for cell in &mut surface.content {
                    cell.set_symbol("!");
                }
                render(&DEFAULT_THEME, area, &mut surface);
                for y in surface.area.top()..surface.area.bottom() {
                    for x in surface.area.left()..surface.area.right() {
                        if x < area.left()
                            || x >= area.right()
                            || y < area.top()
                            || y >= area.bottom()
                        {
                            assert_eq!(
                                surface[(x, y)].symbol.as_str(),
                                "!",
                                "{area:?} at ({x}, {y})"
                            );
                        }
                    }
                }
            }
        }
        // The surface may itself be smaller than the requested pane.
        let mut surface = Surface::empty(Rect::new(5, 5, 20, 4));
        render(&DEFAULT_THEME, Rect::new(0, 0, 100, 50), &mut surface);
    }

    #[test]
    fn welcome_adapts_content_to_available_space() {
        for (width, height, logo, tutor) in [
            (100, 20, true, true),
            (40, 12, false, true),
            (25, 5, false, true),
            (5, 1, false, false),
            (0, 0, false, false),
        ] {
            let mut surface = Surface::empty(Rect::new(0, 0, width, height));
            render(&DEFAULT_THEME, surface.area, &mut surface);
            let text: String = surface
                .content
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect();
            assert_eq!(text.contains("*****"), logo, "{width}x{height}");
            assert_eq!(text.contains(":tutor"), tutor, "{width}x{height}");
            if width == 5 {
                assert_eq!(text, "Helix");
            }
        }
    }

    #[test]
    fn logo_tracks_light_dark_and_reloaded_theme_colors() {
        let loader =
            Loader::new(&[std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../runtime")]);
        let mut surface = Surface::empty(Rect::new(0, 0, 100, 20));
        for name in [
            "default",
            "solarized_light",
            "tokyonight_day",
            "base16_default",
        ] {
            let theme = loader.load(name).unwrap();
            render(&theme, surface.area, &mut surface);
            let logo_x = surface
                .content
                .iter()
                .position(|cell| cell.symbol.as_str() == "*")
                .unwrap()
                % usize::from(surface.area.width);
            for (symbol, scope) in [
                ("*", "keyword"),
                (":", "function"),
                ("=", "type"),
                ("-", "constant"),
            ] {
                let cell = surface
                    .content
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| index % usize::from(surface.area.width) < logo_x + 14)
                    .map(|(_, cell)| cell)
                    .find(|cell| cell.symbol.as_str() == symbol)
                    .unwrap();
                assert_eq!(
                    Some(cell.fg),
                    theme.get(scope).fg.or(theme.get("ui.text").fg),
                    "{name}: {scope}"
                );
            }
        }
    }
}
