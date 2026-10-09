//! Forwards the freedesktop Secret Service (`org.freedesktop.secrets`) from the host session
//! bus into termway's isolated bus.
//!
//! Clients such as VS Code / Electron and Chromium keep their encryption keys in the OS
//! keyring via libsecret. The isolated bus has no keyring, and activating one there hangs
//! because gnome-keyring-daemon detects the host instance through its control socket. The
//! bridge owns the name on the isolated bus and relays calls, replies and signals to the
//! host keyring, so only this one service crosses the isolation boundary.
//!
//! Only the small subset of the D-Bus wire protocol needed for relaying is implemented,
//! keeping termway free of extra dependencies.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

pub const SECRET_SERVICE_NAME: &str = "org.freedesktop.secrets";

const BUS_NAME: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

pub const METHOD_CALL: u8 = 1;
pub const METHOD_RETURN: u8 = 2;
pub const ERROR: u8 = 3;
pub const SIGNAL: u8 = 4;

pub const FLAG_NO_REPLY_EXPECTED: u8 = 0x1;

const FIELD_PATH: u8 = 1;
const FIELD_INTERFACE: u8 = 2;
const FIELD_MEMBER: u8 = 3;
const FIELD_ERROR_NAME: u8 = 4;
const FIELD_REPLY_SERIAL: u8 = 5;
const FIELD_DESTINATION: u8 = 6;
const FIELD_SENDER: u8 = 7;
const FIELD_SIGNATURE: u8 = 8;
const FIELD_UNIX_FDS: u8 = 9;

/// Upper bound from the D-Bus specification (128 MiB)
const MAX_MESSAGE_SIZE: usize = 128 * 1024 * 1024;

/// A D-Bus message with its body kept as opaque, already-marshalled bytes.
///
/// The body is only valid in the byte order it was marshalled with, so relayed messages
/// keep `big_endian` from the original and only the header is re-encoded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub big_endian: bool,
    pub msg_type: u8,
    pub flags: u8,
    pub serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error_name: Option<String>,
    pub reply_serial: Option<u32>,
    pub destination: Option<String>,
    pub sender: Option<String>,
    pub signature: Option<String>,
    pub unix_fds: Option<u32>,
    pub body: Vec<u8>,
}

impl Message {
    pub fn method_call(destination: &str, path: &str, interface: &str, member: &str) -> Self {
        Self {
            msg_type: METHOD_CALL,
            path: Some(path.into()),
            interface: Some(interface.into()),
            member: Some(member.into()),
            destination: Some(destination.into()),
            ..Default::default()
        }
    }

    /// Set a body consisting of string (`s`) and uint32 (`u`) arguments
    pub fn with_args(mut self, args: &[Arg]) -> Self {
        let mut w = Writer::new(self.big_endian);
        let mut sig = String::new();
        for arg in args {
            match arg {
                Arg::Str(s) => {
                    sig.push('s');
                    w.string(s);
                }
                Arg::U32(v) => {
                    sig.push('u');
                    w.u32(*v);
                }
            }
        }
        self.signature = Some(sig);
        self.body = w.buf;
        self
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(self.big_endian);
        w.u8(if self.big_endian { b'B' } else { b'l' });
        w.u8(self.msg_type);
        w.u8(self.flags);
        w.u8(1);
        w.u32(self.body.len() as u32);
        w.u32(self.serial);

        w.u32(0); // header field array length, patched below
        let len_pos = w.buf.len() - 4;
        w.pad(8);
        let start = w.buf.len();
        let str_fields = [
            (FIELD_PATH, "o", &self.path),
            (FIELD_INTERFACE, "s", &self.interface),
            (FIELD_MEMBER, "s", &self.member),
            (FIELD_ERROR_NAME, "s", &self.error_name),
            (FIELD_DESTINATION, "s", &self.destination),
            (FIELD_SENDER, "s", &self.sender),
        ];
        for (code, sig, value) in str_fields {
            if let Some(v) = value {
                w.pad(8);
                w.u8(code);
                w.signature(sig);
                w.string(v);
            }
        }
        for (code, value) in [(FIELD_REPLY_SERIAL, self.reply_serial), (FIELD_UNIX_FDS, self.unix_fds)] {
            if let Some(v) = value {
                w.pad(8);
                w.u8(code);
                w.signature("u");
                w.u32(v);
            }
        }
        if let Some(sig) = &self.signature {
            w.pad(8);
            w.u8(FIELD_SIGNATURE);
            w.signature("g");
            w.signature(sig);
        }
        let fields_len = (w.buf.len() - start) as u32;
        let len_bytes = w.encode_u32(fields_len);
        w.buf[len_pos..len_pos + 4].copy_from_slice(&len_bytes);

        w.pad(8);
        w.buf.extend_from_slice(&self.body);
        w.buf
    }

