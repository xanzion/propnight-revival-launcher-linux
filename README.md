# Propnight Revival Linux

Linux-native launcher for the [Propnight Revival](https://propnightrevival.online) community servers.

This is a complete port of the original Windows [.NET + WebView2 launcher](https://github.com/AzoTe06/propnight-revival-launcher) to **Tauri v2** (Rust + system webview). The launcher itself runs natively on Linux (no Wine/Proton required for the launcher). The game binary (`Propnight.exe`) is still a Windows Unreal Engine executable. Play launches it through Steam (`steam -applaunch 1549180`) so Steam's Proton and launch options apply.

## Features

- Signed manifest download and ECDSA P-256 verification (refuses unsigned/tampered manifests)
- Delta updates: only changed files are downloaded, SHA-256 verified before applying
- Automatic backup of original game files into `revival-backups/` before first replacement
- Byte-patching of `Propnight.exe` (only on known Steam builds, with pre/post checks)
- Goldberg Steam emulator identity setup (`force_account_name.txt`, `force_steamid.txt`)
- UE4SS mod enabling
- Profile / save sync of `SaveUserData.sav` with the server
- Friends list, presence, and join-via-relay
- Login / account handling with secure token storage (system keyring when available)
- Update notifications (never force silent updates)
- Bug reporting
- Settings persistence
- Steam library detection (native Steam, Flatpak Steam, multiple library folders via `libraryfolders.vdf`)
- Play launches via `steam -applaunch 1549180` (uses your Steam Proton and launch options)
- XDG-compliant paths (`~/.local/share`, `~/.config`)
- No root privileges required

## Prerequisites

- Rust (stable, 1.77+)
- Node.js + npm
- System webview dependencies (on Ubuntu/Debian: `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`, etc.)
- Steam with Propnight installed
- Proton (Proton-GE or Proton Experimental recommended) for running the Windows game binary

## Build

```bash
# Clone and enter
cd propnight-revival-linux

# Install frontend tooling
npm install

# Development
npm run tauri dev

# Production AppImage (and deb)
npm run tauri build
```

The AppImage will be produced under `src-tauri/target/release/bundle/appimage/`.

### Linux-specific notes

- **Steam paths**: Automatically detects `~/.local/share/Steam`, `~/.steam/steam`, Flatpak (`~/.var/app/com.valvesoftware.Steam/...`), and additional libraries from `libraryfolders.vdf`.
- **Launch**: Play runs `steam -applaunch 1549180`. Steam must be set to **Proton Experimental**, and Propnight's launch options must run the script below. The launcher deploys `src-tauri/resources/UE4SS-settings.ini` (all hooks off) on every Play.

### Steam launch script

Place the script at `~/scripts/propnight-direct.sh` and make it executable:

```bash
mkdir -p ~/scripts
cat > ~/scripts/propnight-direct.sh << 'EOF'
#!/bin/bash
export WINEDLLOVERRIDES="dwmapi=n,b;steam_api64=n,b"
export PROTON_LOG=1
exec "${1/Propnight_BE.exe/Propnight.exe}" "${@:2}"
EOF
chmod +x ~/scripts/propnight-direct.sh
```

In Steam: Propnight → Properties → Launch Options:

```text
~/scripts/propnight-direct.sh %command%
```

The script sets the DLL overrides Proton needs for UE4SS and Goldberg, and starts `Propnight.exe` instead of `Propnight_BE.exe`. `%command%` must stay so Proton still wraps the launch.
- **Saves**: Prefers the Proton prefix path for `SaveUserData.sav`, falls back to XDG locations.
- **Token storage**: Uses the system keyring (`keyring` crate) when available; falls back to the config file.

## Feature status

All core features from the original launcher are implemented and wired to the existing HTML UI. Progress reporting, folder picker, and secure storage are included.

## License

MIT (same as the original launcher)
