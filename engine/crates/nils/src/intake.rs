// SPDX-License-Identifier: AGPL-3.0-only
//! The doors' intake: what stands between the listening socket and the
//! request handlers, and what the engine says of its own health.
//!
//! On 2026-10-02 the pre-production engine lost its port and lived on. The
//! slab door's parallel reads, 64 handlers and a desk warming the next items
//! took the process to its limit of 1,024 descriptors; an accept failed; the
//! HTTP library ended its accept loop on that error and dropped the listener;
//! the handler that received the error returned; the others waited for
//! requests that could no longer come, for an hour, and nothing was logged.
//! The accept loop now survives its errors (engine/vendor/tiny_http), and
//! this module keeps the rest from coming to that:
//!
//! - one thread takes every request from the socket and queues it; the
//!   handlers take from the queue. A picture (a tile, slab, render or thumb
//!   door, which reads tile files) waits in its client's own queue, and the
//!   handlers take pictures client by client in turn, each client at most
//!   its share of the handlers while others wait, and never all of them: a
//!   quarter is kept for the doors that are not pictures, so one reader's
//!   prefetch neither starves another reader nor a claim or an answer;
//! - the health door (`GET /api/health`, no token, numbers only) is answered
//!   by that thread, so it answers while every handler is busy, and says
//!   whether requests are being taken (`live`) and whether the engine has
//!   descriptors to spare (`ready`);
//! - under systemd's watchdog the engine asks its own health door over its
//!   own socket and feeds the watchdog only on a live answer, so a hung
//!   engine is restarted;
//! - the soft limit on open files is raised to the hard one at start;
//! - what goes wrong is said on stderr, which is the journal, at most once
//!   per ten seconds per kind.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// What a request is to the intake.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A door that reads tile files, from one client (a hash of its bearer,
    /// or of its address where it has none).
    Picture(u64),
    /// Any other door.
    Other,
}

/// The doors that read tile files: `GET /api/instances/{id}/tiles|slab|render|thumb/..`.
pub fn kind_of(method: &str, url: &str, bearer: Option<&str>, remote: Option<&str>) -> Kind {
    if method != "GET" {
        return Kind::Other;
    }
    let path = url.split_once('?').map_or(url, |(p, _)| p);
    let mut segs = path.trim_matches('/').split('/');
    let picture = segs.next() == Some("api")
        && segs.next() == Some("instances")
        && segs.next().is_some()
        && matches!(
            segs.next(),
            Some("tiles" | "slab" | "render" | "thumb" | "preview")
        );
    if !picture {
        return Kind::Other;
    }
    let mut h = DefaultHasher::new();
    match bearer {
        Some(b) => ("bearer", b).hash(&mut h),
        None => ("remote", remote.unwrap_or("")).hash(&mut h),
    }
    Kind::Picture(h.finish())
}

/// How many pictures one client may have waiting before the next is
/// answered 503 with a retry: more than a reader's page ever asks at once.
pub const PICTURES_WAITING_PER_CLIENT: usize = 512;

/// How long requests may wait with no handler taking or finishing any
/// before the engine says it is not live.
pub const STALLED_AFTER: Duration = Duration::from_secs(60);

/// A request the intake handed to a handler; the handler says it is done
/// by dropping it, a panic included.
pub struct Taken<'a, T> {
    pub job: Option<T>,
    picture: Option<u64>,
    intake: &'a Intake<T>,
}

impl<T> Drop for Taken<'_, T> {
    fn drop(&mut self) {
        self.intake.done(self.picture);
    }
}

/// The queue between the socket and the handlers.
pub struct Intake<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
    workers: usize,
    picture_slots: usize,
    waiting_cap: usize,
}

struct State<T> {
    other: VecDeque<T>,
    pictures: HashMap<u64, VecDeque<T>>,
    /// Clients with pictures waiting, in the order of their turns.
    turns: VecDeque<u64>,
    running: HashMap<u64, usize>,
    pictures_running: usize,
    busy: usize,
    waiting: usize,
    stopped: bool,
    /// The last time a handler took or finished a request.
    progress: Instant,
    /// Since when requests have been waiting, without a break.
    waiting_since: Option<Instant>,
    refused: u64,
}

