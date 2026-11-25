# NextDude Deployment & Operations Guide

This guide covers building, packaging, configuring, securing, and operating both `nextdude-server` (Linux) and `nextdude-client` (Windows).

---

## Part 1: Linux Server (`nextdude-server`)

Supported targets: Debian 12 (Bookworm), Ubuntu 22.04 LTS, Ubuntu 24.04 LTS (`x86_64`).

### 1.1 Build Dependencies

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config protobuf-compiler libssl-dev
```

Install Rust stable (`>= 1.80`) via `rustup` if not already present, and optionally `cargo-deb` for `.deb` package creation:

```bash
cargo install cargo-deb
```

### 1.2 Building Binary, Tarball, or `.deb` Package

Build release binary and verify configuration crate checks:

```bash
cargo test -p nextdude-common -p nextdude-server
cargo build --release -p nextdude-server
```

Create a Debian package (uses metadata in `crates/server/Cargo.toml` and maintainer scripts in `packaging/linux/`):

```bash
cargo deb -p nextdude-server
sudo dpkg -i target/debian/nextdude-server_*.deb
```

Or create a portable tarball bundle (`dist/nextdude-server-0.1.0-linux-x86_64.tar.gz`):

```bash
./packaging/linux/build-tarball.sh 0.1.0
```

### 1.3 Systemd Service, User, and Permissions

The `.deb` maintainer script (`packaging/linux/postinst`) automatically:
- Creates system user/group `nextdude` (`nologin`).
- Creates `/var/lib/nextdude`, `/var/log/nextdude` (`0750`, owned by `nextdude:nextdude`), and `/etc/nextdude/profiles`.
- Installs `/etc/nextdude/server.toml` (`0640`, `root:nextdude`) and default profiles under `/usr/share/nextdude/profiles/*.toml`.
- Enables the hardened systemd unit (`packaging/linux/nextdude-server.service`) with:
  - `User=nextdude`, `Group=nextdude`, `NoNewPrivileges=true`
  - `ProtectSystem=strict`, `ProtectHome=true`, `PrivateTmp=true`
  - `ReadWritePaths=/var/lib/nextdude /var/log/nextdude`
  - `AmbientCapabilities=CAP_NET_RAW CAP_NET_BIND_SERVICE`

#### ICMP & Privileged UDP Ports

`nextdude-server` listens on `514/udp` (Syslog) and `162/udp` (SNMP Trap) and sends ICMP echo requests:
- `CAP_NET_BIND_SERVICE` allows binding UDP ports `< 1024` as the non-root `nextdude` user.
- For ICMP, the server prefers unprivileged ICMP datagram sockets (`net.ipv4.ping_group_range`) and falls back to `CAP_NET_RAW` raw sockets when needed. To allow unprivileged ICMP pings even without `CAP_NET_RAW`:

```bash
echo 'net.ipv4.ping_group_range = 0 2147483647' | sudo tee /etc/sysctl.d/99-nextdude-icmp.conf
sudo sysctl --system
```

Check socket & config readiness before starting:

```bash
sudo -u nextdude nextdude-server --config /etc/nextdude/server.toml check-config
```

### 1.4 Configuration (`/etc/nextdude/server.toml`)

Key directives in `packaging/linux/server.toml`:
- `listen_addr` / `grpc_port` (`9443`): gRPC-over-TLS 1.3 listen endpoint.
- `data_dir` (`/var/lib/nextdude`): holds `nextdude.db` (SQLite WAL), `master.key` (`0600`), auto-generated `certs/`, and uploaded/rasterized `images/`.
- `log_dir` (`/var/log/nextdude`): daily rolling `nextdude-server.log` files (in addition to `systemd-journald` integration on Linux).
- `tls_cert_path` / `tls_key_path`: optional paths to CA-signed PEM certificate and private key; omit both to auto-generate a self-signed certificate for client TOFU pinning.
- `system_profiles_dir` (`/usr/share/nextdude/profiles`) & `override_profiles_dir` (`/etc/nextdude/profiles`): custom TOML files with matching `id` in `/etc/nextdude/profiles` override shipped profiles.
- `syslog_enabled`, `syslog_port` (`514`), `syslog_queue_capacity` (`8192`), `syslog_forward_addr`: UDP syslog ingestion & optional forwarder.
- `trap_enabled`, `trap_port` (`162`), `mndp_enabled`, `mndp_port` (`5678`): SNMP trap/inform & MikroTik MNDP discovery listeners.
- `status_web_enabled` (`true`), `status_web_addr` (`127.0.0.1`), `status_web_port` (`9080`): lightweight read-only HTTP server status page (`/`, `/api/stats`, `/healthz`). Binds loopback by default; use SSH port forwarding (`ssh -L 9080:127.0.0.1:9080`) or place behind an authenticated TLS reverse proxy if exposed remotely.
- `allow_image_url_import` (`false` by default): keeps remote URL image imports disabled unless explicitly required.
- `retention_*`: controls raw sample, rollup, event, syslog, and audit log cleanup intervals.

### 1.5 Firewall Configuration

Allow only trusted management subnets to reach port `9443/tcp`, and network infrastructure subnets to send UDP telemetry:

```bash
# UFW example (adjust source CIDRs to your management/network ranges)
sudo ufw allow from 10.0.0.0/8 to any port 9443 proto tcp comment 'NextDude gRPC TLS'
sudo ufw allow from 10.0.0.0/8 to any port 514 proto udp comment 'NextDude Syslog'
sudo ufw allow from 10.0.0.0/8 to any port 162 proto udp comment 'NextDude SNMP Trap'
sudo ufw allow from 10.0.0.0/8 to any port 5678 proto udp comment 'NextDude MNDP'
```

### 1.6 Administrator Provisioning & Backups

Create or reset administrators from the server CLI:

```bash
sudo -u nextdude nextdude-server --config /etc/nextdude/server.toml \
  admin create --username nocadmin --password '<strong-password>' --group Admin

sudo -u nextdude nextdude-server --config /etc/nextdude/server.toml \
  admin reset-password --username nocadmin --password '<new-strong-password>'
```

Perform an online SQLite backup while the daemon is running, and **back up `/var/lib/nextdude/master.key` separately in a secure vault** (encrypted secrets in SQLite cannot be decrypted without `master.key`):

```bash
sudo -u nextdude nextdude-server --config /etc/nextdude/server.toml \
  backup --path /var/backups/nextdude-$(date +%F).db
```

Restore from backup (stop the service first):

```bash
sudo systemctl stop nextdude-server
sudo -u nextdude nextdude-server --config /etc/nextdude/server.toml \
  restore --path /var/backups/nextdude-2026-01-15.db
sudo systemctl start nextdude-server
```

---

## Part 2: Windows Desktop Client (`nextdude-client`)

Supported targets: Windows 10 build 1809+ and Windows 11 (`x86_64`).

### 2.1 Build Prerequisites

1. **Visual Studio 2022 Build Tools** with **Desktop development with C++** (MSVC v143 + Windows 10/11 SDK).
2. **Rustup** with target `x86_64-pc-windows-msvc` (stable `>= 1.80`).
3. **Protocol Buffers compiler** (`protoc.exe` on `%PATH%`, e.g., via `winget install Google.Protobuf` or GitHub releases).
4. *(Optional for MSI)* **WiX Toolset v4** (`dotnet tool install --global wix`).

### 2.2 Building Binary and MSI Installer

```powershell
# Compile release binary (embeds app.manifest & generated 32x32 ICO via winresource)
cargo build --release -p nextdude-client

# Output binary:
# target\release\nextdude-client.exe

# Optional: build Windows MSI installer via WiX v4
wix build packaging\windows\NextDudeClient.wxs -o dist\NextDudeClient-0.1.0-x64.msi
```

`packaging\windows\NextDudeClient.wxs` installs:
- `C:\Program Files\NextDude\nextdude-client.exe`
- Start Menu shortcut (`NextDude Client`) stamped with `System.AppUserModel.ID = NextDude.Client.Desktop.1` so Windows Toast alerts display properly in Action Center.
- Optional Windows login autostart feature (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\NextDude`), which can also be toggled later inside Client Settings.

### 2.3 Client Runtime Data & Uninstall Cleanup

- Config & TOFU certificate pins: `%LOCALAPPDATA%\NextDude\client_config.json`
- Rolling daily client logs: `%LOCALAPPDATA%\NextDude\logs\nextdude-client.log.*`

To completely remove all client configuration and cached server fingerprints after uninstalling the MSI, delete `%LOCALAPPDATA%\NextDude\`.
