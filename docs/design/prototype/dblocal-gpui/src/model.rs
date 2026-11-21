//! Model domain DBLocal (sisi UI).
//!
//! Prototipe ini memakai data contoh di memori. Pada implementasi nyata, struct di
//! sini dibaca dari crate `dblocal-core` (state.db SQLite + EngineAdapter) — UI tidak
//! boleh tahu detail proses engine (aturan arsitektur #1 di PRD).

use gpui_kit::{Rgba, SharedString};

use crate::theme::tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EngineKind {
    Postgres,
    Valkey,
    MySql,
    MariaDb,
}

impl EngineKind {
    pub const ALL: [EngineKind; 4] = [
        EngineKind::Postgres,
        EngineKind::Valkey,
        EngineKind::MySql,
        EngineKind::MariaDb,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::Valkey => "Valkey",
            Self::MySql => "MySQL",
            Self::MariaDb => "MariaDB",
        }
    }

    pub fn mono(self) -> &'static str {
        match self {
            Self::Postgres => "PG",
            Self::Valkey => "VK",
            Self::MySql => "MY",
            Self::MariaDb => "MA",
        }
    }

    pub fn color(self) -> Rgba {
        match self {
            Self::Postgres => tokens::accent(),
            Self::Valkey => tokens::danger(),
            Self::MySql => tokens::orange(),
            Self::MariaDb => tokens::info(),
        }
    }

    pub fn canonical_port(self) -> u16 {
        match self {
            Self::Postgres => 5432,
            Self::Valkey => 6379,
            Self::MySql => 3306,
            Self::MariaDb => 3307,
        }
    }

    pub fn superuser(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Valkey => "default",
            Self::MySql | Self::MariaDb => "root",
        }
    }

    /// Versi di katalog (contoh). `true` = runtime sudah terpasang.
    pub fn catalog(self) -> &'static [(&'static str, &'static str, bool)] {
        match self {
            Self::Postgres => &[
                ("18.7.0", "Terbaru", true),
                ("17.7", "Didukung · ≈ 34 MB", false),
                ("16.11", "Didukung", true),
                ("14.20", "Didukung", true),
            ],
            Self::Valkey => &[
                ("9.0.1", "Terbaru · kompatibel Redis", true),
                ("8.1.4", "Didukung · ≈ 6 MB", false),
            ],
            Self::MySql => &[("8.4.11", "LTS · tarball minimal ≈ 80 MB", false)],
            Self::MariaDb => &[
                ("11.8.10", "LTS terbaru · ≈ 95 MB", false),
                ("11.4.14", "LTS · ≈ 92 MB", false),
            ],
        }
    }

    pub fn uri(self, user: &str, password: &str, port: u16) -> String {
        match self {
            Self::Postgres => {
                format!("postgresql://{user}:{password}@127.0.0.1:{port}/postgres")
            }
            Self::Valkey => format!("redis://{user}:{password}@127.0.0.1:{port}"),
            Self::MySql | Self::MariaDb => {
                format!("mysql://{user}:{password}@127.0.0.1:{port}/mysql")
            }
        }
    }

    pub fn env_block(self, user: &str, password: &str, port: u16) -> String {
        let (conn, db) = match self {
            Self::Postgres => ("pgsql", "postgres"),
            Self::Valkey => {
                return format!(
                    "REDIS_HOST=127.0.0.1\nREDIS_PORT={port}\nREDIS_USERNAME={user}\nREDIS_PASSWORD={password}\n"
                );
            }
            Self::MySql | Self::MariaDb => ("mysql", "mysql"),
        };
        format!(
            "DB_CONNECTION={conn}\nDB_HOST=127.0.0.1\nDB_PORT={port}\nDB_DATABASE={db}\nDB_USERNAME={user}\nDB_PASSWORD={password}\n"
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Provisioning,
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Provisioning => "Provisioning",
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Failed => "Failed",
        }
    }

    pub fn color(self) -> Rgba {
        match self {
            Self::Running => tokens::ok(),
            Self::Failed => tokens::danger(),
            Self::Stopped => tokens::text_muted(),
            Self::Provisioning | Self::Starting | Self::Stopping => tokens::warn(),
        }
    }

    pub fn is_busy(self) -> bool {
        matches!(self, Self::Provisioning | Self::Starting | Self::Stopping)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Log,
    Dbl,
    Error,
    Separator,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub time: SharedString,
    pub level: LogLevel,
    pub text: SharedString,
}

