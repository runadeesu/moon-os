#!/usr/bin/env bash
# Builds the moon OS kernel and packages it into a bootable ISO (BIOS + UEFI)
# using Limine. Limine itself is fetched (not vendored in git) into
# third_party/limine on first run.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$ROOT_DIR/build"
LIMINE_DIR="$ROOT_DIR/third_party/limine"
PROFILE="${PROFILE:-release}"

echo "==> Building userland/init (release)"
(cd "$ROOT_DIR/userland/init" && cargo build --release)
export USERLAND_INIT_ELF="$ROOT_DIR/userland/init/target/x86_64-unknown-none/release/init"

echo "==> Building kernel ($PROFILE)"
(cd "$ROOT_DIR" && cargo build -p moon_kernel --profile "$([ "$PROFILE" = release ] && echo release || echo dev)")

if [ "$PROFILE" = "release" ]; then
    KERNEL_BIN="$ROOT_DIR/target/x86_64-unknown-none/release/moon_kernel"
else
    KERNEL_BIN="$ROOT_DIR/target/x86_64-unknown-none/debug/moon_kernel"
fi

if [ ! -d "$LIMINE_DIR" ]; then
    echo "==> Fetching Limine bootloader (binary release, v9.x-binary)"
    git clone --depth=1 --branch v9.x-binary https://github.com/limine-bootloader/limine.git "$LIMINE_DIR"
fi

if [ ! -x "$LIMINE_DIR/limine" ]; then
    echo "==> Building Limine host deploy tool"
    make -C "$LIMINE_DIR"
fi

echo "==> Assembling ISO"
rm -rf "$BUILD_DIR/iso_root"
mkdir -p "$BUILD_DIR/iso_root/boot/limine"
mkdir -p "$BUILD_DIR/iso_root/EFI/BOOT"

cp "$KERNEL_BIN" "$BUILD_DIR/iso_root/boot/moon_kernel"
cp "$ROOT_DIR/boot/limine.conf" "$BUILD_DIR/iso_root/boot/limine/"
cp "$LIMINE_DIR/limine-bios.sys" "$LIMINE_DIR/limine-bios-cd.bin" "$LIMINE_DIR/limine-uefi-cd.bin" \
    "$BUILD_DIR/iso_root/boot/limine/"
cp "$LIMINE_DIR/BOOTX64.EFI" "$BUILD_DIR/iso_root/EFI/BOOT/"
cp "$LIMINE_DIR/BOOTIA32.EFI" "$BUILD_DIR/iso_root/EFI/BOOT/"

xorriso -as mkisofs -R -r -J -b boot/limine/limine-bios-cd.bin \
    -no-emul-boot -boot-load-size 4 -boot-info-table \
    --efi-boot boot/limine/limine-uefi-cd.bin \
    -efi-boot-part --efi-boot-image --protective-msdos-label \
    "$BUILD_DIR/iso_root" -o "$BUILD_DIR/moon-os.iso" >/dev/null

"$LIMINE_DIR/limine" bios-install "$BUILD_DIR/moon-os.iso"

echo "==> Done: $BUILD_DIR/moon-os.iso"
