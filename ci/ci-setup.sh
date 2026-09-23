#!/bin/sh
# What CI runs.
set -eu

export DEBIAN_FRONTEND=noninteractive
# Without these, apt stops for the needrestart prompt and the runner hangs.
export NEEDRESTART_MODE=a
export NEEDRESTART_SUSPEND=1

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

SUDO=
[ "$(id -u)" -eq 0 ] || SUDO=sudo

upstream_failed() {
    echo "::error::UPSTREAM-DEP-FAILED: $1"
    exit 1
}

ours_failed() {
    echo "::error::COSMIC-PRINTERS-FAILED: $1"
    exit 1
}

cmd_deps() {
    $SUDO apt-get update
    $SUDO apt-get install -y \
        cups avahi-daemon libnss-mdns \
        pkg-config clang libclang-dev libglib2.0-dev libxkbcommon-dev \
        build-essential autoconf libavahi-client-dev \
        libpng-dev libssl-dev zlib1g-dev \
        avahi-utils iproute2 ||
        upstream_failed "installing dependencies"
}

cmd_libcups() {
    ref=${1:-master}
    src="${RUNNER_TEMP:-/tmp}/libcups"

    rm -rf "$src"
    git clone --depth 1 --recurse-submodules --shallow-submodules \
        --branch "$ref" https://github.com/OpenPrinting/libcups.git "$src" ||
        upstream_failed "cloning libcups $ref"

    cd "$src"
    ./configure --prefix=/usr/local --with-domainsocket=/run/cups/cups.sock ||
        upstream_failed "configuring libcups $ref"
    make -j"$(nproc)" || upstream_failed "building libcups $ref"
    $SUDO make install || upstream_failed "installing libcups $ref"
    $SUDO ldconfig

    pkg-config --modversion cups3 || upstream_failed "cups3 is not on PKG_CONFIG_PATH"
}

cmd_services() {
    $SUDO systemctl start avahi-daemon || upstream_failed "starting avahi-daemon"
    $SUDO systemctl start cups || upstream_failed "starting cups"
}

cmd_test() {
    cd "$repo_root"

    cargo clippy --workspace --all-targets --locked || ours_failed "clippy"
    cargo test --workspace --locked || ours_failed "unit tests"

    ci/fixtures.sh start || upstream_failed "starting the fixtures"
    ci/fixtures.sh wait || upstream_failed "the fixtures never advertised themselves"

    cargo test -p cosmic-settings-printers-server --locked \
        -- --ignored --test-threads=1 || ours_failed "environment tests"
}

case "${1:-}" in
deps) cmd_deps ;;
libcups) cmd_libcups "${2:-master}" ;;
services) cmd_services ;;
test) cmd_test ;;
*)
    echo "usage: $0 {deps|libcups [ref]|services|test}" >&2
    exit 1
    ;;
esac