    pub fn read_from(r: &mut impl Read) -> io::Result<Self> {
        let mut fixed = [0u8; 16];
        r.read_exact(&mut fixed)?;
        let big_endian = match fixed[0] {
            b'l' => false,
            b'B' => true,
            _ => return Err(invalid("bad endianness marker")),
        };
        let mut head = Reader { buf: &fixed, pos: 4, big_endian };
        let body_len = head.u32()? as usize;
        let _serial = head.u32()?;
        let fields_len = head.u32()? as usize;

        let header_len = (16 + fields_len + 7) & !7;
        let total = header_len
            .checked_add(body_len)
            .filter(|&t| t <= MAX_MESSAGE_SIZE)
            .ok_or_else(|| invalid("message too large"))?;
        let mut buf = vec![0u8; total];
        buf[..16].copy_from_slice(&fixed);
        r.read_exact(&mut buf[16..])?;
        Self::decode(&buf)
    }

    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        if buf.len() < 16 {
            return Err(invalid("truncated header"));
        }
        let big_endian = match buf[0] {
            b'l' => false,
            b'B' => true,
            _ => return Err(invalid("bad endianness marker")),
        };
        let mut r = Reader { buf, pos: 1, big_endian };
        let mut msg = Message {
            big_endian,
            msg_type: r.u8()?,
            flags: r.u8()?,
            ..Default::default()
        };
        let _version = r.u8()?;
        let body_len = r.u32()? as usize;
        msg.serial = r.u32()?;
        let fields_len = r.u32()? as usize;

        r.align(8);
        let fields_end = r.pos + fields_len;
        while r.pos < fields_end {
            r.align(8);
            let code = r.u8()?;
            let sig = r.signature()?;
            match (code, sig.as_str()) {
                (FIELD_PATH, "o") => msg.path = Some(r.string()?),
                (FIELD_INTERFACE, "s") => msg.interface = Some(r.string()?),
                (FIELD_MEMBER, "s") => msg.member = Some(r.string()?),
                (FIELD_ERROR_NAME, "s") => msg.error_name = Some(r.string()?),
                (FIELD_DESTINATION, "s") => msg.destination = Some(r.string()?),
                (FIELD_SENDER, "s") => msg.sender = Some(r.string()?),
                (FIELD_REPLY_SERIAL, "u") => msg.reply_serial = Some(r.u32()?),
                (FIELD_UNIX_FDS, "u") => msg.unix_fds = Some(r.u32()?),
                (FIELD_SIGNATURE, "g") => msg.signature = Some(r.signature()?),
                // Unknown fields must be ignored; skip the basic types they can use
                (_, "s") | (_, "o") => {
                    r.string()?;
                }
                (_, "g") => {
                    r.signature()?;
                }
                (_, "u") | (_, "i") => {
                    r.u32()?;
                }
                (_, "y") => {
                    r.u8()?;
                }
                _ => return Err(invalid("unsupported header field type")),
            }
        }

        r.pos = fields_end;
        r.align(8);
        let body_end = r.pos + body_len;
        if body_end > buf.len() {
            return Err(invalid("truncated body"));
        }
        msg.body = buf[r.pos..body_end].to_vec();
        Ok(msg)
    }

    /// Parse a body whose signature is a single string (`s`)
    pub fn body_string(&self) -> io::Result<String> {
        if self.signature.as_deref() != Some("s") {
            return Err(invalid("expected string body"));
        }
        Reader { buf: &self.body, pos: 0, big_endian: self.big_endian }.string()
    }

    /// Parse a body whose signature is a single uint32 (`u`)
    pub fn body_u32(&self) -> io::Result<u32> {
        if self.signature.as_deref() != Some("u") {
            return Err(invalid("expected uint32 body"));
        }
        Reader { buf: &self.body, pos: 0, big_endian: self.big_endian }.u32()
    }

    /// Parse a body whose signature is a string array (`as`)
    pub fn body_string_array(&self) -> io::Result<Vec<String>> {
        if self.signature.as_deref() != Some("as") {
            return Err(invalid("expected string array body"));
        }
        let mut r = Reader { buf: &self.body, pos: 0, big_endian: self.big_endian };
        let len = r.u32()? as usize;
        r.align(4);
        let end = r.pos + len;
        let mut out = Vec::new();
        while r.pos < end {
            out.push(r.string()?);
        }
        Ok(out)
    }
}

