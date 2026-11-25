# NextDude

NextDude is a production-oriented network monitoring system written in Rust, designed as a modern successor to MikroTik The Dude. It separates execution cleanly into a headless Linux monitoring daemon (`nextdude-server`) and a native Windows desktop GUI client (`nextdude-client`) backed by a shared domain and gRPC/Protobuf crate (`nextdude-common`) and a cross-platform client engine (`nextdude-client-core`).

All polling, probing, discovery, syslog/trap ingestion, alert evaluation, dependency suppression, historical telemetry storage, and user workspace preferences execute on the server. Multiple Windows clients can connect concurrently to one server, and a single client stores profiles for multiple servers.

---

## Key Capabilities

- **Client-Server State Separation**:
  - `nextdude-server` (Linux x86_64) runs as a non-root `systemd` service with zero GUI dependencies.
  - `nextdude-client` (Windows 10 build 1809+ / Windows 11 x64) is built with `eframe`/`egui` (`wgpu` DX12/GL with software fallback) and holds no authoritative monitoring database. Per-admin panel layouts, filters, and last-opened map live on the server.
- **Monitoring & Probes**:
  - Supports `ICMP`, `TCP`, `HTTP`, `HTTPS`, `DNS`, `SNMP OID`, `SNMP Interface`, `SNMP Custom`, `FTP`, `SMTP`, `IMAP`, `POP3`, `SSH`, `Telnet`, and `NTP`.
  - Linux ICMP uses unprivileged datagram ICMP sockets (`SOCK_DGRAM` / `IPPROTO_ICMP`) when `net.ipv4.ping_group_range` permits it, automatically falling back to `SOCK_RAW` when `CAP_NET_RAW` is granted, with an explicit startup capability check.
  - Per-probe debounce (`Unknown`, `Up`, `Down`, `Unreachable`, `Partial`), monotonic scheduler intervals with jitter, global and per-host concurrency limits, down-host backoff, `catch_unwind` task supervision with automatic restart, and root-cause dependency suppression.
- **Pure-Rust SNMPv1 / v2c / v3 Engine**:
  - Custom BER/DER codec and RFC 3414 USM implementation (`MD5` / `SHA-1` authentication, `DES-CBC` / `AES-128-CFB` privacy) supporting `GET`, `GETNEXT`, `GETBULK`, table walks, `Trapv1`, `Trapv2`, and `InformRequest` (UDP 162).
  - Externalized TOML vendor profiles (`/usr/share/nextdude/profiles` with `/etc/nextdude/profiles` overrides) for Generic MIB-II / HOST-RESOURCES-MIB, MikroTik, Cisco, Juniper, Huawei, ZTE, Ruijie, Alcatel-Lucent, and Ubiquiti, plus a built-in MIB OID name dictionary.
- **Interface & Link Monitoring**:
  - Polls `ifOperStatus`, `ifAdminStatus`, and 64-bit counters (`ifHCInOctets` / `ifHCOutOctets`) with 32-bit fallback, counter wrap and device reboot reset handling, and stable mapping by `ifName`/`ifDescr`.
  - Link status evaluation (`Up`, `Down`, `NoReply`, `Flapping`, `Unknown`), utilization calculation, and elastic/curved/orthogonal parallel link routing on the map canvas.
- **Discovery, Logs, Traps, Notifications & Diagnostics**:
  - CIDR sweep, LLDP-MIB / CDP-MIB traversal, and MikroTik MNDP listener (UDP 5678) streaming candidate devices and links with duplicate detection and force-directed auto-layout.
  - Tolerant RFC 3164 / RFC 5424 syslog receiver (UDP 514) with vendor parsers, bounded ingestion queue, drop counter visibility, alert rule matching, external syslog forwarding, and Telegram Bot notification queue.
  - Interactive streamed diagnostics (`Ping`, `Traceroute`, `TCP Connect`, `DNS Lookup`, `SNMP Walk`).
- **Built-In Server Status Web Page**:
  - Lightweight offline-capable read-only HTTP status UI (`http://127.0.0.1:9080/` by default; JSON at `/api/stats` and `/healthz`) showing live running child tasks (`polling_scheduler`, `rollup_retention`, receivers, active probe/discovery/tool workers, restart counts) and aggregate server stats (uptime, Linux `/proc` RSS/load, SQLite DB + WAL size, device/service/link totals, active outages, active client sessions, syslog drops) without exposing device names, IPs, usernames, or secrets.