/// The intake's numbers, for the health door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub workers: usize,
    pub busy: usize,
    pub waiting: usize,
    pub pictures_waiting: usize,
    pub pictures_running: usize,
    pub picture_slots: usize,
    pub clients: usize,
    pub refused: u64,
    /// Requests have waited this long while no handler took or finished one.
    pub stalled_for: Option<Duration>,
}

impl<T> Intake<T> {
    pub fn new(workers: usize) -> Self {
        Self::with_cap(workers, PICTURES_WAITING_PER_CLIENT)
    }

    pub fn with_cap(workers: usize, waiting_cap: usize) -> Self {
        let workers = workers.max(1);
        let picture_slots = workers.saturating_sub((workers / 4).max(1)).max(1);
        Intake {
            state: Mutex::new(State {
                other: VecDeque::new(),
                pictures: HashMap::new(),
                turns: VecDeque::new(),
                running: HashMap::new(),
                pictures_running: 0,
                busy: 0,
                waiting: 0,
                stopped: false,
                progress: Instant::now(),
                waiting_since: None,
                refused: 0,
            }),
            ready: Condvar::new(),
            workers,
            picture_slots,
            waiting_cap,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue a request; a picture whose client has too many waiting comes
    /// back, to be answered 503.
    pub fn push(&self, kind: Kind, job: T) -> Result<(), T> {
        let mut s = self.lock();
        match kind {
            Kind::Other => s.other.push_back(job),
            Kind::Picture(client) => {
                let queue = s.pictures.entry(client).or_default();
                if queue.len() >= self.waiting_cap {
                    s.refused += 1;
                    return Err(job);
                }
                queue.push_back(job);
                if queue.len() == 1 {
                    s.turns.push_back(client);
                }
            }
        }
        if s.waiting == 0 {
            s.waiting_since = Some(Instant::now());
        }
        s.waiting += 1;
        drop(s);
        self.ready.notify_one();
        Ok(())
    }

    /// The next request for a handler, waiting up to `wait`: any door that
    /// is not a picture first, then the pictures client by client. `None`
    /// when nothing came or the intake stopped.
    pub fn take(&self, wait: Duration) -> Option<Taken<'_, T>> {
        let until = Instant::now() + wait;
        let mut s = self.lock();
        loop {
            if s.stopped {
                return None;
            }
            if let Some(job) = s.other.pop_front() {
                return Some(self.taken(&mut s, job, None));
            }
            if s.pictures_running < self.picture_slots
                && let Some((client, job)) = self.next_picture(&mut s)
            {
                return Some(self.taken(&mut s, job, Some(client)));
            }
            let now = Instant::now();
            if now >= until {
                return None;
            }
            s = self
                .ready
                .wait_timeout(s, until - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn taken<'a>(&'a self, s: &mut State<T>, job: T, picture: Option<u64>) -> Taken<'a, T> {
        s.busy += 1;
        s.waiting -= 1;
        if s.waiting == 0 {
            s.waiting_since = None;
        }
        s.progress = Instant::now();
        if let Some(client) = picture {
            s.pictures_running += 1;
            *s.running.entry(client).or_default() += 1;
        }
        Taken {
            job: Some(job),
            picture,
            intake: self,
        }
    }

    /// The first client in turn that is under its share: the picture slots
    /// split evenly over the clients with pictures waiting or running. A
    /// client alone has them all.
    fn next_picture(&self, s: &mut State<T>) -> Option<(u64, T)> {
        let mut active: Vec<u64> = s.running.keys().copied().collect();
        active.extend(s.turns.iter().filter(|c| !s.running.contains_key(c)));
        let share = self.picture_slots.div_ceil(active.len().max(1));
        let at = s
            .turns
            .iter()
            .position(|c| s.running.get(c).copied().unwrap_or(0) < share)?;
        let client = s.turns.remove(at)?;
        let queue = s.pictures.get_mut(&client)?;
        let job = queue.pop_front()?;
        if queue.is_empty() {
            s.pictures.remove(&client);
        } else {
            s.turns.push_back(client);
        }
        Some((client, job))
    }

    fn done(&self, picture: Option<u64>) {
        let mut s = self.lock();
        s.busy = s.busy.saturating_sub(1);
        s.progress = Instant::now();
        if let Some(client) = picture {
            s.pictures_running = s.pictures_running.saturating_sub(1);
            if let Some(n) = s.running.get_mut(&client) {
                *n -= 1;
                if *n == 0 {
                    s.running.remove(&client);
                }
            }
        }
        drop(s);
        // a slot and a share came free: whoever waits may now be eligible
        self.ready.notify_all();
    }

    pub fn stop(&self) {
        self.lock().stopped = true;
        self.ready.notify_all();
    }

    pub fn stopped(&self) -> bool {
        self.lock().stopped
    }

    pub fn snapshot(&self) -> Snapshot {
        let s = self.lock();
        let stalled_for = s.waiting_since.and_then(|since| {
            let quiet = since.elapsed().min(s.progress.elapsed());
            (quiet >= STALLED_AFTER).then_some(quiet)
        });
        let clients: std::collections::HashSet<&u64> =
            s.running.keys().chain(s.turns.iter()).collect();
        Snapshot {
            workers: self.workers,
            busy: s.busy,
            waiting: s.waiting,
            pictures_waiting: s.pictures.values().map(VecDeque::len).sum(),
            pictures_running: s.pictures_running,
            picture_slots: self.picture_slots,
            clients: clients.len(),
            refused: s.refused,
            stalled_for,
        }
    }
}

/// The engine's counts of what went wrong, beside the intake's.
#[derive(Default)]
pub struct Trouble {
    pub accept_errors: AtomicU64,
    pub panics: AtomicU64,
    pub health_probes_failed: AtomicU64,
}

/// A line on stderr at most once per [`Said::EVERY`], saying how many were
/// not said in between: a storm of errors is one line every ten seconds in
/// the journal, never a flood, and never silence.
pub struct Said {
    last: Mutex<Option<Instant>>,
    quiet: AtomicUsize,
}

impl Default for Said {
    fn default() -> Self {
        Self::new()
    }
}

impl Said {
    pub const EVERY: Duration = Duration::from_secs(10);

    pub const fn new() -> Self {
        Said {
            last: Mutex::new(None),
            quiet: AtomicUsize::new(0),
        }
    }

    pub fn say(&self, line: impl FnOnce() -> String) {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|at| at.elapsed() < Self::EVERY) {
            self.quiet.fetch_add(1, Ordering::Relaxed);
            return;
        }
        *last = Some(Instant::now());
        drop(last);
        let quiet = self.quiet.swap(0, Ordering::Relaxed);
        let more = if quiet > 0 {
            format!(" ({quiet} more like it since the last line)")
        } else {
            String::new()
        };
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), "nils serve: {}{more}", line());
    }
}

