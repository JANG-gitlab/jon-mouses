<p align="center">
  <img src="docs/logo.png" width="96" alt="">
</p>

<h1 align="center">Jon Mouse's</h1>

<p align="center">
  One mouse and one keyboard for every computer on your desk.<br>
  <a href="README.fr.md">Lire en français</a>
</p>

<p align="center">
  <a href="https://github.com/JANG-gitlab/jon-mouses/releases/latest/download/JonMouses-mac.dmg"><b>Download for Mac</b></a>
  &nbsp;·&nbsp;
  <a href="https://github.com/JANG-gitlab/jon-mouses/releases/latest/download/JonMouses-windows-setup.exe"><b>Download for Windows</b></a>
</p>

---

Push the cursor off the edge of one screen and it walks onto the next one, keyboard included, as if your Mac and your PC were a single desk. Copy on one, paste on the other.

## Why another one

Deskflow, Input Leap and Lan Mouse already share a mouse. Jon Mouse's focuses on what they leave to the user:

- **Nothing to configure.** Devices find each other on the local network. To pair, you type a 6-digit code. No IP address, no server, no config file.
- **Real-size layout.** Each screen is drawn at its physical size in centimetres. Place them like they sit on your desk and the cursor crosses at the same physical height, even between a 13-inch laptop and a 27-inch monitor. Elsewhere the edge is a wall.
- **French and English** from the start.

## Install

### Mac

1. Download [`JonMouses-mac.dmg`](https://github.com/JANG-gitlab/jon-mouses/releases/latest/download/JonMouses-mac.dmg), open it and drag Jon Mouse's into Applications.
2. Open it once. macOS says it cannot check the developer: open **System Settings → Privacy & Security** and click **Open Anyway** at the bottom. This only happens the first time, because the app is not signed with a paid Apple developer account yet.
3. The mouse icon appears in the menu bar. Click it and follow the two **Allow** lines (Accessibility, then Input Monitoring). macOS needs both to read and move the cursor.

### Windows

1. Download [`JonMouses-windows-setup.exe`](https://github.com/JANG-gitlab/jon-mouses/releases/latest/download/JonMouses-windows-setup.exe) and run it.
2. If SmartScreen shows a blue window, click **More info → Run anyway**. Same reason: no paid code-signing certificate yet.
3. When the firewall asks, allow **private networks**. The mouse icon appears next to the clock.

## Use it

1. Start Jon Mouse's on both computers, on the same network.
2. From the menu of one of them, click **Pair …**. The other one shows a code; type it.
3. Open **Screen layout…** and drag the screens where they really are. The orange segment is where the cursor can cross.
4. Hold **Ctrl + Alt + Esc** for one second at any time to bring the cursor home.

## Platforms

| | Mac | Windows | Android | iPad | Linux |
|---|---|---|---|---|---|
| Share mouse and keyboard | ✅ | ✅ | planned | ❌ | planned |
| Real-size layout | ✅ | ✅ | planned | ❌ | planned |
| Shared clipboard (text, images) | ✅ | ✅ | planned | ❌ | planned |
| Main display only | yes, for now | yes, for now | | | |

**iPad will not happen**: iPadOS does not let an app move the pointer or type into other apps. Only Apple's Universal Control can.

## Wi-Fi on Macs

Macs keep a second radio link alive for AirDrop and Continuity (AWDL). About once a second it pulls the Wi-Fi chip onto another channel for 50 to 100 ms, and the cursor stutters. A cable fixes it, and so does setting your router's 5 GHz channel to 44 or 149.

Otherwise, on first launch Jon Mouse's offers to install a small helper that switches AWDL off while the app runs and back on when it quits. macOS asks for your password once. If you said no, the menu keeps a **Reduce Wi-Fi stutter…** line. The helper is two short files in [`app/helper`](app/helper); to install it by hand:

```bash
sudo install -o root -g wheel -m 755 app/helper/awdl.sh /Library/PrivilegedHelperTools/app.jonmouses.awdl
sudo install -o root -g wheel -m 644 app/helper/app.jonmouses.awdl.plist /Library/LaunchDaemons/app.jonmouses.awdl.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/app.jonmouses.awdl.plist
```

To remove it:

```bash
sudo launchctl bootout system/app.jonmouses.awdl
sudo rm /Library/LaunchDaemons/app.jonmouses.awdl.plist /Library/PrivilegedHelperTools/app.jonmouses.awdl
```

## Security and privacy

- Pairing uses SPAKE2 on the 6-digit code, so a listener on the network cannot guess it offline, and an attacker gets one try per code.
- Every connection then runs Noise IK with the keys exchanged at pairing. Motion travels over encrypted UDP, everything else over encrypted TCP.
- Nothing leaves your local network. No account, no telemetry.

## Build from source

You need Rust and Node.js.

```bash
cd app
npx @tauri-apps/cli@2 build
```

## Credits

Input capture and emulation build on the `input-capture` and `input-emulation` crates from [Lan Mouse](https://github.com/feschber/lan-mouse) by Ferdinand Schober, patched in [`vendor/`](vendor) for smoother macOS capture and reliable key repeat.

## License

[GPL-3.0](LICENSE). See [CONTRIBUTING.md](CONTRIBUTING.md) to help.