- **Interactive Topology Map & Asset Pipeline**:
  - Two switchable map layers (Physical Links and Dependencies), viewport culling + Level-of-Detail (LOD) rendering targeting 2,000+ nodes, copy-paste across maps, alignment tools, lock toggle, undo/redo, PNG export, and global Find.
  - Hardened server image pipeline rejecting SVG scripts/external refs and rasterizing validated SVGs via `resvg` alongside PNG/JPEG/WEBP thumbnails (`32`, `64`, `128`, `256`).
- **Security & Packaging**:
  - TLS 1.3 via `rustls` (`ring` provider) with Trust-On-First-Use (TOFU) SHA-256 certificate fingerprint pinning on the client.
  - Argon2id admin password hashing, RBAC groups, brute-force lockout, ChaCha20-Poly1305 encryption at rest (`master.key`, `0600`), write-only secret API fields, Windows DPAPI (`CryptProtectData` / `CurrentUser`) password protection in `client_config.json`.
  - Debian/Ubuntu `.deb` packaging metadata (`cargo-deb`), systemd hardening unit, tarball bundler, and Windows WiX v4 MSI installer definition (`NextDudeClient.wxs`).

---

## Workspace Structure

```text
dudeV2/
├── Cargo.toml                     # Workspace manifest, strict lints, LTO release profile
├── LICENSE                        # Apache-2.0
├── README.md
├── docs/
│   ├── ARCHITECTURE.md            # System architecture, protocol, SNMP & engine internals
│   └── DEPLOYMENT.md              # Linux server & Windows client build, setup, and packaging
├── assets/icons/                  # Built-in geometric SVG device icons
├── profiles/                      # Default TOML SNMP vendor profiles
├── packaging/
│   ├── linux/                     # server.toml, systemd unit, maintainer scripts, tarball script
│   └── windows/                   # WiX v4 installer source (NextDudeClient.wxs)
└── crates/
    ├── common/                    # nextdude-common: APP_NAME, domain types, OID dict, protobuf/gRPC
    ├── server/                    # nextdude-server: daemon, SNMP, SQLite WAL, engine, gRPC API
    ├── client-core/               # nextdude-client-core: TLS TOFU, gRPC worker, cache, i18n, geometry
    └── client/                    # nextdude-client: Windows egui/wgpu desktop app & Win32 platform layer
```

For detailed internals, data flows, schema overview, and concurrency guarantees, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

---

## Quick Start

### 1. Prerequisites

| Component | Linux Server (`nextdude-server`) | Windows Client (`nextdude-client`) |
|---|---|---|
| Rust Toolchain | Rust stable (`>= 1.80`) | Rust stable (`>= 1.80`, `x86_64-pc-windows-msvc`) |
| Protobuf Compiler | `protobuf-compiler` (`protoc` in `PATH`) | `protoc.exe` in `PATH` |
| System Libraries | `pkg-config` (bundled SQLite via `rusqlite`) | Visual Studio Build Tools (MSVC C++ workload) |
| Packaging (Optional) | `cargo-deb`, `systemd` | WiX Toolset v4 (`wix`) |

### 2. Build & Test

On Linux:

```bash
cargo fmt --check
cargo clippy -p nextdude-common -p nextdude-server -p nextdude-client-core --all-targets -- -D warnings
cargo test -p nextdude-common -p nextdude-server -p nextdude-client-core
cargo build --release -p nextdude-server
```

On Windows (PowerShell with MSVC toolchain and `protoc` installed):

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p nextdude-client
```

### 3. First Server Setup (Linux)

```bash
# Create configuration & runtime directories (or install generated .deb)
sudo nextdude-server --config /etc/nextdude/server.toml check-config

# Create initial administrator before connecting from Windows
sudo -u nextdude nextdude-server --config /etc/nextdude/server.toml \
  admin create --username admin --password 'ChangeMeStrongPassword!' --group Admin