/// The process's open descriptors, where the system says (Linux).
pub fn open_files() -> Option<usize> {
    std::fs::read_dir("/proc/self/fd").ok().map(|d| d.count())
}

/// The soft and hard limits on open files.
#[cfg(unix)]
#[allow(unsafe_code, reason = "getrlimit writes the struct it is given")]
#[allow(clippy::unnecessary_cast, reason = "rlim_t is not u64 on every target")]
pub fn file_limit() -> Option<(u64, u64)> {
    let mut l = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: a valid pointer to a rlimit
    (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut l) } == 0)
        .then_some((l.rlim_cur as u64, l.rlim_max as u64))
}

#[cfg(not(unix))]
pub fn file_limit() -> Option<(u64, u64)> {
    None
}

/// Raise the soft limit on open files to the hard one (at most 1,048,576),
/// as a server is expected to: the soft limit is 1,024 under systemd unless
/// the unit says otherwise, while the hard one is 524,288. Returns the soft
/// limit as it stands after.
#[cfg(unix)]
#[allow(unsafe_code, reason = "setrlimit reads the struct it is given")]
#[allow(clippy::unnecessary_cast, reason = "rlim_t is not u64 on every target")]
pub fn raise_file_limit() -> Option<u64> {
    let (soft, hard) = file_limit()?;
    let want = if hard == libc::RLIM_INFINITY as u64 {
        1 << 20
    } else {
        hard.min(1 << 20)
    };
    if soft < want {
        let l = libc::rlimit {
            rlim_cur: want as libc::rlim_t,
            rlim_max: hard as libc::rlim_t,
        };
        // SAFETY: a valid pointer to a rlimit
        let _ = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &l) };
    }
    file_limit().map(|(soft, _)| soft)
}

