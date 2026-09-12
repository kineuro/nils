// SPDX-License-Identifier: AGPL-3.0-only
//! The terminal for `nils setup` when a person is at one: colour in the
//! group's plum and cream, keys read one at a time, and the pieces the wizard
//! draws (the banner, the checklist while it works, the card when it is
//! done). Everything here that draws returns lines, so what a screen looks
//! like is tested without a terminal; only reading keys and the window's width
//! touch one.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The colours of the group's mark, as a terminal can show them: truecolor
/// where it says so, the nearest of the 256 otherwise, or none at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Palette {
    Truecolor,
    Ansi256,
    Plain,
}

impl Palette {
    /// The palette for this process: none when output is not a terminal or
    /// `NO_COLOR` is set, truecolor when `COLORTERM` says so.
    pub(crate) fn detect(terminal: bool) -> Palette {
        Palette::choose(
            terminal,
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        )
    }

    fn choose(
        terminal: bool,
        no_color: bool,
        colorterm: Option<&str>,
        term: Option<&str>,
    ) -> Palette {
        if !terminal || no_color || term == Some("dumb") {
            return Palette::Plain;
        }
        match colorterm {
            Some("truecolor" | "24bit") => Palette::Truecolor,
            _ => Palette::Ansi256,
        }
    }

    fn paint(self, text: &str, truecolor: &str, ansi256: &str) -> String {
        match self {
            Palette::Truecolor => format!("\x1b[{truecolor}m{text}\x1b[0m"),
            Palette::Ansi256 => format!("\x1b[{ansi256}m{text}\x1b[0m"),
            Palette::Plain => text.to_string(),
        }
    }

    /// The mark's plum, lifted so it reads on a dark terminal as well.
    pub(crate) fn plum(self, text: &str) -> String {
        self.paint(text, "1;38;2;196;120;168", "1;38;5;175")
    }

    pub(crate) fn cream(self, text: &str) -> String {
        self.paint(text, "38;2;246;245;242", "38;5;255")
    }

    pub(crate) fn bold(self, text: &str) -> String {
        self.paint(text, "1", "1")
    }

    pub(crate) fn dim(self, text: &str) -> String {
        self.paint(text, "2", "2")
    }

    pub(crate) fn good(self, text: &str) -> String {
        self.paint(text, "38;2;120;190;130", "38;5;114")
    }

    pub(crate) fn bad(self, text: &str) -> String {
        self.paint(text, "38;2;220;110;100", "38;5;167")
    }
}

/// How wide a line is on screen: its characters, without colour codes.
pub(crate) fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // a colour code runs to its final letter
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            width += 1;
        }
    }
    width
}

/// A line padded with spaces to a width on screen.
pub(crate) fn pad(text: &str, width: usize) -> String {
    let shown = visible_width(text);
    if shown >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - shown))
    }
}

/// Uncoloured text cut to a width on screen, with an ellipsis where it was
/// cut.
pub(crate) fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

// ------------------------------------------------------------------- keys

/// A key, as the wizard cares about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Backspace,
    Tab,
    /// Ctrl-C, read as a key: in raw mode it is not a signal, so the
    /// wizard can put the terminal back before it goes.
    Interrupt,
    /// Ctrl-U, which throws away the text typed so far.
    ClearLine,
    Char(char),
}

/// The keys in what a terminal sent. Arrows arrive as `ESC [ A` (or `ESC O A`
/// in application mode); an escape on its own is the Escape key.
pub(crate) fn parse_keys(bytes: &[u8]) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x1b if i + 2 < bytes.len() && matches!(bytes[i + 1], b'[' | b'O') => {
                let key = match bytes[i + 2] {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    b'C' => Some(Key::Right),
                    b'D' => Some(Key::Left),
                    _ => None,
                };
                match key {
                    Some(key) => {
                        keys.push(key);
                        i += 3;
                    }
                    None => {
                        // an escape sequence this does not use: skip to its end
                        let mut end = i + 2;
                        while end < bytes.len()
                            && !bytes[end].is_ascii_alphabetic()
                            && bytes[end] != b'~'
                        {
                            end += 1;
                        }
                        i = end + 1;
                    }
                }
            }
            0x1b => {
                keys.push(Key::Escape);
                i += 1;
            }
            b'\r' | b'\n' => {
                keys.push(Key::Enter);
                i += 1;
            }
            0x7f | 0x08 => {
                keys.push(Key::Backspace);
                i += 1;
            }
            b'\t' => {
                keys.push(Key::Tab);
                i += 1;
            }
            0x03 => {
                keys.push(Key::Interrupt);
                i += 1;
            }
            0x15 => {
                keys.push(Key::ClearLine);
                i += 1;
            }
            _ => {
                // a character, which may take more than one byte
                let len = utf8_len(bytes[i]);
                let end = (i + len).min(bytes.len());
                if let Ok(text) = std::str::from_utf8(&bytes[i..end]) {
                    for c in text.chars() {
                        if !c.is_control() {
                            keys.push(Key::Char(c));
                        }
                    }
                }
                i = end;
            }
        }
    }
    keys
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// A terminal in raw mode for as long as this lives: keys arrive one at a
/// time, unechoed, and Ctrl-C is a key. The terminal is put back when it is
/// dropped, however the wizard ends.
pub(crate) struct Raw {
    #[cfg(unix)]
    fd: i32,
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl Raw {
    #[cfg(unix)]
    #[allow(
        unsafe_code,
        reason = "tcgetattr and tcsetattr fill and read a plain struct through a pointer"
    )]
    pub(crate) fn on(fd: i32) -> Raw {
        let mut term = std::mem::MaybeUninit::<libc::termios>::zeroed();
        // SAFETY: `fd` is an open descriptor and the struct outlives the call.
        if unsafe { libc::tcgetattr(fd, term.as_mut_ptr()) } != 0 {
            return Raw { fd, saved: None };
        }
        // SAFETY: tcgetattr returned 0, so the struct is initialised.
        let saved = unsafe { term.assume_init() };
        let mut raw = saved;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
        raw.c_iflag &= !(libc::IXON | libc::ICRNL);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: `raw` is a copy of a struct the kernel just filled.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) };
        Raw {
            fd,
            saved: Some(saved),
        }
    }

    #[cfg(not(unix))]
    pub(crate) fn on(_fd: i32) -> Raw {
        Raw {}
    }

    /// Whether the terminal took raw mode.
    pub(crate) fn active(&self) -> bool {
        #[cfg(unix)]
        {
            self.saved.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

impl Drop for Raw {
    #[cfg(unix)]
    #[allow(
        unsafe_code,
        reason = "tcsetattr reads the struct saved when raw mode began"
    )]
    fn drop(&mut self) {
        if let Some(saved) = self.saved {
            // SAFETY: `saved` is the struct tcgetattr filled for this fd.
            unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &saved) };
        }
    }

    #[cfg(not(unix))]
    fn drop(&mut self) {}
}

