//! Embedded-terminal demo using the WezTerm VT engine (published as the
//! `tattoy-wezterm-term` fork because WezTerm's own crates are git-only).

use std::sync::Arc;

use tattoy_wezterm_cell as wezterm_cell;
use tattoy_wezterm_surface::CursorVisibility;
use tattoy_wezterm_term as wt;
use term_core::{Cell, Rgb, Snapshot, TermBackend};
use wt::color::{ColorAttribute, ColorPalette, SrgbaTuple};
use wt::config::TerminalConfiguration;
use wt::{Terminal, TerminalSize};

#[derive(Debug, Default)]
struct Config {
    palette: ColorPalette,
}

impl TerminalConfiguration for Config {
    fn color_palette(&self) -> ColorPalette {
        self.palette.clone()
    }
}

/// Sink writer: discards terminal answerback bytes. Good enough for the
/// spike; a richer impl would pipe these back to the PTY writer.
struct SinkWriter;

impl std::io::Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct WeztermBackend {
    term: Terminal,
    palette: ColorPalette,
}

impl WeztermBackend {
    fn new(rows: u16, cols: u16) -> Self {
        let size = TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        };
        let config = Arc::new(Config::default());
        let palette = config.color_palette();
        let term = Terminal::new(
            size,
            config,
            "sim-flow-term-spike",
            "0.1.0",
            Box::new(SinkWriter),
        );
        Self { term, palette }
    }
}

impl TermBackend for WeztermBackend {
    fn name(&self) -> &'static str {
        "tattoy-wezterm-term"
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let size = TerminalSize {
            rows: rows as usize,
            cols: cols as usize,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        };
        self.term.resize(size);
    }

    fn advance(&mut self, bytes: &[u8]) {
        self.term.advance_bytes(bytes);
    }

    fn snapshot(&self) -> Snapshot {
        let screen = self.term.screen();
        let cols = screen.physical_cols;
        let rows = screen.physical_rows;
        // Grab the visible rows without pulling scrollback: translate
        // visible row 0 to its physical index and take `rows` lines.
        let start = screen.phys_row(0);
        let lines = screen.lines_in_phys_range(start..start + rows);

        let mut cells_out: Vec<Vec<Cell>> = Vec::with_capacity(rows);
        for line in lines.iter().take(rows) {
            let mut row_cells: Vec<Cell> = Vec::with_capacity(cols);
            for cell in line.cells() {
                let s = cell.str();
                let ch = s.chars().next().unwrap_or(' ');
                let attrs = cell.attrs();
                let fg = resolve_color(&self.palette, attrs.foreground(), true);
                let bg = resolve_color(&self.palette, attrs.background(), false);
                row_cells.push(Cell {
                    ch,
                    fg,
                    bg,
                    bold: attrs.intensity() == wezterm_cell::Intensity::Bold,
                    italic: attrs.italic(),
                    underline: attrs.underline() != wezterm_cell::Underline::None,
                });
                if row_cells.len() >= cols {
                    break;
                }
            }
            while row_cells.len() < cols {
                row_cells.push(Cell::default());
            }
            cells_out.push(row_cells);
        }
        while cells_out.len() < rows {
            cells_out.push(vec![Cell::default(); cols]);
        }

        let cp = self.term.cursor_pos();
        Snapshot {
            rows: rows as u16,
            cols: cols as u16,
            cells: cells_out,
            cursor_row: cp.y.max(0) as u16,
            cursor_col: cp.x as u16,
            cursor_visible: matches!(cp.visibility, CursorVisibility::Visible),
        }
    }
}

fn resolve_color(palette: &ColorPalette, attr: ColorAttribute, is_fg: bool) -> Rgb {
    match attr {
        ColorAttribute::TrueColorWithPaletteFallback(srgb, _)
        | ColorAttribute::TrueColorWithDefaultFallback(srgb) => srgb_to_rgb(srgb),
        ColorAttribute::PaletteIndex(idx) => srgb_to_rgb(palette.colors.0[idx as usize]),
        ColorAttribute::Default => {
            if is_fg {
                srgb_to_rgb(palette.foreground)
            } else {
                srgb_to_rgb(palette.background)
            }
        }
    }
}

fn srgb_to_rgb(s: SrgbaTuple) -> Rgb {
    let clamp = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    Rgb(clamp(s.0), clamp(s.1), clamp(s.2))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    term_core::run("term-spike: tattoy-wezterm-term", |rows, cols| {
        Box::new(WeztermBackend::new(rows, cols))
    })
}
