use helix_core::Position;

use helix_view::theme::Style;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

pub struct InlineBlame {
    lines: Vec<(usize, String)>,
    style: Style,
}

impl InlineBlame {
    pub fn new(style: Style, lines: Vec<(usize, String)>) -> Self {
        InlineBlame { style, lines }
    }
}

impl Decoration for InlineBlame {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        // Only render inline blame on the last visual line
        // This prevents it being rendered multiple times per line, if the
        // line is wrapping.
        //
        // We want to render it on the last visual line because unless the user
        // has `soft-wrap.wrap-at-text-width` enabled, for a document line that wraps
        // there will only be space to render inline blame at the last doc line
        if !pos.is_last_visual_line {
            return Position::new(0, 0);
        }

        let Ok(index) = self
            .lines
            .binary_search_by_key(&pos.doc_line, |(line, _)| *line)
        else {
            return Position::new(0, 0);
        };
        let blame = &self.lines[index].1;
        let start = virt_off.col.saturating_add(6);
        if !renderer.column_in_bounds(start, 1) {
            return Position::new(0, 0);
        }
        let Some(column) = start.checked_sub(renderer.offset.col) else {
            return Position::new(0, 0);
        };
        let column = column as u16;
        let x = renderer.viewport.x + column;
        let (end, _) = renderer.set_string_truncated(
            x,
            pos.visual_line,
            blame,
            renderer.viewport.width.saturating_sub(column) as usize,
            |_| self.style,
            true,
            false,
        );
        Position::new(0, 6 + end.saturating_sub(x) as usize)
    }
}
