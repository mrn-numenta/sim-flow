//! Shared egui window + PTY plumbing for the terminal-emulator spike.
//!
//! Each demo crate implements [`TermBackend`] for its chosen emulator
//! (alacritty_terminal, wezterm-term, or rio's copa+rio-backend) and
//! hands the backend to [`run`], which wires up the eframe window,
//! spawns the child process on a PTY, pumps bytes from the PTY into
//! the backend, and renders [`Snapshot`]s into the UI.

use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;

use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

// -----------------------------------------------------------------------
// Cell grid abstraction
// -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const BLACK: Self = Rgb(0, 0, 0);
    pub const WHITE: Self = Rgb(0xcc, 0xcc, 0xcc);

    pub fn as_color32(self) -> egui::Color32 {
        egui::Color32::from_rgb(self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Rgb::WHITE,
            bg: Rgb::BLACK,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<Vec<Cell>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
}

// -----------------------------------------------------------------------
// Backend trait
// -----------------------------------------------------------------------

pub trait TermBackend: Send + 'static {
    fn name(&self) -> &'static str;

    /// Called once at startup with the initial grid size.
    fn resize(&mut self, rows: u16, cols: u16);

    /// Called whenever bytes arrive from the PTY.
    fn advance(&mut self, bytes: &[u8]);

    /// Snapshot the current visible grid for rendering.
    fn snapshot(&self) -> Snapshot;
}

// -----------------------------------------------------------------------
// PTY plumbing
// -----------------------------------------------------------------------

struct PtyIo {
    writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
    bytes_rx: Receiver<Vec<u8>>,
}

fn spawn_child(
    cmd: &[String],
    rows: u16,
    cols: u16,
) -> Result<PtyIo, Box<dyn std::error::Error>> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut builder = if cmd.is_empty() {
        let shell =
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        CommandBuilder::new(shell)
    } else {
        let mut b = CommandBuilder::new(&cmd[0]);
        for arg in &cmd[1..] {
            b.arg(arg);
        }
        b
    };
    // A sensible $TERM so ncurses-style apps behave.
    builder.env("TERM", "xterm-256color");
    if let Ok(cwd) = std::env::current_dir() {
        builder.cwd(cwd);
    }

    let child = pair.slave.spawn_command(builder)?;
    let writer = pair.master.take_writer()?;
    let reader = pair.master.try_clone_reader()?;

    let (bytes_tx, bytes_rx) = unbounded::<Vec<u8>>();
    thread::Builder::new()
        .name("pty-reader".into())
        .spawn(move || reader_loop(reader, bytes_tx))?;

    Ok(PtyIo {
        writer,
        master: pair.master,
        _child: child,
        bytes_rx,
    })
}

fn reader_loop(mut reader: Box<dyn Read + Send>, tx: Sender<Vec<u8>>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(err) => {
                log::warn!("pty read error: {err}");
                break;
            }
        }
    }
    log::info!("pty reader exiting");
}

// -----------------------------------------------------------------------
// Keyboard → PTY byte translation
// -----------------------------------------------------------------------

