//! A tiny MySQL-protocol server, enough for `mysql_async` to connect and run a
//! handful of statements.
//!
//! Why it exists: the admin closures only run after a **successful**
//! connection, so without a live peer every one of them is uncovered and the
//! loopback transport is never truly dialled. This fake speaks just the
//! protocol the driver needs — handshake v10, `COM_QUERY` with a text result
//! set, `COM_STMT_PREPARE`/`COM_STMT_EXECUTE` with a binary result set, `OK` and
//! `ERR` — on an ephemeral loopback port, so a test can run the production
//! `Opts` (no socket, `prefer_socket(false)`, `127.0.0.1:<port>`) against it.
//!
//! It is deliberately minimal and never shipped: it lives behind `#[cfg(test)]`
//! and claims nothing about MySQL's real behaviour beyond what a test asserts
//! (a statement's text, an error code, a value read back).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Column type the fake reports for integer columns.
const TYPE_LONGLONG: u8 = 8;
/// Column type the fake reports for string columns.
const TYPE_VAR_STRING: u8 = 253;
/// SERVER_STATUS_AUTOCOMMIT.
const STATUS_AUTOCOMMIT: u16 = 0x0002;
/// Capabilities advertised: LONG_PASSWORD, PROTOCOL_41, TRANSACTIONS,
/// SECURE_CONNECTION, PLUGIN_AUTH, PLUGIN_AUTH_LENENC_CLIENT_DATA.
const CAPABILITIES: u32 =
    0x0000_0001 | 0x0000_0200 | 0x0000_2000 | 0x0000_8000 | 0x0008_0000 | 0x0020_0000;

/// One result-set column.
#[derive(Debug, Clone)]
pub struct Col {
    /// Column name.
    pub name: &'static str,
    /// Whether the values are integers (`LONGLONG`) or text (`VAR_STRING`).
    pub numeric: bool,
}

impl Col {
    /// An integer column.
    pub fn int(name: &'static str) -> Self {
        Self {
            name,
            numeric: true,
        }
    }

    /// A text column.
    pub fn text(name: &'static str) -> Self {
        Self {
            name,
            numeric: false,
        }
    }
}

/// What the fake answers for one statement.
#[derive(Debug, Clone)]
pub enum Reply {
    /// An empty `OK`.
    Ok,
    /// A text result set.
    Rows {
        /// Columns.
        columns: Vec<Col>,
        /// Rows, one `Option<String>` per column.
        rows: Vec<Vec<Option<String>>>,
    },
    /// An `ERR` with a MySQL code.
    Err {
        /// The MySQL error code (e.g. `1396`).
        code: u16,
        /// The message.
        message: &'static str,
    },
}

/// Convenience: a one-row, one-column integer result.
pub fn count(value: i64) -> Reply {
    Reply::Rows {
        columns: vec![Col::int("count")],
        rows: vec![vec![Some(value.to_string())]],
    }
}