/// The next keys from a terminal in raw mode: one read, which holds a whole
/// escape sequence when the terminal sends one at once. A terminal that is
/// gone reads as Ctrl-C, so the wizard ends rather than spins.
pub(crate) fn read_keys(input: &mut impl std::io::Read) -> Vec<Key> {
    let mut buffer = [0u8; 256];
    match input.read(&mut buffer) {
        Ok(n) if n > 0 => parse_keys(&buffer[..n]),
        _ => vec![Key::Interrupt],
    }
}

/// The terminal's size in columns and rows, 80 by 24 when it cannot say.
#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "TIOCGWINSZ fills a plain struct through a pointer"
)]
pub(crate) fn size(fd: i32) -> (usize, usize) {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    // SAFETY: `fd` is an open descriptor and the struct outlives the call.
    let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0;
    if !ok {
        return (80, 24);
    }
    // SAFETY: the ioctl returned 0, so the struct is initialised.
    let size = unsafe { size.assume_init() };
    let (cols, rows) = (usize::from(size.ws_col), usize::from(size.ws_row));
    (
        if cols == 0 { 80 } else { cols },
        if rows == 0 { 24 } else { rows },
    )
}

#[cfg(not(unix))]
pub(crate) fn size(_fd: i32) -> (usize, usize) {
    (80, 24)
}

/// The terminal's width in columns.
pub(crate) fn width(fd: i32) -> usize {
    size(fd).0
}

/// The terminal's other screen, where the steps are drawn, for as long as
/// this lives. The screen a person was looking at comes back when it goes,
/// however the wizard ends.
pub(crate) struct Screen;

impl Screen {
    pub(crate) fn enter() -> Screen {
        print!("\x1b[?1049h\x1b[?25l");
        let _ = std::io::stdout().flush();
        Screen
    }

    /// The whole screen drawn again, each line over the one that was there
    /// and whatever is below the last cleared.
    pub(crate) fn draw(&self, lines: &[String]) {
        let mut out = String::from("\x1b[H");
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                out.push_str("\r\n");
            }
            out.push_str(line);
            out.push_str("\x1b[K");
        }
        out.push_str("\x1b[J");
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // a new line first, for a terminal with no other screen to leave,
        // where whatever is said next would otherwise follow the keys' line
        print!("\x1b[?25h\r\n\x1b[?1049l");
        let _ = std::io::stdout().flush();
    }
}

// ------------------------------------------------------------- questions

/// A line a step has said above its question: a note, plain text, a row of
/// the plan, or a question already answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Said {
    Note(String),
    Text(String),
    Row(String, String),
    Answer(String, String),
}

/// The question being answered, with the answer so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ask {
    /// One of a list, each with a few words under it.
    Pick {
        options: Vec<(String, String)>,
        at: usize,
    },
    /// Text, shown as it is typed or hidden. Left empty it is `default`,
    /// unless it is `required`.
    Text {
        text: String,
        cursor: usize,
        hidden: bool,
        default: String,
        required: bool,
    },
}

/// What a key did to a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Stay,
    Answer,
    Back,
    Quit,
}

