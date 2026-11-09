//! "Send books": while this screen is open, a tiny HTTP server on the Wi-Fi takes books
//! uploaded from a phone or computer browser and drops them into the library folder.
//!
//! The screen shows a QR code and the same URL as text. The URL carries a random token, the
//! only trust boundary: anything else gets a 404, and uploaded names are checked (a plain
//! file name with a book extension, never overwriting an existing file).

use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use zarathustra_device::{Canvas, Rect};

use crate::i18n::t;
use zarathustra_engine::library::Format;

use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::ui::{rounded_outline, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{screen_h, screen_w, Ctx};

const PAGE: &str = include_str!("transfer.html");
const MAX_SIZE: u64 = 500 << 20;
const MAX_HEAD: usize = 8 << 10;
const PORTS: std::ops::RangeInclusive<u16> = 8080..=8089;
/// Side of the QR code with its quiet zone: about 5.5 cm, easy for a phone at arm's length.
const QR_SIZE: i32 = 660;
fn retry() -> Rect {
    Rect { x: (screen_w() - 320) / 2, y: 900, w: 320, h: 96 }
}

/// What the server thread reports, in the order it happens (one upload at a time).
enum Event {
    Started(String),
    /// The file name it was saved under (a " (2)" may have been added).
    Done(String),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Receiving,
    Done,
    Failed,
}

/// The listening server; dropping it stops the thread and closes the port again.
struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    events: Receiver<Event>,
}

impl Server {
    fn start(dir: PathBuf, token: String) -> io::Result<Server> {
        let listener = PORTS
            .clone()
            .find_map(|p| TcpListener::bind(("0.0.0.0", p)).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "no free port"))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, events) = mpsc::channel();
        let flag = stop.clone();
        std::thread::Builder::new().name("transfer".into()).spawn(move || {
            for conn in listener.incoming() {
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = conn else { continue };
                // A thread per connection: a browser's idle preconnect must not hold up the
                // real request behind it for the read timeout.
                let (token, dir, flag, tx) = (token.clone(), dir.clone(), flag.clone(), tx.clone());
                let spawned = std::thread::Builder::new().name("transfer-conn".into()).spawn(move || {
                    if let Err(e) = handle(stream, &token, &dir, &flag, &tx) {
                        eprintln!("transfer: {e}");
                    }
                });
                if let Err(e) = spawned {
                    eprintln!("transfer: {e}");
                }
            }
        })?;
        firewall(true, port);
        Ok(Server { port, stop, events })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the blocking accept so the thread sees the flag and lets the port go.
        // ponytail: if this connect fails the thread lingers on the port until exit; the next
        // opening takes the next port of `PORTS`.
        let to = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let _ = TcpStream::connect_timeout(&to, Duration::from_millis(300));
        firewall(false, self.port);
    }
}

/// Kindle firmware drops incoming TCP by default (KOReader opens its SSH port the same way):
/// let the port through while the screen is open. Other systems are left alone.
// ponytail: if the app dies with the screen open, the rule stays until reboot (one port of
// `PORTS`, only this server ever listens there).
fn firewall(open: bool, port: u16) {
    if zarathustra_device::platform().kind != zarathustra_device::Kind::Kindle {
        return;
    }
    let port = port.to_string();
    let rule = [if open { "-I" } else { "-D" }, "INPUT", "-p", "tcp", "--dport", &port, "-j", "ACCEPT"];
    match std::process::Command::new("iptables").args(rule).status() {
        Ok(s) if s.success() => {}
        r => eprintln!("transfer: iptables {rule:?}: {r:?}"),
    }
}

/// This device's address on the network with the default route; `None` without Wi-Fi.
/// A UDP "connect" only picks the route: nothing is sent.
pub(crate) fn local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
}

/// Six random characters, unambiguous to type (no 0/o, 1/l/i): about 30 bits.
fn token() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    // std seeds `RandomState` from the OS random source: no extra dependency for a token.
    let mut n = std::collections::hash_map::RandomState::new().build_hasher().finish();
    (0..6)
        .map(|_| {
            let c = ALPHABET[(n % ALPHABET.len() as u64) as usize] as char;
            n /= ALPHABET.len() as u64;
            c
        })
        .collect()
}

pub enum Tap {
    None,
    Redraw,
    Back,
}

pub struct Transfer {
    dir: PathBuf,
    /// `None` without a network (or when no port could be opened).
    server: Option<Server>,
    url: String,
    /// The URL's QR code: dark modules row by row, `qr_width` per row.
    qr: Vec<bool>,
    qr_width: usize,
    files: Vec<(String, Status)>,
}

