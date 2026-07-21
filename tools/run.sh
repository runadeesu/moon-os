#!/usr/bin/env bash
# Builds (if needed) and boots moon OS in QEMU, with serial output on stdio.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO="$ROOT_DIR/build/moon-os.iso"

"$ROOT_DIR/tools/build.sh"

qemu-system-x86_64 \
    -M q35 \
    -m 256M \
    -cdrom "$ISO" \
    -serial stdio \
    -no-reboot -no-shutdown \
    "$@"