impl Ask {
    /// A key pressed on this question.
    pub(crate) fn key(&mut self, key: &Key) -> Outcome {
        match self {
            Ask::Pick { options, at } => {
                let n = options.len().max(1);
                match key {
                    Key::Up => *at = (*at + n - 1) % n,
                    Key::Down | Key::Tab => *at = (*at + 1) % n,
                    Key::Enter | Key::Right => return Outcome::Answer,
                    Key::Left | Key::Escape | Key::Backspace => return Outcome::Back,
                    Key::Interrupt => return Outcome::Quit,
                    Key::Char(c) => {
                        if let Some(d) = c.to_digit(10).and_then(|d| usize::try_from(d).ok())
                            && (1..=options.len()).contains(&d)
                        {
                            *at = d - 1;
                        }
                    }
                    _ => {}
                }
                Outcome::Stay
            }
            Ask::Text {
                text,
                cursor,
                required,
                ..
            } => {
                match key {
                    Key::Char(c) => {
                        text.insert(byte_at(text, *cursor), *c);
                        *cursor += 1;
                    }
                    Key::Backspace if *cursor > 0 => {
                        text.remove(byte_at(text, *cursor - 1));
                        *cursor -= 1;
                    }
                    Key::Left => *cursor = cursor.saturating_sub(1),
                    Key::Right => *cursor = (*cursor + 1).min(text.chars().count()),
                    Key::ClearLine => {
                        text.clear();
                        *cursor = 0;
                    }
                    Key::Enter if !(*required && text.is_empty()) => return Outcome::Answer,
                    Key::Escape => return Outcome::Back,
                    Key::Interrupt => return Outcome::Quit,
                    _ => {}
                }
                Outcome::Stay
            }
        }
    }
}

/// Where the character at `chars` begins in `text`, in bytes.
fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i)
}

/// Plain text broken into lines no wider than `width`: at spaces, and inside
/// a word only where the word is longer than a line.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let mut word = word.to_string();
            while word.chars().count() > width {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                lines.push(word.chars().take(width).collect());
                word = word.chars().skip(width).collect();
            }
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        lines.push(line);
    }
    lines
}

/// Everything a step's screen shows.
pub(crate) struct View<'a> {
    /// Each step's title, and what it was answered with once it has been.
    pub(crate) steps: &'a [(&'a str, Option<String>)],
    /// The step being asked, counted from 1; 0 before the first.
    pub(crate) current: usize,
    pub(crate) said: &'a [Said],
    pub(crate) question: &'a str,
    pub(crate) ask: &'a Ask,
    /// Whether there is an answer to go back to.
    pub(crate) back: bool,
}

/// How wide the list of steps down the side is.
const SIDE: usize = 36;

/// A step's screen: the banner where there is room, the steps down the side
/// with the current one marked and the answers given, and beside them what
/// this step has said, its question and the keys. In a narrow window the
/// steps are a line above the question instead.
pub(crate) fn screen(p: Palette, (width, height): (usize, usize), view: &View) -> Vec<String> {
    let wide = width >= SIDE + 56;
    let panel_width = if wide {
        width - SIDE - 3
    } else {
        width.saturating_sub(2)
    }
    .min(72);
    let (mut body, question_at) = panel(p, view, panel_width);
    let mut out = Vec::new();
    if height >= if wide { 24 } else { 30 } {
        out.extend(banner(p));
    }
    if !wide {
        match view.current.checked_sub(1).and_then(|i| view.steps.get(i)) {
            Some((title, _)) => out.push(format!(
                " {} {}",
                p.dim(&format!("Step {} of {} ·", view.current, view.steps.len())),
                p.bold(title)
            )),
            None => out.push(format!(" {}", p.bold("NILS setup"))),
        }
        let dots: Vec<String> = (1..=view.steps.len())
            .map(|n| {
                if n < view.current {
                    p.good("●")
                } else if n == view.current {
                    p.plum("●")
                } else {
                    p.dim("○")
                }
            })
            .collect();
        out.push(format!(" {}", dots.join(" ")));
        out.push(format!(" {}", p.dim(&"─".repeat(panel_width))));
    }
    let side = if wide {
        sidebar(p, view.steps, view.current)
    } else {
        Vec::new()
    };
    // what the banner, the header and the two lines of keys leave: the
    // lines above the question go first, then what does not fit below it
    let room = height.saturating_sub(out.len() + 2).max(side.len());
    if body.len() > room {
        let drop = (body.len() - room).min(question_at);
        body.drain(..drop);
        body.truncate(room);
    }
    if wide {
        for i in 0..body.len().max(side.len()) {
            let left = side.get(i).map_or("", String::as_str);
            let right = body.get(i).map_or("", String::as_str);
            out.push(format!(" {}{} {right}", pad(left, SIDE), p.dim("│")));
        }
        out.push(format!(" {}{}", pad("", SIDE), p.dim("│")));
        out.push(format!(" {}  {}", pad("", SIDE), hints(p, view)));
    } else {
        out.extend(body.into_iter().map(|line| format!(" {line}")));
        out.push(String::new());
        out.push(format!(" {}", hints(p, view)));
    }
    out.truncate(height);
    out
}

/// The steps, ticked with their answers up to the current one, which is
/// marked, and dim after it.
fn sidebar(p: Palette, steps: &[(&str, Option<String>)], current: usize) -> Vec<String> {
    steps
        .iter()
        .enumerate()
        .map(|(i, (title, answer))| {
            let n = i + 1;
            if n < current {
                format!(
                    "{} {}{}",
                    p.good("✓"),
                    pad(title, 19),
                    p.dim(&truncate(answer.as_deref().unwrap_or(""), SIDE - 22))
                )
            } else if n == current {
                format!("{} {}", p.plum("▸"), p.bold(title))
            } else {
                format!("  {}", p.dim(title))
            }
        })
        .collect()
}