pub enum Arg<'a> {
    Str(&'a str),
    U32(u32),
}

struct Writer {
    buf: Vec<u8>,
    big_endian: bool,
}

impl Writer {
    fn new(big_endian: bool) -> Self {
        Self { buf: Vec::new(), big_endian }
    }

    #[allow(clippy::manual_is_multiple_of)]
    fn pad(&mut self, align: usize) {
        while self.buf.len() % align != 0 {
            self.buf.push(0);
        }
    }

    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn encode_u32(&self, v: u32) -> [u8; 4] {
        if self.big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    }

    fn u32(&mut self, v: u32) {
        self.pad(4);
        let bytes = self.encode_u32(v);
        self.buf.extend_from_slice(&bytes);
    }

    fn string(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    fn signature(&mut self, s: &str) {
        self.buf.push(s.len() as u8);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    big_endian: bool,
}

impl Reader<'_> {
    #[allow(clippy::manual_div_ceil)]
    fn align(&mut self, align: usize) {
        self.pos = (self.pos + align - 1) / align * align;
    }

    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len());
        let end = end.ok_or_else(|| invalid("truncated message"))?;
        let bytes = &self.buf[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> io::Result<u32> {
        self.align(4);
        let b: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(if self.big_endian { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
    }

    fn string(&mut self) -> io::Result<String> {
        let len = self.u32()? as usize;
        let s = String::from_utf8(self.take(len)?.to_vec()).map_err(|_| invalid("invalid UTF-8"))?;
        self.take(1)?; // nul terminator
        Ok(s)
    }

    fn signature(&mut self) -> io::Result<String> {
        let len = self.u8()? as usize;
        let s = String::from_utf8(self.take(len)?.to_vec()).map_err(|_| invalid("invalid UTF-8"))?;
        self.take(1)?;
        Ok(s)
    }
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Decode `%XX` escapes used in D-Bus address values
fn unescape_address_value(v: &str) -> String {
    let bytes = v.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn connect_address(address: &str) -> io::Result<UnixStream> {
    let mut last_err = io::Error::new(io::ErrorKind::NotFound, "no usable unix transport in address");
    for entry in address.split(';').filter(|e| !e.is_empty()) {
        let Some(params) = entry.strip_prefix("unix:") else {
            continue;
        };
        for kv in params.split(',') {
            let Some((key, value)) = kv.split_once('=') else {
                continue;
            };
            let value = unescape_address_value(value);
            let result = match key {
                "path" => UnixStream::connect(&value),
                "abstract" => {
                    use std::os::linux::net::SocketAddrExt;
                    std::os::unix::net::SocketAddr::from_abstract_name(value.as_bytes())
                        .and_then(|addr| UnixStream::connect_addr(&addr))
                }
                _ => continue,
            };
            match result {
                Ok(stream) => return Ok(stream),
                Err(e) => last_err = e,
            }
        }
    }
    Err(last_err)
}

/// A blocking connection to a message bus, used for setup calls before relaying starts.
pub struct Connection {
    stream: UnixStream,
    unique_name: String,
    next_serial: u32,
}

impl Connection {
    pub fn connect(address: &str) -> io::Result<Self> {
        let mut stream = connect_address(address)?;
        Self::authenticate(&mut stream)?;
        let mut conn = Self {
            stream,
            unique_name: String::new(),
            next_serial: 1,
        };
        let reply = conn.call(Message::method_call(BUS_NAME, BUS_PATH, BUS_NAME, "Hello"))?;
        conn.unique_name = reply.body_string()?;
        Ok(conn)
    }

    fn authenticate(stream: &mut UnixStream) -> io::Result<()> {
        let uid = unsafe { libc::getuid() }.to_string();
        let hex: String = uid.bytes().map(|b| format!("{:02x}", b)).collect();
        stream.write_all(format!("\0AUTH EXTERNAL {}\r\n", hex).as_bytes())?;

        // Read byte-wise so nothing past the line is consumed from the binary stream
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while !line.ends_with(b"\r\n") {
            stream.read_exact(&mut byte)?;
            line.push(byte[0]);
            if line.len() > 4096 {
                return Err(invalid("authentication reply too long"));
            }
        }
        if !line.starts_with(b"OK ") {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("D-Bus authentication rejected: {}", String::from_utf8_lossy(&line).trim()),
            ));
        }
        stream.write_all(b"BEGIN\r\n")
    }

    pub fn unique_name(&self) -> &str {
        &self.unique_name
    }

    pub fn send(&mut self, mut msg: Message) -> io::Result<u32> {
        msg.serial = self.next_serial;
        self.next_serial += 1;
        self.stream.write_all(&msg.encode())?;
        Ok(msg.serial)
    }

    pub fn read(&mut self) -> io::Result<Message> {
        Message::read_from(&mut self.stream)
    }

    /// Send a method call and wait for its reply, discarding unrelated messages meanwhile
    pub fn call(&mut self, msg: Message) -> io::Result<Message> {
        let serial = self.send(msg)?;
        loop {
            let reply = self.read()?;
            if reply.reply_serial != Some(serial) {
                continue;
            }
            return match reply.msg_type {
                METHOD_RETURN => Ok(reply),
                _ => Err(io::Error::other(format!(
                    "{}: {}",
                    reply.error_name.as_deref().unwrap_or("D-Bus error"),
                    reply.body_string().unwrap_or_default()
                ))),
            };
        }
    }

    fn bus_call(&mut self, member: &str, args: &[Arg]) -> io::Result<Message> {
        self.call(Message::method_call(BUS_NAME, BUS_PATH, BUS_NAME, member).with_args(args))
    }

    pub fn request_name(&mut self, name: &str) -> io::Result<()> {
        const DO_NOT_QUEUE: u32 = 4;
        const PRIMARY_OWNER: u32 = 1;
        let reply = self.bus_call("RequestName", &[Arg::Str(name), Arg::U32(DO_NOT_QUEUE)])?;
        match reply.body_u32()? {
            PRIMARY_OWNER => Ok(()),
            code => Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("could not own {} (RequestName returned {})", name, code),
            )),
        }
    }

    /// Whether the bus has an owner for `name` or can activate one
    pub fn name_available(&mut self, name: &str) -> io::Result<bool> {
        for member in ["ListNames", "ListActivatableNames"] {
            if self.bus_call(member, &[])?.body_string_array()?.iter().any(|n| n == name) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn add_match(&mut self, rule: &str) -> io::Result<()> {
        self.bus_call("AddMatch", &[Arg::Str(rule)]).map(|_| ())
    }

    fn into_relay(self) -> io::Result<(UnixStream, Arc<RelaySender>)> {
        let writer = self.stream.try_clone()?;
        Ok((
            self.stream,
            Arc::new(RelaySender {
                inner: Mutex::new((writer, self.next_serial)),
            }),
        ))
    }
}

/// Write half of a relayed connection; serial allocation and the write happen under one lock
struct RelaySender {
    inner: Mutex<(UnixStream, u32)>,
}

impl RelaySender {
    fn send(&self, mut msg: Message, before_write: impl FnOnce(u32)) -> io::Result<()> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (stream, next_serial) = &mut *guard;
        msg.serial = *next_serial;
        *next_serial = next_serial.wrapping_add(1).max(1);
        before_write(msg.serial);
        stream.write_all(&msg.encode())
    }
}

/// Host serial -> (client unique name on the isolated bus, client serial)
type PendingReplies = Arc<Mutex<HashMap<u32, (String, u32)>>>;

/// Relays one well-known service from a host bus into an isolated bus.
pub struct ServiceBridge {
    streams: Vec<UnixStream>,
}

impl ServiceBridge {
    /// Bridge `org.freedesktop.secrets` if the host bus can provide it
    pub fn start_secret_service(isolated_address: &str, host_address: &str) -> io::Result<Self> {
        Self::start(SECRET_SERVICE_NAME, isolated_address, host_address)
    }

    pub fn start(name: &str, isolated_address: &str, host_address: &str) -> io::Result<Self> {
        let mut host = Connection::connect(host_address)?;
        if !host.name_available(name)? {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not provided on the host bus", name),
            ));
        }
        host.add_match(&format!("type='signal',sender='{}'", name))?;

        let mut isolated = Connection::connect(isolated_address)?;
        isolated.request_name(name)?;

        let (host_reader, host_sender) = host.into_relay()?;
        let (isolated_reader, isolated_sender) = isolated.into_relay()?;
        let streams = vec![host_reader.try_clone()?, isolated_reader.try_clone()?];
        let pending: PendingReplies = Arc::new(Mutex::new(HashMap::new()));

        {
            let name = name.to_string();
            let host_sender = host_sender.clone();
            let isolated_sender = isolated_sender.clone();
            let pending = pending.clone();
            std::thread::Builder::new()
                .name("termway-dbus-bridge-in".into())
                .spawn(move || relay_calls(isolated_reader, &name, &host_sender, &isolated_sender, &pending))?;
        }
        std::thread::Builder::new()
            .name("termway-dbus-bridge-out".into())
            .spawn(move || relay_replies_and_signals(host_reader, &isolated_sender, &pending))?;

        Ok(Self { streams })
    }
}