# Start service
sudo systemctl enable --now nextdude-server
```

Open required firewall ports on the Linux host:
- `9443/tcp`: Client gRPC-over-TLS 1.3 connections
- `9080/tcp`: Optional lightweight read-only HTTP status page (binds `127.0.0.1` by default; keep local via SSH tunnel or reverse proxy unless required on a management subnet)
- `514/udp`: Syslog receiver (optional if disabled)
- `162/udp`: SNMP Trap / Inform receiver (optional if disabled)
- `5678/udp`: MikroTik MNDP listener (optional if disabled)

For full Debian `.deb`, tarball, `net.ipv4.ping_group_range` vs `CAP_NET_RAW`, backup/restore, TLS custom certificates, and WiX v4 MSI build instructions, see [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md).

---

## Security Notes & Operational Scope

- **Server-Side Storage & Encryption**: Secrets (SNMP communities, SNMPv3 auth/priv passwords, Telegram bot tokens) are encrypted with ChaCha20-Poly1305 using `/var/lib/nextdude/master.key` (`0600`). API responses redact secrets to empty strings; admins must re-enter secrets when rotating credentials.
- **No Unauthenticated Plaintext Control Channel**: The gRPC server enforces TLS 1.3 and validates client session tokens against hashed tokens / server state. On first connection to a self-signed server, `nextdude-client` displays the SHA-256 certificate fingerprint for explicit TOFU confirmation before transmitting credentials.
- **Windows DPAPI Caveat**: Saved server passwords when "Remember password" is enabled are encrypted with Windows DPAPI (`CryptProtectData`, `CurrentUser` scope) in `%LOCALAPPDATA%\NextDude\client_config.json`. Any process running under the same logged-in Windows user account can decrypt DPAPI data; leaving "Remember password" unchecked avoids persisting credentials on disk.
- **Toast & Alarm Delivery**: Windows Toast notifications and looped alarm audio only fire while `nextdude-client` is running (including minimized to the Windows system tray) and actively connected to the server. No background Windows service runs when the client exits.
- **Out of Scope**: Full Web configuration/topology UI, agent/remote probes, and RouterOS proprietary API are excluded (`nextdude-server` exposes only the lightweight read-only status page on port `9080`; all map/device configuration runs through `nextdude-client`, while `ProbeExecutor` in `crates/server/src/probe/mod.rs` remains open for future remote agents).
- **MSI Uninstall Behavior**: Uninstalling via the WiX v4 MSI removes the application binaries, Start Menu shortcut, and autostart registry entry if installed, while leaving user logs and `client_config.json` intact under `%LOCALAPPDATA%\NextDude\`. Remove that folder manually for a complete purge.

---

## Verification Status & Pending Checks (Perlu Verifikasi)

Because command execution in the authoring environment failed (`powershell` executable unavailable in `%PATH%`), compilation, lints, tests, and packaging have **not** yet been run in this environment. Verify the following items directly on Linux and Windows build hosts:

1. **Workspace Build, Clippy, and Test Suite**:
   - Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` on both Linux (`nextdude-server`) and Windows (`nextdude-client`).
2. **Linux ICMP Socket (`SOCK_DGRAM` vs `SOCK_RAW`) Behavior**:
   - Verify `tokio::net::UdpSocket::from_std` wrapping `socket2` `Type::RAW` (`IPPROTO_ICMP` / `IPPROTO_ICMPV6`) on target Debian 12 / Ubuntu 22.04 / 24.04 kernels in both `CAP_NET_RAW` mode and unprivileged `net.ipv4.ping_group_range` mode (`nextdude-server check-config`).
3. **Windows Crate Feature Flags (`windows` 0.58 & `windows-sys` 0.59)**:
   - Confirm all feature names in `crates/client/Cargo.toml` and `WinRT` `ToastNotificationManager` / `XmlDocument` bindings in `crates/client/src/platform.rs` compile cleanly against `x86_64-pc-windows-msvc`.
4. **SNMPv3 RFC 3414 Interoperability on Real Hardware**:
   - Unit tests cover BER roundtrips, USM key localization, HMAC authentication, and AES/DES cipher symmetry. Wire interoperability (particularly authoritative engine boots/time discovery, privacy salt ordering, and CFB behavior) should be verified against `net-snmp` (`snmpd`) or MikroTik RouterOS / Cisco IOS hardware.
5. **Vendor Profile OID Coverage (`profiles/*.toml`)**:
   - Standard MIB-II (`IF-MIB`, `HOST-RESOURCES-MIB`), MikroTik (`1.3.6.1.4.1.14988`), Cisco (`1.3.6.1.4.1.9`), Juniper (`1.3.6.1.4.1.2636`), and Huawei (`hwEntityCpuUsage`) base OIDs are included, while model-specific temperature/fan/PSU OIDs for ZTE, Ruijie, Alcatel-Lucent OmniSwitch (`chasEntTempCurrent` variant differences), Huawei, and Ubiquiti airMAX/UniFi hardware revisions should be checked via `nextdude-client` SNMP Walk before relying on threshold alerts.
