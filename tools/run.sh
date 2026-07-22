#!/usr/bin/env bash
# Builds (if needed) and boots moon OS in QEMU, with serial output on stdio.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ISO="$ROOT_DIR/build/moon-os.iso"
DISK="$ROOT_DIR/build/moon-os-disk.img"

"$ROOT_DIR/tools/build.sh"

# A real, persistent SATA disk backing fs::persist: files, accounts, and
# settings survive across `run.sh` invocations because this same image file
# is reused every time (only created once, left alone after that -- deleting
# it starts fresh, same as wiping a real drive). 64 MiB is generous for what
# a RAMFS snapshot at this project's current scale needs.
if [ ! -f "$DISK" ]; then
    echo "==> Creating persistent disk image: $DISK"
    qemu-img create -f raw "$DISK" 64M
fi

qemu-system-x86_64 \
    -M q35 \
    -m 256M \
    -device ich9-ahci,id=ahci0 \
    -drive if=none,id=cd0,file="$ISO",media=cdrom \
    -device ide-cd,drive=cd0,bus=ahci0.0 \
    -drive if=none,id=disk0,file="$DISK",format=raw \
    -device ide-hd,drive=disk0,bus=ahci0.1 \
    -netdev user,id=net0 -device rtl8139,netdev=net0 \
    -audiodev id=snd0,driver=none -device AC97,audiodev=snd0 \
    -serial stdio \
    -no-reboot -no-shutdown \
    "$@"
