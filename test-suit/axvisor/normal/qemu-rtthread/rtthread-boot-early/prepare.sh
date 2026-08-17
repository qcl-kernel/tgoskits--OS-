#!/bin/sh
set -eu

workspace=${AXBUILD_WORKSPACE_ROOT:?}
output="$workspace/tmp/axbuild/axvisor/qemu-rtthread/rtthread.bin"
stamp="$workspace/tmp/axbuild/axvisor/qemu-rtthread/source.sha256"

# The full closure prepare step owns image construction and native validation.
# Early-boot evidence must consume only that verified, pinned image.
test -s "$output"
test -s "$stamp"
printf "%s\n" "using verified RT-Thread image at $output"
