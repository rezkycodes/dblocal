//! Protocol clients used to prove an engine really works.
//!
//! Both are the production crates the adapters use (`tokio-postgres` and
//! `redis`), driven the same way: no wrapper that could hide a failure.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use mysql_async::prelude::Queryable;
use redis::{Commands, Connection};
use tokio_postgres::{Client, NoTls};

/// A `tokio-postgres` session kept alive by its own small runtime.
pub struct PgClient {
    runtime: tokio::runtime::Runtime,
    client: Client,
}

impl PgClient {
    /// Connects using the URI the service produced.
    pub fn connect(uri: &str, timeout: Duration) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| format!("build the client runtime: {err}"))?;
        let config: tokio_postgres::Config = uri
            .parse()
            .map_err(|err| format!("parse the postgres URI: {err}"))?;
        let _ = timeout;
        let (client, connection) = runtime
            .block_on(config.connect(NoTls))
            .map_err(|err| format!("connect to postgres at {uri}: {err}"))?;
        runtime.spawn(async move {
            let _ = connection.await;
        });
        Ok(Self { runtime, client })
    }

    /// Runs a statement that returns rows with one integer column.
    pub fn query_one(&self, sql: &str) -> Result<i32, String> {
        let row = self
            .runtime
            .block_on(self.client.query_one(sql, &[]))
            .map_err(|err| format!("query {sql:?}: {err}"))?;
        row.try_get::<_, i32>(0)
            .map_err(|err| format!("column 0 of {sql:?}: {err}"))
    }

    /// Runs a statement that returns text.
    pub fn query_text(&self, sql: &str) -> Result<String, String> {
        let row = self
            .runtime
            .block_on(self.client.query_one(sql, &[]))
            .map_err(|err| format!("query {sql:?}: {err}"))?;
        row.try_get::<_, String>(0)
            .map_err(|err| format!("column 0 of {sql:?}: {err}"))
    }

    /// Runs a batch of statements with no result.
    pub fn batch(&self, sql: &str) -> Result<(), String> {
        self.runtime
            .block_on(self.client.batch_execute(sql))
            .map_err(|err| format!("batch {sql:?}: {err}"))
    }
}

/// A synchronous `redis` connection, the same client the Valkey adapter uses.
pub struct RedisClient {
    connection: Connection,
}

impl RedisClient {
    /// Connects to `uri`, authenticating when the URI carries a password.
    pub fn connect(uri: &str) -> Result<Self, String> {
        let client =
            redis::Client::open(uri).map_err(|err| format!("parse the redis URI {uri}: {err}"))?;
        let connection = client
            .get_connection_with_timeout(Duration::from_secs(5))
            .map_err(|err| format!("connect to valkey at {uri}: {err}"))?;
        Ok(Self { connection })
    }

    /// `PING`, expecting `PONG`.
    pub fn ping(&mut self) -> Result<String, String> {
        redis::cmd("PING")
            .query::<String>(&mut self.connection)
            .map_err(|err| format!("PING: {err}"))
    }

    /// `SET key value`.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let _: () = self
            .connection
            .set(key, value)
            .map_err(|err| format!("SET {key}: {err}"))?;
        Ok(())
    }

    /// `GET key`.
    pub fn get(&mut self, key: &str) -> Result<Option<String>, String> {
        self.connection
            .get(key)
            .map_err(|err| format!("GET {key}: {err}"))
    }
}

/// A `mysql_async` session kept alive by its own small runtime.
///
/// This is the production client crate the MySQL/MariaDB adapters already depend
/// on, driven with the URI the service produced, so the test proves real SQL and
/// not just a socket that opens.
pub struct MyClient {
    runtime: tokio::runtime::Runtime,
    conn: mysql_async::Conn,
}