/// What the current step has said, then its question and the answer being
/// given, and where the question begins.
fn panel(p: Palette, view: &View, width: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    for said in view.said {
        match said {
            Said::Note(text) => out.extend(wrap(text, width).iter().map(|l| p.dim(l))),
            Said::Text(text) => out.extend(wrap(text, width)),
            Said::Row(key, value) => {
                for (i, line) in wrap(value, width.saturating_sub(13)).iter().enumerate() {
                    let key = if i == 0 { key.as_str() } else { "" };
                    out.push(format!("{} {line}", p.dim(&format!("{key:<12}"))));
                }
            }
            Said::Answer(question, answer) => {
                if question.chars().count() + answer.chars().count() + 4 <= width {
                    out.push(format!("{} {}  {}", p.good("✓"), p.dim(question), answer));
                } else {
                    out.push(format!("{} {}", p.good("✓"), p.dim(question)));
                    out.extend(
                        wrap(answer, width.saturating_sub(2))
                            .iter()
                            .map(|l| format!("  {l}")),
                    );
                }
            }
        }
    }
    if !out.is_empty() {
        out.push(String::new());
    }
    let question_at = out.len();
    out.extend(wrap(view.question, width).iter().map(|l| p.bold(l)));
    out.push(String::new());
    match view.ask {
        Ask::Pick { options, at } => {
            for (i, (title, hint)) in options.iter().enumerate() {
                if i == *at {
                    out.push(format!("{} {}", p.plum("▸"), p.plum(title)));
                } else {
                    out.push(format!("  {title}"));
                }
                if !hint.is_empty() {
                    for line in wrap(hint, width.saturating_sub(2)) {
                        out.push(format!("  {}", p.dim(&line)));
                    }
                }
            }
        }
        Ask::Text {
            text,
            cursor,
            hidden,
            default,
            required,
        } => {
            let shown: Vec<char> = if *hidden {
                vec!['•'; text.chars().count()]
            } else {
                text.chars().collect()
            };
            // what fits of the text, with the cursor in view
            let room = width.saturating_sub(4).max(8);
            let start = (cursor + 1).saturating_sub(room);
            let before: String = shown[start..*cursor].iter().collect();
            let under = shown.get(*cursor).copied().unwrap_or(' ');
            let after: String = shown
                .iter()
                .skip(cursor + 1)
                .take(room.saturating_sub(cursor - start + 1))
                .collect();
            let mut line = format!("{} {before}\x1b[7m{under}\x1b[27m{after}", p.plum("›"));
            if text.is_empty() && !default.is_empty() && !*required {
                line.push_str(&p.dim(&format!(" empty takes {default}")));
            }
            out.push(line);
        }
    }
    (out, question_at)
}

/// The keys this question takes.
fn hints(p: Palette, view: &View) -> String {
    let mut keys = match view.ask {
        Ask::Pick { .. } => vec!["↑↓ choose", "⏎ next"],
        Ask::Text { .. } => vec!["⏎ next"],
    };
    if view.back {
        keys.push(match view.ask {
            Ask::Pick { .. } => "← back",
            Ask::Text { .. } => "esc back",
        });
    }
    keys.push("ctrl-c quit");
    p.dim(&keys.join("   "))
}

// --------------------------------------------------------------- drawing

/// The wordmark, the system's name, Karolinska Institutet and the link.
pub(crate) fn banner(p: Palette) -> Vec<String> {
    vec![
        String::new(),
        format!(" {}", p.plum("╔╗╔ ╦ ╦   ╔═╗")),
        format!(
            " {}   {}",
            p.plum("║║║ ║ ║   ╚═╗"),
            p.cream("Neuroimaging Intelligent Linked System")
        ),
        format!(
            " {}   {}",
            p.plum("╝╚╝ ╩ ╩═╝ ╚═╝"),
            p.dim("Karolinska Institutet · kineuro.se/nils")
        ),
        String::new(),
    ]
}

/// A bar `width` wide, filled for `done` of `all`.
pub(crate) fn bar(p: Palette, done: usize, all: usize, width: usize) -> String {
    let all = all.max(1);
    let done = done.min(all);
    let filled = width * done / all;
    format!(
        "{}{}",
        p.plum(&"█".repeat(filled)),
        p.dim(&"░".repeat(width - filled))
    )
}

/// Where one thing the install does has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskState {
    Waiting,
    /// Running for this many seconds.
    Running(u64),
    /// Done, in this many seconds.
    Done(u64),
    Failed,
    /// Never reached, in an install that finished without it.
    Skipped,
}

/// The spinner's frame for a moment in tenths of a second.
pub(crate) fn spinner(tenths: u64) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[(tenths % FRAMES.len() as u64) as usize]
}

/// How wide the checklist is on screen, less its one column of margin.
pub(crate) const CHECKLIST_WIDTH: usize = 56;