fn translate_key(event: &egui::Event) -> Option<Vec<u8>> {
    match event {
        egui::Event::Text(s) => Some(s.as_bytes().to_vec()),
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => Some(key_bytes(*key, modifiers)),
        egui::Event::Paste(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

fn key_bytes(key: egui::Key, m: &egui::Modifiers) -> Vec<u8> {
    use egui::Key;
    if m.ctrl {
        // Ctrl-<letter> → control byte.
        if let Some(b) = ctrl_byte(key) {
            return vec![b];
        }
    }
    match key {
        Key::Enter => b"\r".to_vec(),
        Key::Tab => {
            if m.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        Key::Backspace => b"\x7f".to_vec(),
        Key::Escape => b"\x1b".to_vec(),
        Key::ArrowUp => csi(m, b'A'),
        Key::ArrowDown => csi(m, b'B'),
        Key::ArrowRight => csi(m, b'C'),
        Key::ArrowLeft => csi(m, b'D'),
        Key::Home => csi(m, b'H'),
        Key::End => csi(m, b'F'),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::Space => b" ".to_vec(),
        _ => Vec::new(),
    }
}

fn ctrl_byte(key: egui::Key) -> Option<u8> {
    use egui::Key;
    let c = match key {
        Key::A => b'a',
        Key::B => b'b',
        Key::C => b'c',
        Key::D => b'd',
        Key::E => b'e',
        Key::F => b'f',
        Key::G => b'g',
        Key::H => b'h',
        Key::I => b'i',
        Key::J => b'j',
        Key::K => b'k',
        Key::L => b'l',
        Key::M => b'm',
        Key::N => b'n',
        Key::O => b'o',
        Key::P => b'p',
        Key::Q => b'q',
        Key::R => b'r',
        Key::S => b's',
        Key::T => b't',
        Key::U => b'u',
        Key::V => b'v',
        Key::W => b'w',
        Key::X => b'x',
        Key::Y => b'y',
        Key::Z => b'z',
        _ => return None,
    };
    // Ctrl-<letter> = letter & 0x1f.
    Some(c & 0x1f)
}

fn csi(_m: &egui::Modifiers, final_byte: u8) -> Vec<u8> {
    // Minimal CSI; modifiers could be encoded, but Claude and most
    // AI CLIs cope with plain sequences.
    vec![b'\x1b', b'[', final_byte]
}

// -----------------------------------------------------------------------
// egui app
// -----------------------------------------------------------------------

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 36;

struct App {
    backend: Arc<Mutex<Box<dyn TermBackend>>>,
    backend_name: String,
    pty: PtyIo,
    font_size: f32,
    cols: u16,
    rows: u16,
    last_text_size: Option<egui::Vec2>,
    cell_w: f32,
    cell_h: f32,
}

impl App {
    fn new(
        backend: Box<dyn TermBackend>,
        pty: PtyIo,
        rows: u16,
        cols: u16,
    ) -> Self {
        let backend_name = backend.name().to_string();
        Self {
            backend: Arc::new(Mutex::new(backend)),
            backend_name,
            pty,
            font_size: 14.0,
            cols,
            rows,
            last_text_size: None,
            cell_w: 0.0,
            cell_h: 0.0,
        }
    }

    fn pump_pty(&mut self) {
        // Drain everything available this frame.
        while let Ok(bytes) = self.pty.bytes_rx.try_recv() {
            self.backend.lock().advance(&bytes);
        }
    }

    fn write_to_pty(&mut self, bytes: &[u8]) {
        if let Err(err) = self.pty.writer.write_all(bytes) {
            log::warn!("pty write failed: {err}");
        }
        let _ = self.pty.writer.flush();
    }

    fn maybe_resize(&mut self, ui: &egui::Ui) {
        // Recompute rows/cols from the available rect and cell size.
        if self.cell_w <= 0.0 || self.cell_h <= 0.0 {
            return;
        }
        let avail = ui.available_size();
        let new_cols = ((avail.x / self.cell_w).floor() as u16).max(20);
        let new_rows = ((avail.y / self.cell_h).floor() as u16).max(5);
        if new_cols != self.cols || new_rows != self.rows {
            self.cols = new_cols;
            self.rows = new_rows;
            let _ = self.pty.master.resize(PtySize {
                rows: new_rows,
                cols: new_cols,
                pixel_width: 0,
                pixel_height: 0,
            });
            self.backend.lock().resize(new_rows, new_cols);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_pty();

        // Collect keystrokes destined for the PTY.
        let events: Vec<egui::Event> = ctx.input(|i| i.events.clone());
        let mut out = Vec::new();
        for ev in &events {
            if let Some(bytes) = translate_key(ev) {
                out.extend(bytes);
            }
        }
        if !out.is_empty() {
            self.write_to_pty(&out);
        }

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "term-spike · backend = {} · grid = {}x{}",
                    self.backend_name, self.cols, self.rows,
                ));
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.label(format!("font {:.0}pt", self.font_size));
                    },
                );
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.maybe_resize(ui);
            let snap = self.backend.lock().snapshot();
            render_snapshot(ui, &snap, self.font_size, &mut self.cell_w, &mut self.cell_h);
            self.last_text_size = Some(ui.min_size());
        });

        // Ask eframe to redraw continuously so terminal updates stream.
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

fn render_snapshot(
    ui: &mut egui::Ui,
    snap: &Snapshot,
    font_size: f32,
    cell_w_out: &mut f32,
    cell_h_out: &mut f32,
) {
    use egui::{Color32, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Vec2};

    let font = FontId::new(font_size, FontFamily::Monospace);
    // Measure an average cell width using 'M' and the font metrics.
    let (cell_w, cell_h) = ui.fonts(|f| {
        let g = f.glyph_width(&font, 'M');
        let row_h = f.row_height(&font);
        (g.max(1.0), row_h.max(1.0))
    });
    *cell_w_out = cell_w;
    *cell_h_out = cell_h;

    let desired = Vec2::new(cell_w * snap.cols as f32, cell_h * snap.rows as f32);
    let (rect, _resp) = ui.allocate_exact_size(desired, Sense::click());

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);

    for (row_idx, row) in snap.cells.iter().enumerate() {
        let y = rect.min.y + cell_h * row_idx as f32;
        for (col_idx, cell) in row.iter().enumerate() {
            let x = rect.min.x + cell_w * col_idx as f32;
            let cell_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell_w, cell_h));
            if cell.bg.0 != 0 || cell.bg.1 != 0 || cell.bg.2 != 0 {
                painter.rect_filled(cell_rect, 0.0, cell.bg.as_color32());
            }
            if cell.ch != ' ' && cell.ch != '\0' {
                painter.text(
                    Pos2::new(x, y),
                    egui::Align2::LEFT_TOP,
                    cell.ch,
                    font.clone(),
                    cell.fg.as_color32(),
                );
            }
        }
    }

    if snap.cursor_visible
        && (snap.cursor_row as usize) < snap.cells.len()
    {
        let x = rect.min.x + cell_w * snap.cursor_col as f32;
        let y = rect.min.y + cell_h * snap.cursor_row as f32;
        let c_rect =
            Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell_w, cell_h));
        painter.rect_stroke(c_rect, 0.0, Stroke::new(1.5, Color32::from_rgb(0xff, 0xaa, 0x00)));
    }
}

// -----------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------

pub fn run<F>(title: &str, make_backend: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(u16, u16) -> Box<dyn TermBackend>,
{
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_micros()
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    log::info!("spawning child: {:?}", args);

    let rows = DEFAULT_ROWS;
    let cols = DEFAULT_COLS;
    let pty = spawn_child(&args, rows, cols)?;
    let mut backend = make_backend(rows, cols);
    backend.resize(rows, cols);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 720.0]),
        ..Default::default()
    };
    let title = title.to_string();
    eframe::run_native(
        &title,
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(backend, pty, rows, cols)))),
    )
    .map_err(|e| format!("eframe error: {e}").into())
}