impl MyClient {
    /// Connects using the URI the service produced.
    pub fn connect(uri: &str) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| format!("build the client runtime: {err}"))?;
        let opts = mysql_async::Opts::from_url(uri)
            .map_err(|err| format!("parse the mysql URI {uri}: {err}"))?;
        let conn = runtime
            .block_on(mysql_async::Conn::new(opts))
            .map_err(|err| format!("connect to mysql at {uri}: {err}"))?;
        Ok(Self { runtime, conn })
    }

    /// Runs a query whose first column is an integer.
    pub fn query_i64(&mut self, sql: &str) -> Result<i64, String> {
        let value: Option<i64> = self
            .runtime
            .block_on(self.conn.query_first(sql))
            .map_err(|err| format!("query {sql:?}: {err}"))?;
        value.ok_or_else(|| format!("query {sql:?} returned no row"))
    }

    /// Runs one or more statements with no result.
    pub fn batch(&mut self, sql: &str) -> Result<(), String> {
        self.runtime
            .block_on(self.conn.query_drop(sql))
            .map(|_| ())
            .map_err(|err| format!("batch {sql:?}: {err}"))
    }

    /// Runs a query whose first column is text.
    pub fn query_one_text(&mut self, sql: &str) -> Result<String, String> {
        let value: Option<String> = self
            .runtime
            .block_on(self.conn.query_first(sql))
            .map_err(|err| format!("query {sql:?}: {err}"))?;
        value.ok_or_else(|| format!("query {sql:?} returned no row"))
    }
}

/// A `mysql_async` session at one user/db over loopback TCP, for proving the
/// privileges an admin-created account actually has.
pub struct ScopedMyClient {
    runtime: tokio::runtime::Runtime,
    conn: mysql_async::Conn,
}

impl ScopedMyClient {
    /// Connects as `user`/`password` to `db` on `127.0.0.1:port` over TCP.
    pub fn connect(port: u16, user: &str, password: &str, db: &str) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| format!("build the client runtime: {err}"))?;
        let builder = mysql_async::OptsBuilder::default()
            .ip_or_hostname("127.0.0.1")
            .tcp_port(port)
            .user(Some(user))
            .pass(Some(password))
            .db_name(Some(db))
            .prefer_socket(false);
        let opts = mysql_async::Opts::from(builder);
        let conn = runtime
            .block_on(mysql_async::Conn::new(opts))
            .map_err(|err| format!("connect as {user} to {db} on {port}: {err}"))?;
        Ok(Self { runtime, conn })
    }

    /// Runs a query whose first column is an integer.
    pub fn query_i64(&mut self, sql: &str) -> Result<i64, String> {
        let value: Option<i64> = self
            .runtime
            .block_on(self.conn.query_first(sql))
            .map_err(|err| format!("query {sql:?}: {err}"))?;
        value.ok_or_else(|| format!("query {sql:?} returned no row"))
    }

    /// Runs a statement; `Ok(())` when the server accepts it.
    pub fn can(&mut self, sql: &str) -> bool {
        self.runtime.block_on(self.conn.query_drop(sql)).is_ok()
    }

    /// Runs a statement, returning an error string when the server refuses it.
    pub fn batch(&mut self, sql: &str) -> Result<(), String> {
        self.runtime
            .block_on(self.conn.query_drop(sql))
            .map(|_| ())
            .map_err(|err| format!("batch {sql:?}: {err}"))
    }
}

/// Opens a raw TCP connection to `127.0.0.1:port` and closes it.
///
/// Used only as a liveness pre-check before handing the port to a protocol
/// client, so a "connection refused" is reported as such rather than as a
/// protocol error.
pub fn tcp_reachable(port: u16, timeout: Duration) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        timeout,
    )
    .is_ok()
}

/// Waits until the port accepts a TCP connection.
pub fn wait_tcp(port: u16, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if tcp_reachable(port, Duration::from_millis(200)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Waits until the port refuses a TCP connection.
pub fn wait_tcp_gone(port: u16, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if !tcp_reachable(port, Duration::from_millis(100)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    !tcp_reachable(port, Duration::from_millis(100))
}