/// A running fake server. Dropping it stops the accept loop.
pub struct FakeServer {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
    shutdown: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl FakeServer {
    /// Binds an ephemeral loopback port and answers with `reply`.
    pub fn start(reply: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        listener.set_nonblocking(true).expect("nonblocking");
        let log = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let reply = Arc::new(reply);
        let join = {
            let log = Arc::clone(&log);
            let shutdown = Arc::clone(&shutdown);
            thread::spawn(move || {
                while !shutdown.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let log = Arc::clone(&log);
                            let reply = Arc::clone(&reply);
                            thread::spawn(move || serve(stream, &*reply, &log));
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Self {
            port,
            log,
            shutdown,
            join: Some(join),
        }
    }

    /// The port the fake listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Every statement the fake received, in order.
    pub fn statements(&self) -> Vec<String> {
        self.log.lock().map(|log| log.clone()).unwrap_or_default()
    }

    /// Whether any received statement contains `needle`.
    pub fn saw(&self, needle: &str) -> bool {
        self.statements().iter().any(|sql| sql.contains(needle))
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Unblock a pending accept so the thread notices the flag promptly.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    reply: &(dyn Fn(&str) -> Reply + Send + Sync),
    log: &Mutex<Vec<String>>,
) {
    let _ = stream.set_nodelay(true);
    if write_packet(&mut stream, 0, &handshake()).is_err() {
        return;
    }
    // Handshake response: contents do not matter, only that it exists.
    if read_packet(&mut stream).is_err() {
        return;
    }
    // The connect-time `auth_ok`. A test can refuse the login by answering with
    // an `ERR` instead, which is how the driver's connect-error mapping is
    // reached. The handshake response was sequence 1, so this reply is 2.
    let ok = match reply("@@connect") {
        Reply::Err { code, message } => {
            let _ = write_packet(&mut stream, 2, &err_packet(code, message));
            return;
        }
        Reply::Ok | Reply::Rows { .. } => ok_packet(),
    };
    if write_packet(&mut stream, 2, &ok).is_err() {
        return;
    }
    let mut prepared: HashMap<u32, String> = HashMap::new();
    let mut next_id: u32 = 1;
    loop {
        let (_, payload) = match read_packet(&mut stream) {
            Ok(packet) => packet,
            Err(_) => return,
        };
        let Some((&command, rest)) = payload.split_first() else {
            return;
        };
        match command {
            // COM_QUIT
            0x01 => return,
            // COM_PING: a failed write means the client went away.
            0x0e => {
                let _ = write_packet(&mut stream, 1, &ok_packet());
            }
            // COM_QUERY
            0x03 => {
                let sql = String::from_utf8_lossy(rest).into_owned();
                record(log, &sql);
                if respond(&mut stream, server_settings(&sql, reply(&sql)), false).is_err() {
                    return;
                }
            }
            // COM_STMT_PREPARE
            0x16 => {
                let sql = String::from_utf8_lossy(rest).into_owned();
                record(log, &sql);
                let id = next_id;
                next_id += 1;
                prepared.insert(id, sql.clone());
                let params = sql.matches('?').count() as u16;
                if write_packet(&mut stream, 1, &prepare_ok(id, params)).is_err() {
                    return;
                }
                for index in 0..params {
                    let col = Col::text("param");
                    if write_packet(&mut stream, 2 + index as u8, &column_def(&col)).is_err() {
                        return;
                    }
                }
                if params > 0 && write_packet(&mut stream, 2 + params as u8, &eof_packet()).is_err()
                {
                    return;
                }
            }
            // COM_STMT_EXECUTE: binary result set
            0x17 => {
                let id = if rest.len() >= 4 {
                    u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]])
                } else {
                    0
                };
                let sql = prepared.get(&id).cloned().unwrap_or_default();
                if respond(&mut stream, reply(&sql), true).is_err() {
                    return;
                }
            }
            // COM_STMT_CLOSE and anything else: no response.
            _ => {}
        }
    }
}

/// Answers the driver's connect-time `SELECT @@…` setting query (text protocol)
/// before the test's own reply; every other statement goes to the test.
fn server_settings(sql: &str, reply: Reply) -> Reply {
    if !sql.trim_start().starts_with("SELECT @@") {
        return reply;
    }
    if sql.contains("@@max_allowed_packet") && sql.contains("@@wait_timeout") {
        return Reply::Rows {
            columns: vec![Col::int("@@max_allowed_packet"), Col::int("@@wait_timeout")],
            rows: vec![vec![Some("16777216".to_owned()), Some("28800".to_owned())]],
        };
    }
    // `@@socket` only; the loopback arm has prefer_socket off and never asks.
    Reply::Rows {
        columns: vec![Col::text("@@socket")],
        rows: vec![vec![None]],
    }
}

fn record(log: &Mutex<Vec<String>>, sql: &str) {
    if let Ok(mut log) = log.lock() {
        log.push(sql.to_owned());
    }
}

/// Writes `reply`; `binary` selects the binary row format (`COM_STMT_EXECUTE`).
fn respond(stream: &mut TcpStream, reply: Reply, binary: bool) -> std::io::Result<()> {
    match reply {
        Reply::Ok => write_packet(stream, 1, &ok_packet()),
        Reply::Err { code, message } => write_packet(stream, 1, &err_packet(code, message)),
        Reply::Rows { columns, rows } => {
            write_packet(stream, 1, &lenenc_int(columns.len() as u64))?;
            for (index, col) in columns.iter().enumerate() {
                write_packet(stream, 2 + index as u8, &column_def(col))?;
            }
            write_packet(stream, 2 + columns.len() as u8, &eof_packet())?;
            let mut seq = 3 + columns.len() as u8;
            for row in rows {
                let payload = if binary {
                    binary_row(&columns, &row)
                } else {
                    text_row(&row)
                };
                write_packet(stream, seq, &payload)?;
                seq += 1;
            }
            write_packet(stream, seq, &eof_packet())
        }
    }
}

fn text_row(row: &[Option<String>]) -> Vec<u8> {
    let mut payload = Vec::new();
    for value in row {
        match value {
            Some(text) => payload.extend(lenenc_str(text)),
            None => payload.push(0xfb),
        }
    }
    payload
}

fn binary_row(columns: &[Col], row: &[Option<String>]) -> Vec<u8> {
    let mut payload = vec![0x00];
    // Null bitmap: 2 reserved bits, then one bit per column.
    let bitmap_len = (columns.len() + 7 + 2) / 8;
    let mut bitmap = vec![0u8; bitmap_len];
    for (index, value) in row.iter().enumerate() {
        if value.is_none() {
            let bit = index + 2;
            bitmap[bit / 8] |= 1 << (bit % 8);
        }
    }
    payload.extend(bitmap);
    for (col, value) in columns.iter().zip(row) {
        let Some(text) = value else {
            continue;
        };
        if col.numeric {
            let number: i64 = text.parse().unwrap_or(0);
            payload.extend(number.to_le_bytes());
        } else {
            payload.extend(lenenc_str(text));
        }
    }
    payload
}

fn handshake() -> Vec<u8> {
    let mut out = Vec::new();
    out.push(10); // protocol version
    out.extend(b"8.0.36-fake\0");
    out.extend(1u32.to_le_bytes()); // connection id
    let nonce = *b"abcdefgh"; // part 1, 8 bytes
    out.extend(nonce);
    out.push(0); // filler
    out.extend((CAPABILITIES as u16).to_le_bytes());
    out.push(0x21); // utf8_general_ci
    out.extend(STATUS_AUTOCOMMIT.to_le_bytes());
    out.extend(((CAPABILITIES >> 16) as u16).to_le_bytes());
    out.push(21); // auth plugin data length
    out.extend([0u8; 10]);
    out.extend(*b"ijklmnopqrst\0"); // part 2, 13 bytes
    out.extend(b"mysql_native_password\0");
    out
}

fn ok_packet() -> Vec<u8> {
    let mut out = vec![0x00, 0x00, 0x00];
    out.extend(STATUS_AUTOCOMMIT.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

fn err_packet(code: u16, message: &str) -> Vec<u8> {
    let mut out = vec![0xff];
    out.extend(code.to_le_bytes());
    out.push(b'#');
    out.extend(b"HY000");
    out.extend(message.as_bytes());
    out
}

fn eof_packet() -> Vec<u8> {
    let mut out = vec![0xfe];
    out.extend(0u16.to_le_bytes()); // warnings
    out.extend(STATUS_AUTOCOMMIT.to_le_bytes());
    out
}

fn prepare_ok(id: u32, params: u16) -> Vec<u8> {
    let mut out = vec![0x00];
    out.extend(id.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // columns (unknown until execute)
    out.extend(params.to_le_bytes());
    out.push(0x00);
    out.extend(0u16.to_le_bytes()); // warnings
    out
}

fn column_def(col: &Col) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(lenenc_str("def")); // catalog
    out.extend(lenenc_str("")); // schema
    out.extend(lenenc_str("")); // table
    out.extend(lenenc_str("")); // org_table
    out.extend(lenenc_str(col.name));
    out.extend(lenenc_str(col.name)); // org_name
    out.push(0x0c);
    out.extend(33u16.to_le_bytes()); // charset
    out.extend(1024u32.to_le_bytes()); // column length
    out.push(if col.numeric {
        TYPE_LONGLONG
    } else {
        TYPE_VAR_STRING
    });
    out.extend(0u16.to_le_bytes()); // flags
    out.push(0); // decimals
    out.extend([0u8; 2]);
    out
}

fn lenenc_int(value: u64) -> Vec<u8> {
    if value < 251 {
        vec![value as u8]
    } else if value < 65_536 {
        let mut out = vec![0xfc];
        out.extend((value as u16).to_le_bytes());
        out
    } else {
        let mut out = vec![0xfd];
        let bytes = (value as u32).to_le_bytes();
        out.extend(&bytes[..3]);
        out
    }
}

fn lenenc_str(value: &str) -> Vec<u8> {
    let mut out = lenenc_int(value.len() as u64);
    out.extend(value.as_bytes());
    out
}

fn write_packet(stream: &mut TcpStream, seq: u8, payload: &[u8]) -> std::io::Result<()> {
    let len = payload.len() as u32;
    let mut header = [0u8; 4];
    header[0..3].copy_from_slice(&len.to_le_bytes()[0..3]);
    header[3] = seq;
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    stream.flush()
}

fn read_packet(stream: &mut TcpStream) -> std::io::Result<(u8, Vec<u8>)> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    let len = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;
    Ok((header[3], payload))
}