/// Everything the install will do, listed before it starts: a tick and its
/// time for each finished step, a spinner and what it is doing on the current
/// one, and one bar for the whole install.
pub(crate) fn checklist(
    p: Palette,
    title: &str,
    tasks: &[(String, TaskState)],
    detail: &str,
    tenths: u64,
) -> Vec<String> {
    const NAME: usize = 18;
    const TIME: usize = 5;
    let width = CHECKLIST_WIDTH;
    let finished = tasks
        .iter()
        .filter(|(_, s)| !matches!(s, TaskState::Waiting | TaskState::Running(_)))
        .count();
    let all = tasks.len();
    let percent = (100 * finished).checked_div(all).unwrap_or(100);
    let count = format!("{finished} of {all}");
    let mut out = vec![
        format!(
            " {}{}",
            pad(&p.bold(title), width - count.len()),
            p.dim(&count)
        ),
        format!(" {} {percent:>3}%", bar(p, finished, all, width - 5)),
        String::new(),
    ];
    let detail_width = width - 2 - NAME - TIME;
    for (name, state) in tasks {
        let (mark, time) = match state {
            TaskState::Waiting => (p.dim("·"), String::new()),
            TaskState::Running(s) => (p.plum(&spinner(tenths).to_string()), format!("{s}s")),
            TaskState::Done(s) => (p.good("✓"), format!("{s}s")),
            TaskState::Failed => (p.bad("✗"), String::new()),
            TaskState::Skipped => (p.dim("–"), String::new()),
        };
        let label = match state {
            TaskState::Waiting | TaskState::Skipped => p.dim(name),
            TaskState::Failed => p.bad(name),
            _ => name.clone(),
        };
        let doing = match state {
            TaskState::Running(_) if !detail.is_empty() => {
                p.dim(&truncate(detail, detail_width - 1))
            }
            _ => String::new(),
        };
        out.push(format!(
            " {mark} {}{}{}",
            pad(&label, NAME),
            pad(&doing, detail_width),
            p.dim(&format!("{time:>TIME$}"))
        ));
    }
    out
}

/// The checklist drawn in place while an install works, and again ten times
/// a second so its spinner turns. Nothing else may write to the terminal
/// while it is drawn, so what the install says meanwhile is kept by the
/// caller and shown once this is finished.
pub(crate) struct Live {
    board: Arc<Mutex<Board>>,
    drawer: Option<std::thread::JoinHandle<()>>,
}

struct Board {
    palette: Palette,
    title: String,
    rows: Vec<(String, TaskState, Option<Instant>)>,
    detail: String,
    started: Instant,
    /// How many lines the last drawing took, to move back over them.
    drawn: usize,
    stop: bool,
}

impl Board {
    fn draw(&mut self) {
        let tasks: Vec<(String, TaskState)> = self
            .rows
            .iter()
            .map(|(name, state, since)| {
                let state = match state {
                    TaskState::Running(_) => {
                        TaskState::Running(since.map_or(0, |s| s.elapsed().as_secs()))
                    }
                    other => other.clone(),
                };
                (name.clone(), state)
            })
            .collect();
        let tenths = u64::try_from(self.started.elapsed().as_millis() / 100).unwrap_or(0);
        let lines = checklist(self.palette, &self.title, &tasks, &self.detail, tenths);
        let mut out = String::new();
        if self.drawn > 0 {
            let _ = write!(out, "\x1b[{}F", self.drawn);
        }
        for line in &lines {
            let _ = writeln!(out, "\x1b[2K{line}");
        }
        self.drawn = lines.len();
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }

    /// The running row, finished as `into` says for the seconds it took.
    fn settle(&mut self, into: impl Fn(u64) -> TaskState) {
        for (_, state, since) in &mut self.rows {
            if matches!(state, TaskState::Running(_)) {
                *state = into(since.map_or(0, |s| s.elapsed().as_secs()));
            }
        }
    }
}