#[cfg(not(unix))]
pub fn raise_file_limit() -> Option<u64> {
    None
}

/// The descriptors an engine with this many handlers wants at least: a
/// registry connection and an ask reader each (a socket and, on Postgres,
/// the client's own event descriptors), the tile reads, connections and room.
pub fn files_wanted(workers: usize) -> u64 {
    (workers as u64) * 8 + crate::pyramid::TILE_READS.at_once() as u64 + 512
}

/// systemd's notification socket, where the engine runs under it.
#[derive(Clone)]
pub struct Notify {
    socket: std::path::PathBuf,
}

impl Notify {
    /// From `NOTIFY_SOCKET`; none outside systemd or where the unit gives
    /// none.
    pub fn from_env() -> Option<Notify> {
        let socket = std::env::var_os("NOTIFY_SOCKET")?;
        (!socket.is_empty()).then(|| Notify {
            socket: socket.into(),
        })
    }

    #[cfg(test)]
    pub fn at(socket: impl Into<std::path::PathBuf>) -> Notify {
        Notify {
            socket: socket.into(),
        }
    }

    /// Send one notification (`READY=1`, `WATCHDOG=1`, `STATUS=...`).
    #[cfg(unix)]
    pub fn send(&self, what: &str) -> std::io::Result<()> {
        use std::os::unix::ffi::OsStrExt as _;
        let sock = std::os::unix::net::UnixDatagram::unbound()?;
        let bytes = self.socket.as_os_str().as_bytes();
        if let Some(name) = bytes.strip_prefix(b"@") {
            #[cfg(target_os = "linux")]
            {
                use std::os::linux::net::SocketAddrExt as _;
                let addr = std::os::unix::net::SocketAddr::from_abstract_name(name)?;
                return sock.send_to_addr(what.as_bytes(), &addr).map(|_| ());
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = name;
                return Err(std::io::Error::other("an abstract socket is Linux's"));
            }
        }
        sock.send_to(what.as_bytes(), &self.socket).map(|_| ())
    }

    #[cfg(not(unix))]
    pub fn send(&self, _what: &str) -> std::io::Result<()> {
        Ok(())
    }
}

/// How often systemd's watchdog wants to hear from the engine, where the
/// unit sets `WatchdogSec` (`WATCHDOG_USEC`, for this process).
pub fn watchdog_every() -> Option<Duration> {
    if let Some(pid) = std::env::var("WATCHDOG_PID").ok()
        && pid.trim() != std::process::id().to_string()
    {
        return None;
    }
    let usec: u64 = std::env::var("WATCHDOG_USEC").ok()?.trim().parse().ok()?;
    (usec > 0).then(|| Duration::from_micros(usec))
}