impl Transfer {
    /// Opens the screen and starts serving, when there is a network.
    pub fn open() -> Transfer {
        // Books go to library/, the folder the home shows (not Amazon's documents/).
        let dir = crate::library_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("transfer: {}: {e}", dir.display());
        }
        let mut t = Transfer { dir, server: None, url: String::new(), qr: Vec::new(), qr_width: 0, files: Vec::new() };
        t.start();
        t
    }

    fn start(&mut self) {
        let Some(ip) = local_ip() else { return };
        let token = token();
        match Server::start(self.dir.clone(), token.clone()) {
            Ok(server) => {
                let url = format!("http://{ip}:{}/{token}", server.port);
                self.server = Some(server);
                self.set_url(url);
            }
            Err(e) => eprintln!("transfer: {e}"),
        }
    }

    /// Shows `url` (also how the desktop snapshot fakes a network).
    pub fn set_url(&mut self, url: String) {
        if let Ok(code) = qrcode::QrCode::with_error_correction_level(&url, qrcode::EcLevel::M) {
            self.qr_width = code.width();
            self.qr = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
        }
        self.url = url;
    }

    /// Takes in what the server did since the last call; true if the screen changed.
    pub fn poll(&mut self) -> bool {
        let Some(server) = &self.server else { return false };
        let mut changed = false;
        for ev in server.events.try_iter() {
            changed = true;
            match ev {
                Event::Started(name) => self.files.push((name, Status::Receiving)),
                Event::Done(name) => {
                    if let Some(f) = self.files.last_mut() {
                        *f = (name, Status::Done);
                    }
                }
                Event::Failed => {
                    if let Some(f) = self.files.last_mut() {
                        f.1 = Status::Failed;
                    }
                }
            }
        }
        changed
    }

    /// Whether any book arrived (the library needs a look again).
    pub fn received(&self) -> bool {
        self.files.iter().any(|f| f.1 == Status::Done)
    }

    pub fn tap(&mut self, (x, y): (i32, i32)) -> Tap {
        if y < TOPBAR_H && x < 400 {
            return Tap::Back;
        }
        if self.url.is_empty() && retry().contains(x, y) {
            self.start();
            return Tap::Redraw;
        }
        Tap::None
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(sans_bold), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::sans(fonts, true), crate::serif(fonts, true))
        else {
            return;
        };
        let s = t();
        topbar(c, raster, sans, serif_bold, s.back_library, s.send_books);
        let soft = Label { face: sans, size: 30.0, ink: INK_SOFT };
        let width = (screen_w() - 2 * PAD) as f32;

        if self.url.is_empty() {
            let head = Label { face: serif_bold, size: 50.0, ink: INK };
            head.draw_centred(c, raster, s.no_wifi, screen_w() as f32 / 2.0, 560);
            let hint = s.no_wifi_hint.replace("{device}", zarathustra_device::platform().name);
            for (n, line) in soft.wrap(&hint, width - 120.0, 4).iter().enumerate() {
                soft.draw_centred(c, raster, line, screen_w() as f32 / 2.0, 650 + n as i32 * 44);
            }
            rounded_outline(c, retry(), retry().h / 2, 3, INK, PAPER);
            Label { face: sans, size: 34.0, ink: INK }.centered(c, raster, s.retry, retry());
            return;
        }

        for (n, line) in soft.wrap(s.transfer_hint, width, 2).iter().enumerate() {
            soft.draw_centred(c, raster, line, screen_w() as f32 / 2.0, TOPBAR_H + 80 + n as i32 * 42);
        }
        // Whole pixels per module, with the standard four-module quiet zone around.
        let side = self.qr_width as i32;
        let module = QR_SIZE / (side + 8).max(1);
        let (x0, y0) = ((screen_w() - module * side) / 2, TOPBAR_H + 170 + 4 * module);
        for (i, _) in self.qr.iter().enumerate().filter(|(_, &dark)| dark) {
            let (col, row) = ((i % self.qr_width) as i32, (i / self.qr_width) as i32);
            c.fill(Rect::new(x0 + col * module, y0 + row * module, module, module), INK);
        }
        let url_y = y0 + side * module + 4 * module + 40;
        let url = Label { face: sans_bold, size: 42.0, ink: INK };
        url.draw_centred(c, raster, &url.fit(&self.url, width), screen_w() as f32 / 2.0, url_y);

        // What arrived, the latest first, as many as fit.
        let mut y = url_y + 70;
        c.fill(Rect::new(PAD, y, screen_w() - 2 * PAD, 2), LINE);
        y += 60;
        let done = self.files.iter().filter(|f| f.1 == Status::Done).count();
        let caption = Label { face: sans, size: 26.0, ink: INK_SOFT };
        if self.files.is_empty() {
            caption.draw(c, raster, s.waiting_books, PAD as f32, y);
            return;
        }
        caption.draw(c, raster, &format!("{}: {done}", s.received), PAD as f32, y);
        let name = Label { face: sans, size: 30.0, ink: INK };
        for (file, status) in self.files.iter().rev() {
            y += 54;
            if y > screen_h() - 40 {
                break;
            }
            let state = match status {
                Status::Receiving => s.receiving,
                Status::Done => "✓",
                Status::Failed => s.failed,
            };
            let sw = soft.width(state);
            soft.draw(c, raster, state, screen_w() as f32 - PAD as f32 - sw, y);
            name.draw(c, raster, &name.fit(file, width - sw - 32.0), PAD as f32, y);
        }
    }
}

