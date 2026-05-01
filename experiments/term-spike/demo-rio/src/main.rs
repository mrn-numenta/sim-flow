//! Embedded-terminal demo using Rio's VT engine (`copa` parser +
//! `rio-backend` Crosswords state).

use rio_backend::ansi::CursorShape;
use rio_backend::config::colors::{AnsiColor, ColorRgb, NamedColor};
use rio_backend::crosswords::pos::{Column, Line, Pos};
use rio_backend::crosswords::{Crosswords, Mode};
use rio_backend::event::{EventListener, RioEvent, VoidListener, WindowId};
use rio_backend::performer::handler::Processor;
use term_core::{Cell, Rgb, Snapshot, TermBackend};

// Our own listener newtype keeps trait resolution unambiguous.
#[derive(Clone)]
struct Listener(VoidListener);

impl EventListener for Listener {
    fn event(&self) -> (Option<RioEvent>, bool) {
        self.0.event()
    }
}

struct RioBackend {
    term: Crosswords<Listener>,
    parser: Processor,
}

impl RioBackend {
    fn new(rows: u16, cols: u16) -> Self {
        let size =
            rio_backend::crosswords::CrosswordsSize::new(cols as usize, rows as usize);
        // SAFETY: WindowId::dummy() is unsound only when passed to real
        // winit APIs; rio-backend accepts it as an opaque tag.
        let window_id = unsafe { WindowId::dummy() };
        let term = Crosswords::new(
            size,
            CursorShape::Block,
            Listener(VoidListener),
            window_id,
            0,
            10_000,
        );
        Self {
            term,
            parser: Processor::new(),
        }
    }
}

impl TermBackend for RioBackend {
    fn name(&self) -> &'static str {
        "rio-backend"
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let size =
            rio_backend::crosswords::CrosswordsSize::new(cols as usize, rows as usize);
        self.term.resize(size);
    }

    fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn snapshot(&self) -> Snapshot {
        let rows = self.term.screen_lines();
        let cols = self.term.columns();
        let mut cells_out: Vec<Vec<Cell>> = Vec::with_capacity(rows);
        for row in 0..rows {
            let line = Line(row as i32);
            let mut row_cells: Vec<Cell> = Vec::with_capacity(cols);
            for col in 0..cols {
                let pos = Pos::new(line, Column(col));
                let square = &self.term.grid[pos];
                let style = self.term.grid.style_set.get(square.style_id());
                let ch = match square.c() {
                    '\0' => ' ',
                    other => other,
                };
                let fg = resolve_color(&style.fg, true);
                let bg = resolve_color(&style.bg, false);
                row_cells.push(Cell {
                    ch,
                    fg,
                    bg,
                    bold: false,
                    italic: false,
                    underline: false,
                });
            }
            cells_out.push(row_cells);
        }

        let cursor = self.term.cursor().pos;
        let cursor_visible = self.term.mode().contains(Mode::SHOW_CURSOR);
        Snapshot {
            rows: rows as u16,
            cols: cols as u16,
            cells: cells_out,
            cursor_row: cursor.row.0.max(0) as u16,
            cursor_col: cursor.col.0 as u16,
            cursor_visible,
        }
    }
}

fn resolve_color(color: &AnsiColor, is_fg: bool) -> Rgb {
    match color {
        AnsiColor::Spec(ColorRgb { r, g, b }) => Rgb(*r, *g, *b),
        AnsiColor::Named(name) => named_to_rgb(*name, is_fg),
        AnsiColor::Indexed(idx) => indexed_to_rgb(*idx, is_fg),
    }
}

fn named_to_rgb(name: NamedColor, is_fg: bool) -> Rgb {
    match name {
        NamedColor::Black => Rgb(0x10, 0x10, 0x10),
        NamedColor::Red => Rgb(0xcc, 0x33, 0x33),
        NamedColor::Green => Rgb(0x33, 0xcc, 0x66),
        NamedColor::Yellow => Rgb(0xcc, 0xb1, 0x33),
        NamedColor::Blue => Rgb(0x33, 0x66, 0xcc),
        NamedColor::Magenta => Rgb(0xcc, 0x33, 0xcc),
        NamedColor::Cyan => Rgb(0x33, 0xcc, 0xcc),
        NamedColor::White => Rgb(0xcc, 0xcc, 0xcc),
        NamedColor::LightBlack => Rgb(0x66, 0x66, 0x66),
        NamedColor::LightRed => Rgb(0xff, 0x55, 0x55),
        NamedColor::LightGreen => Rgb(0x55, 0xff, 0x88),
        NamedColor::LightYellow => Rgb(0xff, 0xd7, 0x55),
        NamedColor::LightBlue => Rgb(0x55, 0x99, 0xff),
        NamedColor::LightMagenta => Rgb(0xff, 0x55, 0xff),
        NamedColor::LightCyan => Rgb(0x55, 0xff, 0xff),
        NamedColor::LightWhite => Rgb(0xff, 0xff, 0xff),
        NamedColor::Foreground => {
            if is_fg {
                Rgb(0xcc, 0xcc, 0xcc)
            } else {
                Rgb(0x00, 0x00, 0x00)
            }
        }
        NamedColor::Background => {
            if is_fg {
                Rgb(0xcc, 0xcc, 0xcc)
            } else {
                Rgb(0x00, 0x00, 0x00)
            }
        }
        _ => {
            if is_fg {
                Rgb(0xcc, 0xcc, 0xcc)
            } else {
                Rgb(0x00, 0x00, 0x00)
            }
        }
    }
}

fn indexed_to_rgb(idx: u8, is_fg: bool) -> Rgb {
    if idx < 16 {
        let named = match idx {
            0 => NamedColor::Black,
            1 => NamedColor::Red,
            2 => NamedColor::Green,
            3 => NamedColor::Yellow,
            4 => NamedColor::Blue,
            5 => NamedColor::Magenta,
            6 => NamedColor::Cyan,
            7 => NamedColor::White,
            8 => NamedColor::LightBlack,
            9 => NamedColor::LightRed,
            10 => NamedColor::LightGreen,
            11 => NamedColor::LightYellow,
            12 => NamedColor::LightBlue,
            13 => NamedColor::LightMagenta,
            14 => NamedColor::LightCyan,
            15 => NamedColor::LightWhite,
            _ => NamedColor::White,
        };
        return named_to_rgb(named, is_fg);
    }
    if idx < 232 {
        let i = idx - 16;
        let r = (i / 36) % 6;
        let g = (i / 6) % 6;
        let b = i % 6;
        let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return Rgb(scale(r), scale(g), scale(b));
    }
    let level = 8 + (idx - 232) * 10;
    Rgb(level, level, level)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    term_core::run("term-spike: rio-backend", |rows, cols| {
        Box::new(RioBackend::new(rows, cols))
    })
}