impl Live {
    /// Draw the rows, every one waiting, and keep drawing them.
    pub(crate) fn start(palette: Palette, title: &str, rows: Vec<String>) -> Live {
        let board = Arc::new(Mutex::new(Board {
            palette,
            title: title.to_string(),
            rows: rows
                .into_iter()
                .map(|name| (name, TaskState::Waiting, None))
                .collect(),
            detail: String::new(),
            started: Instant::now(),
            drawn: 0,
            stop: false,
        }));
        // the cursor hidden, or it blinks over the spinner
        print!("\x1b[?25l");
        if let Ok(mut b) = board.lock() {
            b.draw();
        }
        let shared = Arc::clone(&board);
        let drawer = std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let Ok(mut b) = shared.lock() else { break };
                if b.stop {
                    break;
                }
                b.draw();
            }
        });
        Live {
            board,
            drawer: Some(drawer),
        }
    }

    /// The row at `index` is under way, and the one before it is done.
    pub(crate) fn begin(&self, index: usize) {
        if let Ok(mut b) = self.board.lock() {
            b.settle(TaskState::Done);
            if let Some(row) = b.rows.get_mut(index) {
                row.1 = TaskState::Running(0);
                row.2 = Some(Instant::now());
            }
            b.detail.clear();
            b.draw();
        }
    }

    /// What the running row is doing now, in a few words.
    pub(crate) fn detail(&self, text: &str) {
        if let Ok(mut b) = self.board.lock() {
            b.detail = text.to_string();
        }
    }

    /// The running row did not do what it was for; the install goes on.
    pub(crate) fn falter(&self) {
        if let Ok(mut b) = self.board.lock() {
            b.settle(|_| TaskState::Failed);
        }
    }

    /// The last drawing: the running row done, or failed when the install
    /// stopped, and the rows it never reached marked as skipped when it
    /// did not.
    pub(crate) fn finish(mut self, ok: bool, title: &str) {
        if let Ok(mut b) = self.board.lock() {
            if ok {
                b.settle(TaskState::Done);
                for (_, state, _) in &mut b.rows {
                    if *state == TaskState::Waiting {
                        *state = TaskState::Skipped;
                    }
                }
            } else {
                b.settle(|_| TaskState::Failed);
            }
            b.title = title.to_string();
            b.detail.clear();
            b.stop = true;
            b.draw();
        }
        self.stop();
    }

    fn stop(&mut self) {
        let Some(drawer) = self.drawer.take() else {
            return;
        };
        if let Ok(mut b) = self.board.lock() {
            b.stop = true;
        }
        let _ = drawer.join();
        print!("\x1b[?25h");
        let _ = std::io::stdout().flush();
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The card at the end: what runs and where, and each service marked as
/// running or not.
pub(crate) fn card(
    p: Palette,
    title: &str,
    rows: &[(&str, String)],
    services: &[(String, bool)],
) -> Vec<String> {
    let key_width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let inner = rows
        .iter()
        .map(|(_, v)| key_width + 2 + visible_width(v))
        .chain(std::iter::once(visible_width(title) + 3))
        .max()
        .unwrap_or(0)
        .max(40);
    let mut out = Vec::new();
    let heading = format!("─ {} ", p.bold(title));
    out.push(format!(
        " {}{}{}",
        p.plum("╭"),
        p.plum(&heading),
        p.plum(&format!(
            "{}╮",
            "─".repeat(inner + 2 - visible_width(&heading))
        ))
    ));
    for (key, value) in rows {
        out.push(format!(
            " {} {}{} {}",
            p.plum("│"),
            p.dim(&format!("{key:<key_width$}  ")),
            pad(value, inner - key_width - 2),
            p.plum("│")
        ));
    }
    out.push(format!(
        " {}",
        p.plum(&format!("╰{}╯", "─".repeat(inner + 2)))
    ));
    if !services.is_empty() {
        out.push(format!(
            " {}",
            services
                .iter()
                .map(|(name, up)| {
                    if *up {
                        format!("{} {name}", p.good("●"))
                    } else {
                        format!("{} {name}", p.bad("●"))
                    }
                })
                .collect::<Vec<_>>()
                .join("  ")
        ));
    }
    out
}

/// What a person runs next, as label, command and a few words, the label
/// given once for the lines that share it.
pub(crate) fn next_steps(p: Palette, lines: &[(&str, &str, &str)]) -> Vec<String> {
    let label_width = lines
        .iter()
        .map(|(l, _, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    // the words line up after the commands that have some; a line with none,
    // such as the documentation's address, does not push them out
    let command_width = lines
        .iter()
        .filter(|(_, _, said)| !said.is_empty())
        .map(|(_, c, _)| c.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = Vec::new();
    let mut previous = "";
    for (label, command, said) in lines {
        let shown = if *label == previous { "" } else { label };
        previous = label;
        let head = format!(" {}  ", p.bold(&format!("{shown:<label_width$}")));
        if said.is_empty() {
            out.push(format!("{head}{command}"));
        } else {
            out.push(format!(
                "{head}{}  {}",
                pad(command, command_width),
                p.dim(said)
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_is_chosen_from_what_the_terminal_says() {
        assert_eq!(
            Palette::choose(true, false, Some("truecolor"), Some("xterm-256color")),
            Palette::Truecolor
        );
        assert_eq!(
            Palette::choose(true, false, None, Some("xterm-256color")),
            Palette::Ansi256
        );
        assert_eq!(
            Palette::choose(true, true, Some("truecolor"), None),
            Palette::Plain,
            "NO_COLOR"
        );
        assert_eq!(
            Palette::choose(false, false, Some("truecolor"), None),
            Palette::Plain
        );
        assert_eq!(
            Palette::choose(true, false, None, Some("dumb")),
            Palette::Plain
        );
        assert_eq!(
            Palette::Plain.plum("NILS"),
            "NILS",
            "no codes without colour"
        );
        assert_eq!(visible_width(&Palette::Truecolor.plum("NILS")), 4);
    }

    #[test]
    fn keys_are_read_from_what_a_terminal_sends() {
        assert_eq!(
            parse_keys(b"\x1b[A\x1b[B\x1b[C\x1b[D"),
            vec![Key::Up, Key::Down, Key::Right, Key::Left]
        );
        assert_eq!(parse_keys(b"\x1bOA"), vec![Key::Up], "application mode");
        assert_eq!(parse_keys(b"\r"), vec![Key::Enter]);
        assert_eq!(parse_keys(b"\x1b"), vec![Key::Escape], "escape on its own");
        assert_eq!(parse_keys(&[0x7f]), vec![Key::Backspace]);
        assert_eq!(parse_keys(&[0x03]), vec![Key::Interrupt]);
        assert_eq!(parse_keys(&[0x15]), vec![Key::ClearLine]);
        assert_eq!(
            parse_keys("é1".as_bytes()),
            vec![Key::Char('é'), Key::Char('1')]
        );
        assert_eq!(
            parse_keys(b"\x1b[3~x"),
            vec![Key::Char('x')],
            "a sequence it does not use is skipped whole"
        );
    }

    #[test]
    fn a_pick_goes_round_and_answers_or_goes_back() {
        let mut ask = Ask::Pick {
            options: vec![
                ("a".to_string(), String::new()),
                ("b".to_string(), String::new()),
                ("c".to_string(), String::new()),
            ],
            at: 0,
        };
        assert_eq!(ask.key(&Key::Up), Outcome::Stay);
        assert!(
            matches!(ask, Ask::Pick { at: 2, .. }),
            "up from the first is the last"
        );
        ask.key(&Key::Down);
        assert!(matches!(ask, Ask::Pick { at: 0, .. }));
        ask.key(&Key::Char('2'));
        assert!(
            matches!(ask, Ask::Pick { at: 1, .. }),
            "a number moves to its place"
        );
        assert_eq!(ask.key(&Key::Enter), Outcome::Answer);
        assert_eq!(ask.key(&Key::Left), Outcome::Back);
        assert_eq!(ask.key(&Key::Escape), Outcome::Back);
        assert_eq!(ask.key(&Key::Interrupt), Outcome::Quit);
    }

    #[test]
    fn text_is_typed_and_edited_and_needed_where_it_must_be() {
        let mut ask = Ask::Text {
            text: String::new(),
            cursor: 0,
            hidden: false,
            default: String::new(),
            required: true,
        };
        for c in "héj".chars() {
            ask.key(&Key::Char(c));
        }
        ask.key(&Key::Left);
        ask.key(&Key::Char('x'));
        assert!(
            matches!(&ask, Ask::Text { text, cursor: 3, .. } if text == "héxj"),
            "{ask:?}"
        );
        ask.key(&Key::Backspace);
        assert!(
            matches!(&ask, Ask::Text { text, cursor: 2, .. } if text == "héj"),
            "{ask:?}"
        );
        ask.key(&Key::ClearLine);
        assert_eq!(
            ask.key(&Key::Enter),
            Outcome::Stay,
            "a needed answer is not taken empty"
        );
        ask.key(&Key::Char('a'));
        assert_eq!(ask.key(&Key::Enter), Outcome::Answer);
        assert_eq!(
            ask.key(&Key::Left),
            Outcome::Stay,
            "in text the arrows move the cursor"
        );
        assert_eq!(ask.key(&Key::Escape), Outcome::Back);
    }

    #[test]
    fn text_wraps_at_spaces_and_inside_a_word_only_when_it_must() {
        assert_eq!(
            wrap("the quick brown fox", 10),
            vec!["the quick", "brown fox"]
        );
        assert_eq!(
            wrap("aaaaaaaaaaaaaaaaaaaa b", 8),
            vec!["aaaaaaaa", "aaaaaaaa", "aaaa b"]
        );
        assert_eq!(wrap("", 10), vec![String::new()]);
    }

    #[test]
    fn a_screen_has_the_steps_down_the_side_and_fits_its_terminal() {
        let steps: Vec<(&str, Option<String>)> = vec![
            ("What to install", Some("everything".to_string())),
            ("Where it runs", None),
            ("Where it lives", None),
            ("The registry", None),
            ("Who may sign in", None),
            ("What it can do", None),
            ("Keeping it running", None),
            ("The plan", None),
        ];
        let said = vec![Said::Note("docker is here but does not answer".to_string())];
        let ask = Ask::Pick {
            options: vec![
                (
                    "On this machine".to_string(),
                    "two small binaries and a directory".to_string(),
                ),
                (
                    "In containers (podman)".to_string(),
                    "one pod, run as you".to_string(),
                ),
            ],
            at: 1,
        };
        let view = View {
            steps: &steps,
            current: 2,
            said: &said,
            question: "How should the parts run?",
            ask: &ask,
            back: true,
        };
        for palette in [Palette::Plain, Palette::Truecolor] {
            let wide = screen(palette, (100, 30), &view);
            assert!(wide.len() <= 30, "{wide:#?}");
            let text: Vec<String> = wide.iter().map(|l| strip(l)).collect();
            let bars: Vec<usize> = text
                .iter()
                .filter_map(|l| l.chars().position(|c| c == '│'))
                .collect();
            assert!(
                !bars.is_empty() && bars.iter().all(|b| *b == bars[0]),
                "the side lines up: {text:#?}"
            );
            let all = text.join("\n");
            assert!(
                all.contains("✓ What to install") && all.contains("everything"),
                "{all}"
            );
            assert!(
                all.contains("▸ Where it runs") && all.contains("▸ In containers (podman)"),
                "{all}"
            );
            assert!(
                all.contains("Neuroimaging Intelligent Linked System"),
                "the banner, where there is room: {all}"
            );
            assert!(all.contains("← back"), "{all}");
        }
        let narrow = screen(Palette::Plain, (60, 20), &view).join("\n");
        assert!(
            narrow.contains("Step 2 of 8 · Where it runs") && !narrow.contains('│'),
            "{narrow}"
        );
        assert!(narrow.lines().count() <= 20);
        let short = screen(Palette::Plain, (100, 12), &view);
        assert!(
            short.len() <= 12
                && short
                    .iter()
                    .any(|l| l.contains("How should the parts run?")),
            "{short:#?}"
        );
    }

    #[test]
    fn the_banner_carries_the_name_the_institute_and_the_link() {
        let lines = banner(Palette::Plain).join("\n");
        assert!(lines.contains("╔╗╔ ╦ ╦   ╔═╗"), "{lines}");
        assert!(
            lines.contains("Neuroimaging Intelligent Linked System"),
            "{lines}"
        );
        assert!(
            lines.contains("Karolinska Institutet · kineuro.se/nils"),
            "{lines}"
        );
    }

    /// What a line reads as, without its colour codes.
    fn strip(text: &str) -> String {
        let mut out = String::new();
        let mut code = false;
        for c in text.chars() {
            if c == '\x1b' {
                code = true;
            } else if code {
                code = !c.is_ascii_alphabetic();
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn the_checklist_counts_what_is_done_and_marks_the_rest() {
        let tasks = vec![
            ("engine image".to_string(), TaskState::Done(6)),
            ("desk image".to_string(), TaskState::Done(2)),
            ("registry".to_string(), TaskState::Running(3)),
            ("services".to_string(), TaskState::Waiting),
        ];
        let lines = checklist(Palette::Plain, "Installing", &tasks, "making the key", 0);
        assert!(
            lines[0].contains("Installing") && lines[0].contains("2 of 4"),
            "{lines:?}"
        );
        assert!(lines[1].contains(" 50%"), "{lines:?}");
        let coloured = checklist(
            Palette::Truecolor,
            "Installing",
            &tasks,
            "making the key",
            0,
        );
        for (plain, colour) in lines.iter().zip(&coloured) {
            assert_eq!(
                visible_width(plain),
                visible_width(colour),
                "as wide in colour: {plain}"
            );
            assert_eq!(strip(colour), *plain);
        }
        assert!(
            lines
                .iter()
                .filter(|l| !l.is_empty())
                .all(|l| visible_width(l) == CHECKLIST_WIDTH + 1),
            "every line as wide: {lines:?}"
        );
        assert!(
            lines[3].starts_with(" ✓ engine image") && lines[3].ends_with("6s"),
            "{lines:?}"
        );
        assert!(
            lines[5].starts_with(" ⠋ registry") && lines[5].contains("making the key"),
            "{lines:?}"
        );
        assert!(lines[6].starts_with(" · services"), "{lines:?}");

        let long = checklist(Palette::Plain, "Installing", &tasks, &"x".repeat(80), 0);
        assert_eq!(
            visible_width(&long[5]),
            CHECKLIST_WIDTH + 1,
            "a long detail is cut: {}",
            long[5]
        );
        assert!(long[5].contains('…'), "{}", long[5]);

        let ended = vec![
            ("engine image".to_string(), TaskState::Done(6)),
            ("gateway".to_string(), TaskState::Failed),
            ("assistant".to_string(), TaskState::Skipped),
        ];
        let lines = checklist(Palette::Plain, "Installed", &ended, "", 0);
        assert!(
            lines[0].contains("3 of 3") && lines[1].contains("100%"),
            "{lines:?}"
        );
        assert!(lines[4].starts_with(" ✗ gateway"), "{lines:?}");
        assert!(lines[5].starts_with(" – assistant"), "{lines:?}");
    }

    #[test]
    fn the_card_lines_up_whatever_it_holds() {
        let rows = vec![
            ("desk", "http://127.0.0.1:7200".to_string()),
            ("registry", "Postgres 17 in ~/nils/postgres".to_string()),
        ];
        let services = vec![("engine".to_string(), true), ("desk".to_string(), false)];
        let next = [
            ("Next", "nils digest <dir>", "bring DICOM in"),
            ("Next", "nils uninstall", "remove it"),
            ("Docs", "https://kineuro.se/nils/docs/", ""),
        ];
        for palette in [Palette::Plain, Palette::Truecolor] {
            let lines = card(palette, "NILS is running", &rows, &services);
            let box_widths: Vec<usize> = lines[..4].iter().map(|l| visible_width(l)).collect();
            assert!(
                box_widths.iter().all(|w| *w == box_widths[0]),
                "every line of the box as wide: {box_widths:?} {lines:?}"
            );
            let text = strip(&lines.join("\n"));
            assert!(text.contains("╭─ NILS is running"), "{text}");
            assert!(
                text.contains("● engine") && text.contains("● desk"),
                "{text}"
            );

            let after = strip(&next_steps(palette, &next).join("\n"));
            assert!(
                after.starts_with(" Next  nils digest <dir>  bring DICOM in"),
                "{after}"
            );
            assert!(
                after.contains("\n       nils uninstall     remove it"),
                "the label once, the words lined up: {after}"
            );
            assert!(
                after.ends_with("\n Docs  https://kineuro.se/nils/docs/"),
                "{after}"
            );
        }
    }
}