/// A parsed request head.
#[derive(Debug, PartialEq, Eq)]
enum Request {
    Page,
    Upload {
        name: String,
        len: u64,
        expect_continue: bool,
    },
    /// Refused: status line and a short reason for the page.
    Reject(&'static str, &'static str),
}

/// Reads the request head (up to the blank line); returns it and the body bytes read past it.
fn read_head(s: &mut TcpStream) -> io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok((String::from_utf8_lossy(&buf).into_owned(), rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "request head too long"));
        }
    }
}

fn parse(head: &str, token: &str) -> Request {
    const NOT_FOUND: Request = Request::Reject("404 Not Found", "not found");
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    let Some(rest) = target.strip_prefix('/').and_then(|t| t.strip_prefix(token)) else { return NOT_FOUND };
    if token.is_empty() {
        return NOT_FOUND;
    }
    match (method, rest) {
        ("GET", "" | "/") => Request::Page,
        ("PUT" | "POST", r) if r.starts_with("/upload?") => {
            let query = &r["/upload?".len()..];
            let raw = query.split('&').find_map(|kv| kv.strip_prefix("name=")).unwrap_or("");
            let decoded = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
            let Some(name) = safe_name(&decoded) else {
                return Request::Reject("400 Bad Request", "not a book file name");
            };
            let header = |key: &str| {
                head.lines().skip(1).find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim().eq_ignore_ascii_case(key).then(|| v.trim())
                })
            };
            let Some(len) = header("content-length").and_then(|v| v.parse::<u64>().ok()) else {
                return Request::Reject("411 Length Required", "length required");
            };
            if len > MAX_SIZE {
                return Request::Reject("413 Payload Too Large", "larger than 500 MB");
            }
            let expect_continue = header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue"));
            Request::Upload { name, len, expect_continue }
        }
        _ => NOT_FOUND,
    }
}

/// `raw` if it is a plain file name with a book extension: no directories, no `..`, no
/// hidden or control characters.
fn safe_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let bad = |c: char| c == '/' || c == '\\' || c.is_control();
    if name.is_empty() || name.len() > 200 || name.starts_with('.') || name.chars().any(bad) {
        return None;
    }
    // The userstore is FAT: titles like "Dune: Messiah" can't be saved as they are.
    let name: String =
        name.chars().map(|c| if matches!(c, ':' | '?' | '"' | '*' | '<' | '>' | '|') { '_' } else { c }).collect();
    Format::from_path(Path::new(&name)).is_some().then_some(name)
}

/// `dir/name`, or `dir/stem (2).ext` and so on when taken: never overwrite a book.
fn unique(dir: &Path, name: &str) -> Option<PathBuf> {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let numbered = (2..1000).map(|n| dir.join(format!("{stem} ({n}).{ext}")));
    std::iter::once(dir.join(name)).chain(numbered).find(|p| !p.exists())
}

fn respond(s: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\
         Cache-Control: no-store\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body)
}