impl LogLine {
    pub fn new(time: &str, level: LogLevel, text: impl Into<SharedString>) -> Self {
        Self {
            time: SharedString::from(time.to_string()),
            level,
            text: text.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DbRow {
    pub name: SharedString,
    pub size: SharedString,
}

#[derive(Clone, Debug)]
pub enum Failure {
    PortInUse { port: u16, suggested: u16 },
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub name: SharedString,
    pub engine: EngineKind,
    pub version: SharedString,
    pub port: u16,
    pub status: Status,
    pub password: String,
    pub no_auth: bool,
    pub pid: Option<u32>,
    pub data_size: SharedString,
    pub autostart: bool,
    /// 0–100 saat `Provisioning`.
    pub progress: f32,
    pub failure: Option<Failure>,
    pub logs: Vec<LogLine>,
    pub databases: Vec<DbRow>,
}

impl Instance {
    pub fn uri(&self, reveal: bool) -> String {
        let pass = self.shown_password(reveal);
        self.engine.uri(self.engine.superuser(), &pass, self.port)
    }

    pub fn env_block(&self, reveal: bool) -> String {
        let pass = self.shown_password(reveal);
        self.engine.env_block(self.engine.superuser(), &pass, self.port)
    }

    fn shown_password(&self, reveal: bool) -> String {
        if self.no_auth {
            String::new()
        } else if reveal {
            self.password.clone()
        } else {
            "•".repeat(self.password.chars().count())
        }
    }

    pub fn data_dir(&self) -> String {
        format!("~/.dblocal/instances/{}/data", self.name)
    }
}

/// Password contoh. Implementasi nyata: 24 karakter dari CSPRNG OS → Keychain.
pub fn demo_password(seed: usize) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut x = (seed as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..24)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ALPHA[(x % ALPHA.len() as u64) as usize] as char
        })
        .collect()
}

pub fn demo_instances() -> Vec<Instance> {
    let pg_logs = vec![
        LogLine::new("21:14:02.118", LogLevel::Log, "starting PostgreSQL 18.7 on x86_64-pc-linux-gnu"),
        LogLine::new("21:14:02.120", LogLevel::Log, "listening on IPv4 address \"127.0.0.1\", port 5432"),
        LogLine::new("21:14:02.121", LogLevel::Log, "listening on IPv6 address \"::1\", port 5432"),
        LogLine::new("21:14:02.164", LogLevel::Log, "database system is ready to accept connections"),
        LogLine::new("21:14:02.402", LogLevel::Dbl, "health check pg_isready OK (284 ms sejak spawn)"),
    ];
    let shop_logs = vec![
        LogLine::new("20:02:41.006", LogLevel::Log, "received fast shutdown request"),
        LogLine::new("20:02:41.214", LogLevel::Log, "database system is shut down"),
        LogLine::new("20:02:41.220", LogLevel::Dbl, "stop selesai: shutdown bersih dalam 0,21 dtk"),
        LogLine::new("", LogLevel::Separator, "sesi baru · 22:39:12"),
        LogLine::new("22:39:12.031", LogLevel::Dbl, "start diminta oleh GUI"),
        LogLine::new("22:39:12.033", LogLevel::Dbl, "uji bind 127.0.0.1:5434 … EADDRINUSE"),
        LogLine::new("22:39:12.034", LogLevel::Dbl, "uji bind [::1]:5434 … EADDRINUSE"),
        LogLine::new("22:39:12.034", LogLevel::Error, "port 5434 dipakai proses lain; engine tidak dijalankan (DBL-1302)"),
        LogLine::new("22:39:12.035", LogLevel::Dbl, "status: starting → failed"),
    ];
    let pg_dbs = vec![
        DbRow { name: "postgres".into(), size: "7,5 MB".into() },
        DbRow { name: "app_dev".into(), size: "31,4 MB".into() },
        DbRow { name: "app_test".into(), size: "9,3 MB".into() },
    ];

    let base = |name: &str, engine: EngineKind, version: &str, port: u16, status: Status, seed: usize| Instance {
        name: SharedString::from(name.to_string()),
        engine,
        version: SharedString::from(version.to_string()),
        port,
        status,
        password: demo_password(seed),
        no_auth: false,
        pid: (status == Status::Running).then_some(48_000 + seed as u32 * 37),
        data_size: "48,2 MB".into(),
        autostart: false,
        progress: 0.,
        failure: None,
        logs: Vec::new(),
        databases: Vec::new(),
    };

    let mut pg_app = base("pg-app", EngineKind::Postgres, "18.7.0", 5432, Status::Running, 1);
    pg_app.logs = pg_logs;
    pg_app.databases = pg_dbs;
    let pg_legacy = base("pg-legacy", EngineKind::Postgres, "14.20", 5433, Status::Stopped, 2);
    let mut pg_shop = base("pg-shop", EngineKind::Postgres, "17.7", 5434, Status::Failed, 3);
    pg_shop.logs = shop_logs;
    pg_shop.failure = Some(Failure::PortInUse { port: 5434, suggested: 5435 });
    let cache = base("cache-dev", EngineKind::Valkey, "9.0.1", 6379, Status::Running, 4);
    let hr = base("laravel-hr", EngineKind::MySql, "8.4.11", 3306, Status::Stopped, 5);
    let blog = base("wp-blog", EngineKind::MariaDb, "11.8.10", 3307, Status::Running, 6);

    vec![pg_app, pg_legacy, pg_shop, cache, hr, blog]
}

/// Port bebas berikutnya mulai dari port kanonik engine, melompati yang dialokasikan.
pub fn next_free_port(instances: &[Instance], engine: EngineKind) -> u16 {
    let mut port = engine.canonical_port();
    while instances.iter().any(|i| i.port == port) {
        port += 1;
    }
    port
}
