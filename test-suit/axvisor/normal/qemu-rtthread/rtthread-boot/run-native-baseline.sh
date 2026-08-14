#!/bin/sh
set -eu

image=${1:?usage: run-native-baseline.sh RTTHREAD_IMAGE}
timeout_seconds=${AXVISOR_RTTHREAD_NATIVE_TIMEOUT:-780}
host_cpu=${AXVISOR_RTTHREAD_NATIVE_CPU:-}
result_dir=$(mktemp -d "${TMPDIR:-/tmp}/axvisor-rtthread-native.XXXXXX")
fifo="$result_dir/serial.fifo"
log="$result_dir/native.log"
qemu_pid=
tee_pid=

cleanup()
{
    if [ -n "$qemu_pid" ] && kill -0 "$qemu_pid" 2>/dev/null; then
        kill "$qemu_pid" 2>/dev/null || true
    fi
    if [ -n "$tee_pid" ] && kill -0 "$tee_pid" 2>/dev/null; then
        kill "$tee_pid" 2>/dev/null || true
    fi
    rm -rf "$result_dir"
}
trap cleanup EXIT HUP INT TERM

test -s "$image"
command -v qemu-system-aarch64 >/dev/null
command -v stdbuf >/dev/null
mkfifo "$fifo"

set -- stdbuf -oL -eL qemu-system-aarch64 \
    -nographic \
    -cpu cortex-a72 \
    -machine virt,gic-version=3 \
    -smp 1 \
    -m 128M \
    -kernel "$image" \
    -append "console=ttyAMA0 earlycon cma=8M coherent_pool=2M rw pic.gicv3_eoimode=1 rtprobe.profile=closure rtprobe.platform=native"
if [ -n "$host_cpu" ]; then
    command -v taskset >/dev/null
    set -- taskset -c "$host_cpu" "$@"
    echo "native RT-Thread host CPU affinity: $host_cpu"
else
    echo "native RT-Thread host CPU affinity: inherited"
fi
"$@" >"$fifo" 2>&1 &
qemu_pid=$!
tee "$log" <"$fifo" &
tee_pid=$!

started=$(date +%s)
state=running
while kill -0 "$qemu_pid" 2>/dev/null; do
    if grep -Eq '^RTTHREAD_RTPROBE_CLOSURE_PASSED platform=native[[:space:]]*$' "$log"; then
        state=passed
        break
    fi
    if grep -Eiq '(^rtprobe (smoke|closure) failed|kernel panic|hard fault|assertion failed)' "$log"; then
        state=failed
        break
    fi
    now=$(date +%s)
    if [ "$((now - started))" -ge "$timeout_seconds" ]; then
        state=timeout
        break
    fi
    sleep 1
done

if kill -0 "$qemu_pid" 2>/dev/null; then
    kill "$qemu_pid" 2>/dev/null || true
fi
wait "$qemu_pid" 2>/dev/null || true
wait "$tee_pid" 2>/dev/null || true

if grep -Eq '^RTTHREAD_RTPROBE_CLOSURE_PASSED platform=native[[:space:]]*$' "$log"; then
    state=passed
fi
if [ -n "${AXVISOR_RTTHREAD_NATIVE_LOG:-}" ]; then
    mkdir -p "$(dirname "$AXVISOR_RTTHREAD_NATIVE_LOG")"
    cp "$log" "$AXVISOR_RTTHREAD_NATIVE_LOG"
fi
if [ "$state" != passed ]; then
    echo "native RT-Thread baseline did not pass: $state" >&2
    exit 1
fi

echo "native RT-Thread baseline passed"