impl Drop for ServiceBridge {
    fn drop(&mut self) {
        // Unblocks both relay threads, which then exit on the read error
        for s in &self.streams {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn error_reply(to: &Message, error_name: &str, text: &str) -> Message {
    Message {
        msg_type: ERROR,
        flags: FLAG_NO_REPLY_EXPECTED,
        error_name: Some(error_name.into()),
        reply_serial: Some(to.serial),
        destination: to.sender.clone(),
        ..Default::default()
    }
    .with_args(&[Arg::Str(text)])
}

/// Isolated bus -> host: forward method calls addressed to the bridged name
fn relay_calls(
    mut reader: UnixStream,
    name: &str,
    host: &RelaySender,
    isolated: &RelaySender,
    pending: &PendingReplies,
) {
    while let Ok(msg) = Message::read_from(&mut reader) {
        if msg.msg_type != METHOD_CALL {
            continue; // NameAcquired and other bus signals
        }
        let Some(client) = msg.sender.clone() else {
            continue;
        };
        if msg.unix_fds.is_some() {
            if msg.flags & FLAG_NO_REPLY_EXPECTED == 0 {
                let err = error_reply(&msg, "org.freedesktop.DBus.Error.NotSupported", "file descriptors cannot be relayed");
                let _ = isolated.send(err, |_| {});
            }
            continue;
        }
        let expects_reply = msg.flags & FLAG_NO_REPLY_EXPECTED == 0;
        let forward = Message {
            big_endian: msg.big_endian,
            msg_type: METHOD_CALL,
            flags: msg.flags,
            path: msg.path.clone(),
            interface: msg.interface.clone(),
            member: msg.member.clone(),
            destination: Some(name.to_string()),
            signature: msg.signature.clone(),
            body: msg.body.clone(),
            ..Default::default()
        };
        let result = host.send(forward, |serial| {
            if expects_reply {
                pending.lock().unwrap_or_else(|e| e.into_inner()).insert(serial, (client, msg.serial));
            }
        });
        if result.is_err() {
            if expects_reply {
                let err = error_reply(&msg, "org.freedesktop.DBus.Error.Disconnected", "host session bus unavailable");
                let _ = isolated.send(err, |_| {});
            }
            break;
        }
    }
}

/// Host -> isolated bus: route replies back to their caller and rebroadcast service signals
fn relay_replies_and_signals(mut reader: UnixStream, isolated: &RelaySender, pending: &PendingReplies) {
    while let Ok(msg) = Message::read_from(&mut reader) {
        let relayed = match msg.msg_type {
            METHOD_RETURN | ERROR => {
                let Some(serial) = msg.reply_serial else { continue };
                let Some((client, client_serial)) =
                    pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&serial)
                else {
                    continue;
                };
                Message {
                    big_endian: msg.big_endian,
                    msg_type: msg.msg_type,
                    flags: FLAG_NO_REPLY_EXPECTED,
                    error_name: msg.error_name,
                    reply_serial: Some(client_serial),
                    destination: Some(client),
                    signature: msg.signature,
                    body: msg.body,
                    ..Default::default()
                }
            }
            // The match rule limits signals to the bridged service; drop the bus's own
            SIGNAL if msg.sender.as_deref() != Some(BUS_NAME) => Message {
                big_endian: msg.big_endian,
                msg_type: SIGNAL,
                flags: FLAG_NO_REPLY_EXPECTED,
                path: msg.path,
                interface: msg.interface,
                member: msg.member,
                signature: msg.signature,
                body: msg.body,
                ..Default::default()
            },
            _ => continue,
        };
        if isolated.send(relayed, |_| {}).is_err() {
            break;
        }
    }
}