/// Ask an engine's health door at `addr` over a fresh connection: whether it
/// answered within `within` and called itself live.
pub fn probe(addr: &str, within: Duration) -> Result<(), String> {
    use std::io::{Read as _, Write as _};
    let to: std::net::SocketAddr = addr.parse().map_err(|e| format!("{addr}: {e}"))?;
    let mut stream =
        std::net::TcpStream::connect_timeout(&to, within).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(within)).ok();
    stream.set_write_timeout(Some(within)).ok();
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("write: {e}"))?;
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&answer);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .ok_or("no answer")?;
    let doc: serde_json::Value =
        serde_json::from_str(body.trim()).map_err(|e| format!("not the health door: {e}"))?;
    if doc["live"] == true {
        Ok(())
    } else {
        Err(format!(
            "it says it is not live: {}",
            doc["why"].as_array().map_or(String::new(), |w| w
                .iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join("; "))
        ))
    }
}

/// The address the engine reaches itself at: its own, with an unspecified
/// host (`0.0.0.0`, `::`) made the loopback.
pub fn self_address(bound: &str) -> String {
    match bound.parse::<std::net::SocketAddr>() {
        Ok(mut a) if a.ip().is_unspecified() => {
            a.set_ip(if a.is_ipv4() {
                std::net::Ipv4Addr::LOCALHOST.into()
            } else {
                std::net::Ipv6Addr::LOCALHOST.into()
            });
            a.to_string()
        }
        _ => bound.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Kind = Kind::Picture(1);
    const B: Kind = Kind::Picture(2);

    fn fill(i: &Intake<&'static str>, kind: Kind, label: &'static str, n: usize) {
        for _ in 0..n {
            i.push(kind, label).unwrap();
        }
    }

    #[test]
    fn the_picture_doors_are_told_apart_by_their_client() {
        let a = kind_of("GET", "/api/instances/7/slab/0/0-32", Some("t1"), None);
        let b = kind_of("GET", "/api/instances/7/tiles/0/3?x=1", Some("t2"), None);
        assert!(matches!(a, Kind::Picture(_)) && matches!(b, Kind::Picture(_)));
        assert_ne!(a, b);
        assert_eq!(
            a,
            kind_of("GET", "/api/instances/9/render/1/2", Some("t1"), None)
        );
        assert_eq!(
            kind_of("GET", "/api/instances/7/manifest", Some("t1"), None),
            Kind::Other
        );
        assert_eq!(
            kind_of("POST", "/api/instances/7/slab/0/0-32", Some("t1"), None),
            Kind::Other
        );
        assert_eq!(
            kind_of("GET", "/api/campaigns", Some("t1"), None),
            Kind::Other
        );
        // without a bearer the address tells clients apart
        assert_ne!(
            kind_of("GET", "/api/instances/7/thumb", None, Some("10.0.0.1")),
            kind_of("GET", "/api/instances/7/thumb", None, Some("10.0.0.2"))
        );
    }

    #[test]
    fn a_flood_of_one_readers_pictures_leaves_the_other_reader_its_turns() {
        // 8 handlers: 6 picture slots, 2 kept for the other doors
        let i: Intake<&str> = Intake::new(8);
        fill(&i, A, "a", 200);
        let mut held = Vec::new();
        for _ in 0..6 {
            held.push(i.take(Duration::ZERO).unwrap());
        }
        // all six slots are reader a's while it is alone
        assert!(held.iter().all(|t| t.job == Some("a")));
        assert!(
            i.take(Duration::ZERO).is_none(),
            "the picture slots are full"
        );
        // reader b asks; the next free slot is b's, not a's 195th
        fill(&i, B, "b", 3);
        held.remove(0);
        assert_eq!(i.take(Duration::ZERO).unwrap().job, Some("b"));
        // a is over its share of three while b waits, so b takes each slot a frees
        held.remove(0);
        let b2 = i.take(Duration::ZERO).unwrap();
        assert_eq!(b2.job, Some("b"));
        held.push(b2);
        held.remove(0);
        let b3 = i.take(Duration::ZERO).unwrap();
        assert_eq!(b3.job, Some("b"));
        held.push(b3);
        let s = i.snapshot();
        assert_eq!(s.clients, 2);
        assert_eq!(s.pictures_waiting, 194);
        assert!(s.pictures_running <= 6);
    }

    #[test]
    fn the_other_doors_go_first_and_keep_handlers_of_their_own() {
        let i: Intake<&str> = Intake::new(4);
        fill(&i, A, "pic", 50);
        let pics: Vec<_> = (0..3).map(|_| i.take(Duration::ZERO).unwrap()).collect();
        assert!(pics.iter().all(|t| t.job == Some("pic")));
        // three of four handlers read pictures; the fourth stays for a claim
        assert!(i.take(Duration::ZERO).is_none());
        i.push(Kind::Other, "claim").unwrap();
        assert_eq!(i.take(Duration::ZERO).unwrap().job, Some("claim"));
    }

    #[test]
    fn a_client_with_too_many_waiting_is_turned_away_and_counted() {
        let i: Intake<u32> = Intake::with_cap(2, 3);
        for n in 0..3 {
            i.push(A, n).unwrap();
        }
        assert_eq!(i.push(A, 9), Err(9));
        assert!(i.push(B, 1).is_ok(), "another client is not");
        assert_eq!(i.snapshot().refused, 1);
    }

    #[test]
    fn a_handler_that_panics_still_gives_its_slot_back() {
        let i: std::sync::Arc<Intake<u32>> = std::sync::Arc::new(Intake::new(2));
        i.push(A, 1).unwrap();
        let j = std::sync::Arc::clone(&i);
        let r = std::thread::spawn(move || {
            let _t = j.take(Duration::ZERO).unwrap();
            panic!("a door panicked");
        })
        .join();
        assert!(r.is_err());
        let s = i.snapshot();
        assert_eq!((s.busy, s.pictures_running), (0, 0));
    }

    #[test]
    fn stopping_wakes_the_handlers() {
        let i: std::sync::Arc<Intake<u32>> = std::sync::Arc::new(Intake::new(2));
        let j = std::sync::Arc::clone(&i);
        let waiting = std::thread::spawn(move || j.take(Duration::from_secs(30)).is_none());
        std::thread::sleep(Duration::from_millis(50));
        i.stop();
        assert!(waiting.join().unwrap());
    }

    #[test]
    fn a_queue_nobody_takes_from_is_called_stalled_and_a_quiet_one_is_not() {
        let i: Intake<u32> = Intake::new(1);
        assert_eq!(i.snapshot().stalled_for, None);
        i.push(Kind::Other, 1).unwrap();
        // just pushed: waiting, not stalled
        assert_eq!(i.snapshot().stalled_for, None);
        {
            let mut s = i.lock();
            let long_ago = Instant::now() - STALLED_AFTER - Duration::from_secs(1);
            s.progress = long_ago;
            s.waiting_since = Some(long_ago);
        }
        assert!(i.snapshot().stalled_for.is_some());
        drop(i.take(Duration::ZERO));
        assert_eq!(i.snapshot().stalled_for, None);
    }

    #[cfg(unix)]
    #[test]
    fn the_notification_reaches_the_socket() {
        let dir = nils_dicom::synth::TempDir::new("intake-notify");
        let path = dir.path().join("notify");
        let sock = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        Notify::at(&path).send("WATCHDOG=1").unwrap();
        let mut buf = [0u8; 64];
        let n = sock.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"WATCHDOG=1");
    }

    #[test]
    fn the_engine_reaches_itself_on_the_loopback() {
        assert_eq!(self_address("0.0.0.0:8437"), "127.0.0.1:8437");
        assert_eq!(self_address("[::]:8437"), "[::1]:8437");
        assert_eq!(self_address("192.0.2.10:8437"), "192.0.2.10:8437");
    }

    #[test]
    fn the_soft_file_limit_is_raised_to_the_hard_one() {
        let Some((_, hard)) = file_limit() else {
            return;
        };
        let soft = raise_file_limit().unwrap();
        assert!(soft == hard || soft == 1 << 20, "{soft} {hard}");
    }
}
