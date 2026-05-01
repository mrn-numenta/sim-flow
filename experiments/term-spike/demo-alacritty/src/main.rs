//! Embedded-terminal demo using `alacritty_terminal` as the VT engine.

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column as AColumn, Line as ALine, Point as APoint};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Processor, Rgb as ARgb};
use term_core::{Cell, Rgb, Snapshot, TermBackend};

#[derive(Clone, Default)]
struct NoopListener;

impl EventListener for NoopListener {
    fn send_event(&self, _event: Event) {}
}

#[derive(Debug, Clone, Copy)]
struct Size {
    rows: usize,
    cols: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

struct AlacrittyBackend {
    term: Term<NoopListener>,
    parser: Processor,
}

impl AlacrittyBackend {
    fn new(rows: u16, cols: u16) -> Self {
        let size = Size {
            rows: rows as usize,
            cols: cols as usize,
        };
        let config = TermConfig::default();
        let term = Term::new(config, &size, NoopListener);
        Self {
            term,
            parser: Processor::new(),
        }
    }
}

impl TermBackend for AlacrittyBackend {
    fn name(&self) -> &'static str {
        "alacritty_terminal"
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let size = Size {
            rows: rows as usize,
            cols: cols as usize,
        };
        self.term.resize(size);
    }

    fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn snapshot(&self) -> Snapshot {
        let grid = self.term.grid();
        let cols = grid.columns();
        let screen = grid.screen_lines();

        let mut cells = Vec::with_capacity(screen);
        for row in 0..screen {
            let line = ALine(row as i32);
            let mut row_cells = Vec::with_capacity(cols);
            for col in 0..cols {
                let point = APoint::new(line, AColumn(col));
                let cell = &grid[point];
                let ch = if cell.c == '\0' { ' ' } else { cell.c };
                let fg = resolve_color(&cell.fg, true);
                let bg = resolve_color(&cell.bg, false);
                row_cells.push(Cell {
                    ch,
                    fg,
                    bg,
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.contains(Flags::UNDERLINE),
                });
            }
            cells.push(row_cells);
        }

        let cursor = self.term.grid().cursor.point;
        let cursor_row = cursor.line.0.max(0) as u16;
        Snapshot {
            rows: screen as u16,
            cols: cols as u16,
            cells,
            cursor_row,
            cursor_col: cursor.column.0 as u16,
            cursor_visible: self.term.mode().contains(TermMode::SHOW_CURSOR),
        }
    }
}

fn resolve_color(c: &AColor, is_fg: bool) -> Rgb {
    match c {
        AColor::Spec(ARgb { r, g, b }) => Rgb(*r, *g, *b),
        AColor::Named(name) => named_to_rgb(*name, is_fg),
        AColor::Indexed(idx) => indexed_to_rgb(*idx, is_fg),
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
        NamedColor::BrightBlack => Rgb(0x66, 0x66, 0x66),
        NamedColor::BrightRed => Rgb(0xff, 0x55, 0x55),
        NamedColor::BrightGreen => Rgb(0x55, 0xff, 0x88),
        NamedColor::BrightYellow => Rgb(0xff, 0xd7, 0x55),
        NamedColor::BrightBlue => Rgb(0x55, 0x99, 0xff),
        NamedColor::BrightMagenta => Rgb(0xff, 0x55, 0xff),
        NamedColor::BrightCyan => Rgb(0x55, 0xff, 0xff),
        NamedColor::BrightWhite => Rgb(0xff, 0xff, 0xff),
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
            8 => NamedColor::BrightBlack,
            9 => NamedColor::BrightRed,
            10 => NamedColor::BrightGreen,
            11 => NamedColor::BrightYellow,
            12 => NamedColor::BrightBlue,
            13 => NamedColor::BrightMagenta,
            14 => NamedColor::BrightCyan,
            15 => NamedColor::BrightWhite,
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
    term_core::run("term-spike: alacritty_terminal", |rows, cols| {
        Box::new(AlacrittyBackend::new(rows, cols))
    })
}
