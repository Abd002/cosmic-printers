#!/bin/sh
# Two IPP printers for the environment tests. They advertise themselves over
# DNS-SD and no queue is configured for them, which is what is being tested.
# A third one has a queue in front of it, for what a queue says about its supplies.
set -eu

state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/cosmic-printers-ci"
pid_dir="$state_dir/pids"

printer_a="CI Test Printer A"
printer_b="CI Test Printer B"
port_a=8801
port_b=8802

supply_printer="CI Supply Printer"
supply_port=8803
supply_queue=CI_Supply_Queue

SUDO=
[ "$(id -u)" -eq 0 ] || SUDO=sudo

start_one() {
    port=$1
    name=$2
    shift 2

    ippeveprinter -p "$port" "$@" "$name" >"$state_dir/$port.log" 2>&1 &
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

    # One name holds a comma, as real printers' supply names sometimes do.
    cat >"$state_dir/supplies.attrs" <<'EOF'
ATTR textWithoutLanguage printer-make-and-model "CI Supply Printer"
ATTR boolean color-supported true
ATTR mimeMediaType document-format-supported application/pdf,image/pwg-raster,image/urf
ATTR name marker-names "Black Toner, High Yield","Cyan Toner","Magenta Toner","Yellow Toner"
ATTR name marker-colors "#000000","#00FFFF","#FF00FF","#FFFF00"
ATTR integer marker-levels 70,50,30,10
ATTR keyword marker-types toner,toner,toner,toner
EOF
    start_one "$supply_port" "$supply_printer" -r off -a "$state_dir/supplies.attrs"
}

# lpadmin needs a scheduler admin, which a member of lpadmin already is.
cups_admin() {
    lpadmin "$@" 2>/dev/null || $SUDO lpadmin "$@"
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

    # A queue only has its printer's supplies once a job has been sent through it.
    cups_admin -p "$supply_queue" -E -v "ipp://localhost:$supply_port/ipp/print" \
        -m drv:///sample.drv/generic.ppd
    lp -d "$supply_queue" /usr/share/cups/data/default-testpage.pdf >/dev/null
    tries=0
    until lpoptions -p "$supply_queue" | grep -q marker-levels; do
        tries=$((tries + 1))
        [ "$tries" -lt 30 ] || {
            echo "$supply_queue never got its printer's supplies" >&2
            exit 1
        }
        sleep 1
    done
    echo "Found the supplies of $supply_queue"
}

# Only what this started; other fixtures are usually running beside it.
stop() {
    [ -d "$pid_dir" ] || return 0

    for pid_file in "$pid_dir"/*.pid; do
        [ -f "$pid_file" ] || continue
        kill "$(cat "$pid_file")" 2>/dev/null || true
        rm -f "$pid_file"
    done
    if lpstat -p "$supply_queue" >/dev/null 2>&1; then
        cups_admin -x "$supply_queue" || true
    fi
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
