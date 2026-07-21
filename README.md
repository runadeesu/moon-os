# moon OS

Windows・macOS・Linux・Android のいいところを参考にした、完全オリジナルの64bit OS。
学習用のトイOSではなく、実際に動作する完成品を目指す長期プロジェクトです。

現在のマイルストーンや今後の計画は [ROADMAP.md](ROADMAP.md) を参照してください。

## 現状 (M0/M1/M2/M3/M4 完了)

- 独自64bitカーネル (Rust, `no_std` / stable toolchain, ナイトリー不要)
- ブートローダーは [Limine](https://github.com/limine-bootloader/limine) を採用
  (BIOS/UEFI 両対応, 自作ではなく実績のあるOSSブートローダーを利用する設計判断。詳細は ROADMAP.md 参照)
- Limine Boot Protocol バインディングは自前実装 (`kernel/src/limine.rs`) — `limine` crate は
  nightly 限定の `ptr_metadata` feature に依存するため使わず、stable Rust で完結させています
- シリアル (COM1) ログ出力
- GDT / TSS (ダブルフォルト用 IST 付き)
- IDT + CPU例外ハンドラ (0〜31番, `#[naked]` トランポリン方式)
- フレームバッファへのテキストコンソール描画 (8x8 パブリックドメインフォント使用、自前スクロール実装)
- 物理メモリアロケータ (ビットマップ方式)、独自ページテーブル操作 (`map`/`unmap`/`translate`)、
  カーネルヒープ (`#[global_allocator]`、自前フリーリストアロケータ) — `Vec` / `Box` などが使用可能
- 8259 PIC リマップ + PIT タイマー割り込み (100Hz) + プリエンプティブなラウンドロビンスケジューラ
  (`kernel/src/sched.rs`) — 専用コンテキストスイッチコードなしで `iretq` の仕組みを流用
- PS/2 キーボード / マウスドライバ (`kernel/src/drivers/`) — IRQ1/IRQ12経由でスキャンコード・
  マウスパケットを受信し、シリアル/フレームバッファへエコー
- PCIバス列挙 + AHCI (SATA) ドライバ — HBA/ポート初期化、ATA IDENTIFY、ATAPI PACKET経由の
  セクタ読み込みに対応。起動用ISOイメージから実際にセクタを読み、ISO9660の"CD001"署名を確認済み
- 簡易VFS + RAMFS (`kernel/src/fs/`) — ファイルの書き込み/読み込み/一覧を実装
- QEMU (BIOS/UEFI 両方) での起動・ヒープ動作・マルチタスク・キーボード/マウス入力・
  ディスクI/Oを実機確認済み

## リポジトリ構成

```
moon-os/
├── Cargo.toml              # ワークスペース定義 + プロファイル設定
├── .cargo/config.toml      # x86_64-unknown-none をデフォルトターゲットに設定
├── ROADMAP.md               # 開発ロードマップ（マイルストーン管理）
├── boot/
│   └── limine.conf          # Limine ブートローダー設定
├── kernel/                  # カーネル本体 (Rust, no_std)
│   ├── Cargo.toml
│   ├── build.rs              # リンカスクリプトの指定
│   ├── linker.ld              # 上位半分 (higher-half) カーネル用リンカスクリプト
│   └── src/
│       ├── main.rs            # エントリポイント (kmain)
│       ├── limine.rs          # Limine Boot Protocol の自前バインディング
│       ├── font.rs             # 8x8 ビットマップフォント (パブリックドメイン)
│       ├── framebuffer.rs      # フレームバッファテキストコンソール
│       ├── arch/x86_64/
│       │   ├── mod.rs
│       │   ├── port.rs          # I/Oポートアクセス
│       │   ├── serial.rs        # 16550 UART (COM1) ドライバ
│       │   ├── gdt.rs           # GDT / TSS
│       │   ├── idt.rs           # IDT / CPU例外ハンドラ / IRQディスパッチ
│       │   ├── pic.rs           # 8259 PIC (リマップ + マスク制御)
│       │   └── pit.rs           # PIT タイマー (スケジューラ用ティック)
│       ├── memory/
│       │   ├── mod.rs            # HHDMオフセット管理・phys_to_virt
│       │   ├── pmm.rs             # 物理メモリアロケータ (ビットマップ)
│       │   ├── paging.rs          # ページテーブル操作 (map/unmap/translate)
│       │   ├── heap.rs            # カーネルヒープ (#[global_allocator])
│       │   └── mmio.rs             # デバイスMMIO領域のマッピング (PCI BAR用)
│       ├── drivers/
│       │   ├── mod.rs
│       │   ├── ps2.rs             # i8042 PS/2コントローラ アクセス
│       │   ├── keyboard.rs         # PS/2キーボード (スキャンコード→ASCII)
│       │   ├── mouse.rs            # PS/2マウス (3バイトパケット)
│       │   ├── pci.rs              # PCIバス列挙 (0xCF8/0xCFC)
│       │   └── ahci.rs             # AHCI (SATA) ドライバ
│       ├── fs/
│       │   ├── mod.rs             # VFS (現状はRAMFS一枚のマウント)
│       │   └── ramfs.rs            # インメモリファイルシステム
│       └── sched.rs               # プリエンプティブ・ラウンドロビンスケジューラ
└── tools/
    ├── build.sh              # カーネルビルド + ISO作成 (Limineは初回実行時に自動取得)
    └── run.sh                # ISOをQEMUで起動
```

## 開発環境のセットアップ

Windows 11 上で開発する場合、`xorriso` / `make` / `gcc` などLinux向けツールチェーンが
必要になるため、**WSL2 (Ubuntu) の利用を強く推奨**します。

### WSL2 (Ubuntu) 上でのセットアップ

```bash
# 必要パッケージ
sudo apt update
sudo apt install -y build-essential nasm xorriso mtools qemu-system-x86 git curl

# Rust (未インストールの場合)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# ベアメタルターゲットを追加 (stable channel でOK、nightly不要)
rustup target add x86_64-unknown-none
```

VS Code は WSL 拡張機能 (`Remote - WSL`) 経由でこの環境を直接開けます。
QEMU の画面表示には X11 サーバー (WSLg は標準で動作) が必要です。

## ビルド & 実行

```bash
# ビルドのみ (build/moon-os.iso が生成される)
./tools/build.sh

# ビルド + QEMU起動 (シリアル出力はそのままターミナルに流れます)
./tools/run.sh
```

初回実行時、`tools/build.sh` は Limine (バイナリリリース) を `third_party/limine/` に
`git clone` し、Limineのホスト側デプロイツール (`limine`) をその場でビルドします。
このディレクトリと `build/`, `target/` は `.gitignore` 対象で、リポジトリには含まれません。

正常に起動すると、シリアルコンソール (ターミナル) に以下のようなログが出力され、
QEMUのウィンドウにはフレームバッファコンソールでバナーが描画されます。

```
moon OS kernel booting...
bootloader: Limine 9.6.7
HHDM offset: 0xffff800000000000
pmm: 255 MiB total, 254 MiB free (65382 4K frames)
heap: mapped and handed to the global allocator
heap self-test: Vec<u32> of 16 squares, sum=1240
framebuffer: 1280x800 @ 32 bpp
ramfs: /hello.txt = "Hello from moon OS RAMFS!\n"
ramfs: files = ["/hello.txt"]
ahci: found controller 8086:2922 at 00:1f.2 (ABAR=0xfebd5000)
ahci: port 2 live, device = Atapi
ahci: port 2 ATAPI read of LBA16 succeeded, ISO9660 PVD signature: CD001 (verified!)
scheduler: 2 task(s) spawned
keyboard: IRQ1 unmasked
mouse: enabled, IRQ12 unmasked
interrupts enabled, 100 Hz timer running, entering idle loop
[task A] iteration 100000000
[task B] iteration 100000000
[task B] iteration 200000000
[task A] iteration 200000000
```

デバッグ用に、QEMUをGUIなしでシリアルログだけ確認したい場合:

```bash
./tools/build.sh
qemu-system-x86_64 -M q35 -m 256M -cdrom build/moon-os.iso -serial stdio -display none -no-reboot -no-shutdown
```

## 次の開発ステップ (M5: GUI / ウィンドウシステム)

- コンポジタ / ウィンドウマネージャー
- 描画プリミティブ (矩形、フォント、画像)
- イベントループ (マウス/キーボード入力の配送)

詳細は [ROADMAP.md](ROADMAP.md) を参照してください。
