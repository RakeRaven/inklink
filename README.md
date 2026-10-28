# InkLink

Use an iPad + Apple Pencil as a pen display for a Windows or Linux computer. The iPad shows a live, hardware-encoded copy of one of your screens and sends the Pencil back as a real pen (pressure, tilt, hover, barrel roll, eraser), so Photoshop, Clip Studio Paint, Krita and friends treat it like a drawing tablet. Works over USB or Wi-Fi.

- **iPad app** (`ios/`): native Swift/UIKit. Low-latency H.264 decode, 240 Hz coalesced Pencil samples, hover on supported iPads.
- **Host** (`host/`): Rust + GStreamer. Captures a monitor, encodes with NVENC / Quick Sync / Media Foundation / VA-API (x264 fallback), injects input.

## Features

- **Pencil → pen**: pressure, tilt, hover (M2+ iPad Pro / M4 iPad Air with Pencil 2 or Pro), barrel roll (Pencil Pro). On Windows it's a real Windows Ink pen; on Linux a uinput tablet.
- **Double-tap / squeeze**: toggle eraser (sends the pen's eraser end, so apps switch to their eraser tool), show/hide toolbar, or any shortcut.
- **Fingers → touchscreen**: pinch zoom, two-finger rotate and pan go to the computer as real multitouch, so canvas rotation works the way your app does it on a touch PC. You can switch this off.
- **Tap gestures**: two-finger tap undo, three-finger tap redo (configurable). Taps never reach the computer as touches.
- **Palm rejection** while the Pencil is down or hovering.
- **Toolbar**: Ctrl / Shift / Alt / Space that stay held while your finger is down, plus any shortcuts you like. Left or right edge.
- Pick the monitor, frame rate (up to 120) and bitrate from the iPad.

## Connecting

The iPad app is the server. Open it and leave it in front, then run the host:

- **USB** (lowest latency, charges the iPad): needs usbmuxd. On Windows that comes with the *Apple Devices* app (Microsoft Store) or iTunes. On Arch it's `pacman -S usbmuxd`. Trust the computer on the iPad once.
- **Wi-Fi**: the host finds the iPad with Bonjour. If that doesn't work (Bonjour can be blocked under LiveContainer), use the address the iPad shows: `inklink-host --host 192.168.x.x`.

The host tries USB first and reconnects on its own.

## Windows host

1. Install the GStreamer runtime from https://gstreamer.freedesktop.org/download/ (MSVC 64-bit, 1.24 or newer).
2. Run `inklink-host.exe` (from the *Host* workflow artifacts, or `cargo build --release` in `host/`).

Input injection can't reach apps running as administrator unless the host also runs as administrator.

**Tips**
- **Photoshop**: uses Windows Ink out of the box. Touch pan, zoom and rotate need *Enable Gestures* in Preferences → Tools.
- **Clip Studio Paint**: File → Preferences → Tablet → *TabletPC (Windows Ink)*. Touch gestures under *Preferences → Touch gesture*.
- A monitor shaped like the iPad avoids letterboxing. An open-source virtual display driver (e.g. *Virtual Display Driver* on GitHub) can add a 4:3 screen that you then pick in the app.

## Linux host

```sh
sudo pacman -S gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-plugin-pipewire gst-plugin-va xdg-desktop-portal usbmuxd
# uinput access for the virtual pen/touch/keyboard
echo 'KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"' | sudo tee /etc/udev/rules.d/70-inklink.rules
sudo usermod -aG input $USER   # then log out and back in
sudo modprobe uinput
cd host && cargo build --release && ./target/release/inklink-host
```

Your compositor asks once which screen to share. The choice is remembered in `~/.config/inklink/portal-token`. Works on KDE, GNOME, Hyprland, sway and others with a ScreenCast portal.

The virtual devices are called **InkLink Pen** and **InkLink Touch**. With more than one monitor, map them to the shared monitor in your compositor's tablet settings (KDE: *Drawing Tablet*; Hyprland: `input:tablet:output`).

## Building the iPad app (no Mac needed)

GitHub Actions builds an unsigned IPA on every push (`.github/workflows/ios.yml`). Download the `InkLink-ipa` artifact and install it with LiveContainer, or sign it with SideStore/Sideloadly. The Xcode project is generated from `ios/project.yml` with XcodeGen. Requires iPadOS 17+.

## Protocol

One TCP connection on port 47800. Messages are `[u8 kind][u32 LE length][payload]`. See `host/src/protocol.rs` and `ios/Sources/Protocol.swift`.

## License

MIT
