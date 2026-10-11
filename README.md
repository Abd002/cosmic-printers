# cosmic-printers

Printer support for COSMIC. It provides reusable printer crates, the COSMIC Settings printer page, and a standalone application.

The project uses IPP through `libcups3`.

## Crates

- `printers-core`: Shared printer types and grouping logic.
- `printers-client`: Async client for the printers daemon.
- `printers-server`: CUPS, IPP, DNS-SD, and Printer Application backend.
- `printers-ui`: Printer list, details, queue, and Add Printer views.
- `printers-app`: Standalone `cosmic-printers` application.

The UI normally connects to the printers daemon. The standalone application can instead run the server in-process when the daemon is unavailable. Both modes use `printers-server` and require `libcups3`.

Set `COSMIC_PRINTERS_BACKEND=daemon` or `COSMIC_PRINTERS_BACKEND=embedded` to force a backend.

## Runtime requirements

On Debian/Ubuntu:

```sh
sudo apt-get install -y cups avahi-daemon
```

Disable `cups-browsed`. It creates its own queues for printers found on the network, which show up next to the printers the app already finds:

```sh
sudo systemctl disable --now cups-browsed
```

## Build dependencies

On Debian/Ubuntu:

```sh
sudo apt-get install -y \
  pkg-config \
  clang \
  libclang-dev \
  libglib2.0-dev \
  libxkbcommon-dev \
  build-essential \
  autoconf \
  libavahi-client-dev \
  libnss-mdns \
  libpng-dev \
  libssl-dev \
  zlib1g-dev
```

Requires `libcups3` from OpenPrinting/libcups:

```sh
git clone --recurse-submodules https://github.com/OpenPrinting/libcups.git
cd libcups

./configure \
  --prefix=/usr/local \
  --with-domainsocket=/run/cups/cups.sock

make -j"$(nproc)"
sudo make install
sudo ldconfig
```

On Arch Linux, nothing searches `/usr/local` by default: pkg-config won't find `cups3` when you build, and the dynamic loader won't find `libcups3.so.3` when you run the app. Fix both:

```sh
echo /usr/local/lib | sudo tee /etc/ld.so.conf.d/usr-local.conf
sudo ldconfig
export PKG_CONFIG_PATH=/usr/local/lib/pkgconfig
```

## Building

Clone the repository and enter it:

```sh
git clone https://github.com/Abd002/cosmic-printers.git
cd cosmic-printers
```

Check the core workspace members:

```sh
cargo check
```

Check the entire workspace:

```sh
cargo check --workspace --all-targets
```

Run the standalone application:

```sh
cargo run -p printers
```
