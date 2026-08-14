#!/bin/sh
set -eu

workspace=${AXBUILD_WORKSPACE_ROOT:?}
case_dir=${AXBUILD_CASE_DIR:?}
rtthread_commit=9657b3ce1070e1c83479f011bd09d84a922072f2
output_dir="$workspace/tmp/axbuild/axvisor/qemu-rtthread"
output="$output_dir/rtthread.bin"
stamp="$output_dir/source.sha256"
native_runner="$case_dir/run-native-baseline.sh"

source_repo=${AXVISOR_RTTHREAD_SOURCE:-"$workspace/tmp/axbuild/axvisor/rt-thread-source"}
toolchain_dir=${AXVISOR_RTTHREAD_TOOLCHAIN:-}
if [ -z "$toolchain_dir" ]; then
    compiler=$(command -v aarch64-none-elf-gcc || true)
    if [ -n "$compiler" ]; then
        toolchain_dir=$(dirname "$compiler")
    fi
fi
if [ -z "$toolchain_dir" ] || [ ! -x "$toolchain_dir/aarch64-none-elf-gcc" ]; then
    echo "missing AArch64 RT-Thread toolchain; set AXVISOR_RTTHREAD_TOOLCHAIN" >&2
    exit 1
fi

if [ ! -d "$source_repo/.git" ]; then
    git clone https://github.com/RT-Thread/rt-thread.git "$source_repo"
fi
if ! git -C "$source_repo" cat-file -e "$rtthread_commit^{commit}" 2>/dev/null; then
    git -C "$source_repo" fetch origin "$rtthread_commit"
fi

mkdir -p "$output_dir"
source_hash=$(
    {
        printf '%s\n' "$rtthread_commit"
        sha256sum "$case_dir/prepare.sh" "$case_dir/rtthread/rtprobe.c" \
            "$case_dir/rtthread/virtual-timer.patch"
        "$toolchain_dir/aarch64-none-elf-gcc" --version | sed -n '1p'
    } | sha256sum | awk '{print $1}'
)
if [ -f "$output" ] && [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$source_hash" ]; then
    echo "RT-Thread test image is up to date at $output"
else
    build_root=$(mktemp -d "$output_dir/build.XXXXXX")
    trap 'rm -rf "$build_root"' EXIT HUP INT TERM
    source_dir="$build_root/rt-thread"
    mkdir -p "$source_dir"
    git -C "$source_repo" archive "$rtthread_commit" | tar -x -C "$source_dir"
    patch -d "$source_dir" -p1 < "$case_dir/rtthread/virtual-timer.patch"

    bsp="$source_dir/bsp/qemu-virt64-aarch64"
    cp "$case_dir/rtthread/rtprobe.c" "$bsp/applications/rtprobe.c"
    sed -i 's/^CONFIG_RT_CPUS_NR=.*/CONFIG_RT_CPUS_NR=1/' "$bsp/.config"
    sed -i 's/^#define RT_CPUS_NR .*/#define RT_CPUS_NR 1/' "$bsp/rtconfig.h"

    RTT_CC=gcc \
    RTT_CC_PREFIX=aarch64-none-elf- \
    RTT_EXEC_PATH="$toolchain_dir" \
    PATH="$toolchain_dir:$PATH" \
    scons -C "$bsp" -j"${AXVISOR_RTTHREAD_JOBS:-2}"

    test -s "$bsp/rtthread.bin"
    cp "$bsp/rtthread.bin" "$output"
    printf '%s\n' "$source_hash" > "$stamp"
    echo "prepared RT-Thread test image at $output"
    rm -rf "$build_root"
    trap - EXIT HUP INT TERM
fi

sh "$native_runner" "$output"
