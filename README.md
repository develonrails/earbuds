<p align="center">
  <img src="data/icons/io.github.develonrails.Earbuds.svg" width="128" alt="">
</p>

<h1 align="center">Earbuds</h1>

<p align="center">Noise control, equalizer and battery for JBL earbuds, in a small native GNOME window.</p>

JBL only makes its Headphones app for phones. Earbuds does the same on your
computer: it talks to the earbuds over Bluetooth directly, written in Rust with
GTK 4 and libadwaita.

## What Earbuds stands for

**Frugal.** All Bluetooth work happens on one thread that sleeps until BlueZ or
the window has something to say: no polling while nothing is connected, and a
battery read every two minutes while something is. Commands are paced, and a
dragged equalizer slider sends where it stopped, not every step it passed.

**Native.** A GNOME app, following the Human Interface Guidelines, your light
or dark style and accent colour, and shipped as a Flatpak.

**Honest about the model.** Every setting the window shows is one the earbuds
answered for when they connected. A setting they decline without saying so (a
Wave Buds 2 refuses noise cancelling with one bud in the case) is reported,
not pretended.

**Private.** No account, no internet, no phone.

## Features

- **Battery** of the left and right earbud and the case
- **Noise control:** noise cancelling, Ambient Aware, TalkThru or off
- **Equalizer:** JBL's presets (Jazz, Vocal, Bass, Club, Studio, Extreme Bass),
  or your own ten-band curve
- **VoiceAware**, voice prompts, Safe Sound and automatic power off
- **Find my earbuds:** make the left or right one beep

Made for, and tested on, the **JBL Wave Buds 2**. Other JBL earbuds of the same
generation (Vibe Buds 2, Tune Buds 2, Tune Beam 2, Tune Flex 2, Wave Flex 2,
Live 770NC, Tour Pro 2, …) speak the same protocol and should work; they show
the settings they answer for. The newest lines (Tune 780NC, Live x80NC, Tour
Pro 4) use a protocol that has not been decoded yet.

## Install

Earbuds comes as a Flatpak from its own signed repository on GitHub Pages.
Add it once:

```sh
flatpak remote-add --user --if-not-exists --from \
    earbuds https://develonrails.github.io/earbuds/earbuds.flatpakrepo
flatpak install --user earbuds io.github.develonrails.Earbuds
```

After that, `flatpak update` and GNOME Software pick up new builds like any
other Flatpak. A single-file bundle is attached to the
[`continuous` release](https://github.com/develonrails/earbuds/releases/tag/continuous)
for installing without a remote, but it cannot update itself.

Pair and connect the earbuds in GNOME's Bluetooth settings first, as usual;
Earbuds does not pair.

## Building

On Fedora, or in a Fedora toolbox:

```sh
sudo dnf install rust cargo gcc pkgconf-pkg-config gtk4-devel libadwaita-devel
cargo run
```

The Flatpak builds offline from vendored crates:

```sh
cargo vendor --locked vendor > build-aux/flatpak/cargo-config.toml
flatpak run org.flatpak.Builder --user --install --force-clean \
    build-dir build-aux/flatpak/io.github.develonrails.Earbuds.yml
```

Run the tests with `cargo test`. CI also runs `cargo fmt` and
`cargo clippy -- -D warnings`. [CONTRIBUTING](docs/CONTRIBUTING.md) has the
rest.

## How it is put together

The earbuds have two Bluetooth identities. The paired headset carries the
audio; the settings live on a second, Bluetooth LE identity named
"<name>-LE", at a random address that changes now and then. Earbuds waits for
BlueZ to report a connected JBL headset, scans a few seconds for its LE twin,
and opens the ATT channel to it directly. BlueZ's own GATT client is refused
by the earbuds, so Earbuds brings a small one of its own.

| | |
|---|---|
| `src/protocol.rs` | JBL's framed protocol: building requests and reading the answers. No I/O; tested on frames captured from a Wave Buds 2. |
| `src/att.rs` | A minimal ATT client over an L2CAP socket. |
| `src/bluez.rs` | BlueZ over D-Bus (zbus): the connected headset, the scan, and keeping bluetoothd off the link. |
| `src/engine.rs` | The Bluetooth thread: connect, read everything, serve commands. |
| `src/state.rs`, `src/window.rs` | What the window shows, and the window. |

## Thanks

The protocol, and the way to the control link, come from
[jbl-control](https://github.com/AhmadRun/jbl-control) by Ahmad Elyan (MIT),
which worked them out on a Wave Buds 2, crediting earlier work by
[omarchy-headphones](https://github.com/ncr/omarchy-headphones),
[jbl-vibe-buds-2-gnome](https://github.com/Asklios/jbl-vibe-buds-2-gnome),
[JblTray](https://github.com/emperorbug46/JblTray) and
[JBL-Headphones-App](https://github.com/FajkPes/JBL-Headphones-App).

If Earbuds is useful to you, you can [buy me a coffee](https://ko-fi.com/apenzoon).

## License

[MIT](LICENSE).

Earbuds is an independent project. It is not affiliated with, endorsed by or
sponsored by JBL or Harman. JBL is a trademark of Harman International.
