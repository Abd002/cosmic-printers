#!/bin/sh
# Two IPP printers for the environment tests. They advertise themselves over
# DNS-SD and no queue is configured for them, which is what is being tested.
set -eu

state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/cosmic-printers-ci"
pid_dir="$state_dir/pids"

printer_a="CI Test Printer A"
printer_b="CI Test Printer B"
port_a=8801
port_b=8802

start_one() {
    port=$1
    name=$2

    ippeveprinter -p "$port" "$name" >"$state_dir/$port.log" 2>&1 &
    echo $! >"$pid_dir/$port.pid"
    echo "Started $name on port $port"
}

start() {
    mkdir -p "$pid_dir"
    command -v ippeveprinter >/dev/null || {
        echo "ippeveprinter is required; install libcups3" >&2
        exit 1
    }

    start_one "$port_a" "$printer_a"
    start_one "$port_b" "$printer_b"
}

# On the advertisement rather than the port, since that is what is waited for.
wait_ready() {
    for name in "$printer_a" "$printer_b"; do
        ippfind -T 30 --literal-name "$name" --exec true \; || {
            echo "$name never appeared over DNS-SD" >&2
            exit 1
        }
        echo "Found $name"
    done
}

# Only what this started; other fixtures are usually running beside it.
stop() {
    [ -d "$pid_dir" ] || return 0

    for pid_file in "$pid_dir"/*.pid; do
        [ -f "$pid_file" ] || continue
        kill "$(cat "$pid_file")" 2>/dev/null || true
        rm -f "$pid_file"
    done
    echo "Stopped the fixtures"
}

case "${1:-start}" in
start) start ;;
wait) wait_ready ;;
stop) stop ;;
*)
    echo "usage: $0 [start|wait|stop]" >&2
    exit 1
    ;;
esac