/// One connection, one request.
fn handle(mut s: TcpStream, token: &str, dir: &Path, stop: &AtomicBool, tx: &Sender<Event>) -> io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    s.set_write_timeout(Some(Duration::from_secs(30)))?;
    let (head, early) = read_head(&mut s)?;
    let (name, len, expect_continue) = match parse(&head, token) {
        Request::Page => return respond(&mut s, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        Request::Reject(status, why) => return respond(&mut s, status, "text/plain; charset=utf-8", why.as_bytes()),
        Request::Upload { name, len, expect_continue } => (name, len, expect_continue),
    };
    if expect_continue {
        s.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    // Only now: from here every way out sends Done or Failed, so no row stays "receiving".
    let _ = tx.send(Event::Started(name.clone()));
    // A hidden partial file the library ignores, renamed only once complete.
    let part = dir.join(format!(".{name}.part"));
    let saved = receive(&mut s, early, len, &part, stop).and_then(|()| {
        let dest = unique(dir, &name).ok_or(io::ErrorKind::AlreadyExists)?;
        std::fs::rename(&part, &dest)?;
        Ok(dest)
    });
    match saved {
        Ok(dest) => {
            let file = dest.file_name().map_or(name, |f| f.to_string_lossy().into_owned());
            let _ = tx.send(Event::Done(file.clone()));
            respond(&mut s, "200 OK", "text/plain; charset=utf-8", file.as_bytes())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            let _ = tx.send(Event::Failed);
            let _ = respond(&mut s, "500 Internal Server Error", "text/plain; charset=utf-8", b"not saved");
            Err(e)
        }
    }
}

/// Streams `len` body bytes (`early` already read) into `path`, stopping if the screen closes.
fn receive(s: &mut TcpStream, mut early: Vec<u8>, len: u64, path: &Path, stop: &AtomicBool) -> io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    early.truncate(len.try_into().unwrap_or(usize::MAX));
    file.write_all(&early)?;
    let mut left = len - early.len() as u64;
    const CHUNK: usize = 64 << 10;
    let mut buf = vec![0u8; CHUNK];
    while left > 0 {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "screen closed"));
        }
        let n = s.read(&mut buf[..left.min(CHUNK as u64) as usize])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        file.write_all(&buf[..n])?;
        left -= n as u64;
    }
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(first: &str, headers: &str) -> String {
        format!("{first}\r\nHost: k\r\n{headers}\r\n")
    }

    #[test]
    fn only_the_token_opens_anything() {
        assert_eq!(parse(&head("GET /k7f3x2 HTTP/1.1", ""), "k7f3x2"), Request::Page);
        assert_eq!(parse(&head("GET /k7f3x2/ HTTP/1.1", ""), "k7f3x2"), Request::Page);
        for bad in ["GET / HTTP/1.1", "GET /k7f3x HTTP/1.1", "GET /k7f3x2z HTTP/1.1", "GET /../k7f3x2 HTTP/1.1", ""] {
            assert!(matches!(parse(&head(bad, ""), "k7f3x2"), Request::Reject("404 Not Found", _)), "{bad}");
        }
        assert!(matches!(parse(&head("GET / HTTP/1.1", ""), ""), Request::Reject("404 Not Found", _)));
    }

    #[test]
    fn uploads_need_a_book_name_and_a_length() {
        let up =
            |name: &str, headers: &str| parse(&head(&format!("PUT /tok/upload?name={name} HTTP/1.1"), headers), "tok");
        assert_eq!(
            up("Il%20nome%20della%20rosa.EPUB", "Content-Length: 12\r\nExpect: 100-continue\r\n"),
            Request::Upload { name: "Il nome della rosa.EPUB".into(), len: 12, expect_continue: true }
        );
        for bad in
            ["..%2Fetc%2Fpasswd.epub", "a%2Fb.epub", "a%5Cb.pdf", ".hidden.epub", "x.sh", "x%00.epub", "", "epub"]
        {
            assert!(matches!(up(bad, "Content-Length: 1\r\n"), Request::Reject("400 Bad Request", _)), "{bad}");
        }
        assert!(matches!(up("a.pdf", ""), Request::Reject("411 Length Required", _)));
        assert!(matches!(up("a.pdf", "content-length: 600000000\r\n"), Request::Reject("413 Payload Too Large", _)));
    }

    #[test]
    fn never_overwrites() {
        let dir = std::env::temp_dir().join(format!("zarathustra-transfer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert_eq!(unique(&dir, "a.epub"), Some(dir.join("a.epub")));
        std::fs::write(dir.join("a.epub"), b"x").expect("write");
        std::fs::write(dir.join("a (2).epub"), b"x").expect("write");
        assert_eq!(unique(&dir, "a.epub"), Some(dir.join("a (3).epub")));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn uploads_land_in_the_folder_over_loopback() {
        let dir = std::env::temp_dir().join(format!("zarathustra-upload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let server = Server::start(dir.clone(), "tok".into()).expect("server");
        let ask = |req: &[u8]| {
            let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port)).expect("connect");
            s.write_all(req).expect("send");
            let mut out = String::new();
            s.read_to_string(&mut out).expect("reply");
            out
        };
        assert!(ask(b"GET /nope HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
        assert!(ask(b"GET /tok HTTP/1.1\r\n\r\n").contains("<input id=\"files\""));
        let put = b"PUT /tok/upload?name=Libro%20uno.epub HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        assert!(ask(put).ends_with("Libro uno.epub"));
        assert!(ask(put).ends_with("Libro uno (2).epub"));
        assert_eq!(std::fs::read(dir.join("Libro uno (2).epub")).expect("saved"), b"hello");
        let names: Vec<_> =
            server.events.try_iter().filter_map(|e| if let Event::Done(n) = e { Some(n) } else { None }).collect();
        assert_eq!(names, ["Libro uno.epub", "Libro uno (2).epub"]);
        drop(server);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn tokens_are_six_typeable_characters() {
        let (a, b) = (token(), token());
        assert_eq!(a.len(), 6);
        assert!(a.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert_ne!(a, b);
    }
}
