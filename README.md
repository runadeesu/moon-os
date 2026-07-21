# moon OS

Windows・macOS・Linux・Android のいいところを参考にした、完全オリジナルの64bit OS。
学習用のトイOSではなく、実際に動作する完成品を目指す長期プロジェクトです。

現在のマイルストーンや今後の計画は [ROADMAP.md](ROADMAP.md) を参照してください。

## 現状 (M0〜M6 基本部分 完了)

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
- ウィンドウシステム (`kernel/src/gui/`) — コンポジタ/ウィンドウマネージャー、
  クリックでのフォーカス切り替え、タイトルバードラッグでのウィンドウ移動に対応。
  ターミナル(help/clear/uptime/mem/echoコマンド)と設定(ライブシステム情報)の
  組み込みウィジェットを搭載 — まだユーザーモードが無いため「アプリ」はカーネル内蔵
- デスクトップの見た目 (ダーク×月/宇宙テーマ) — 手続き型生成の壁紙 (夜空グラデーション、
  星、月、山のシルエット)、上部ステータスバー(ロゴ・実CMOS RTCからのライブ時計・
  ネットワーク状態)、左サイドのドック(旧・下部タスクバーを置き換え、クリックで
  ウィンドウ切り替え)、右上にカレンダー(実日付、Zellerの公式で曜日計算)と
  システムモニターの半透明ウィジェットパネル。半透明表現は `framebuffer::blend_rect`
  による実アルファブレンド(既存ピクセルを読み戻して合成)
- 多言語UI (`kernel/src/i18n.rs`) — 設定ウィンドウの「Lang」行をクリックすると
  英語/日本語/スペイン語/フランス語を切り替え可能(切り替えると即座に再描画される)。
  日本語ラベルは`kernel/src/font_hiragana.rs`(dhepper/font8x8由来のパブリックドメイン
  ひらがなビットマップ96字)を使い全てひらがな表記 — 漢字/カタカナ用フォントはまだ無いため
- 文字描画のソフト化 — `framebuffer::draw_char_at` が1bitグリフの縁に低アルファのハロー
  (縁取りブレンド)を追加し、擬似アンチエイリアスでカクカク感を軽減
- 未来感のあるUIクロム — ウィンドウ枠・ドック・トップバー・デスクトップウィジェットに
  `framebuffer::glow_border`(同心円状の半透明アウトライン)によるネオングロー、
  フォーカスウィンドウの四隅に`draw_corner_brackets`(HUD風Lブラケット)、
  壁紙全体にごく薄いスキャンライン効果を追加
- RTL8139 NICドライバ + 自前ネットワークスタック (`kernel/src/net/`) — Ethernet/ARP/IPv4/
  ICMP/UDPを実装。DHCPクライアントでQEMU SLIRPから実際にIPアドレスを取得し、
  ゲートウェイへのICMP ping・DNS問い合わせ(example.comの実際の名前解決)まで成功
  (TCPは大規模なため今回は未実装。ROADMAP.md参照)
- QEMU (BIOS/UEFI 両方) での起動・ヒープ動作・マルチタスク・キーボード/マウス入力・
  ディスクI/O・ウィンドウのドラッグ操作/フォーカス切り替え/ターミナル操作・
  DHCP/ping/DNSによる実ネットワーク往復を実機確認済み

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
│       ├── font.rs             # 8x8 ASCIIビットマップフォント (パブリックドメイン)
│       ├── font_hiragana.rs    # 8x8 ひらがなビットマップフォント (パブリックドメイン)
│       ├── i18n.rs             # UI多言語対応 (英語/日本語/スペイン語/フランス語)
│       ├── framebuffer.rs      # フレームバッファ描画 (blend/glow/ソフトテキスト含む)
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
│       │   ├── ahci.rs             # AHCI (SATA) ドライバ
│       │   ├── rtl8139.rs          # RTL8139 NICドライバ
│       │   └── rtc.rs              # CMOS RTC (実時刻読み取り)
│       ├── fs/
│       │   ├── mod.rs             # VFS (現状はRAMFS一枚のマウント)
│       │   └── ramfs.rs            # インメモリファイルシステム
│       ├── gui/
│       │   ├── mod.rs             # コンポジタ本体・入力イベント処理
│       │   ├── window.rs           # Window構造体 (位置/サイズ/タイトル/内容)
│       │   ├── desktop.rs          # 壁紙 (夜空グラデーション/星/月/山)
│       │   ├── desktop_widgets.rs  # カレンダー・システムモニターの固定パネル
│       │   ├── topbar.rs           # 上部ステータスバー (ロゴ/時計/ネット状態)
│       │   ├── dock.rs             # 左サイドドック (旧タスクバー)
│       │   └── widgets/
│       │       ├── terminal.rs      # ターミナルウィジェット (組み込みコマンド)
│       │       └── settings.rs      # 設定ウィジェット (ライブシステム情報)
│       ├── net/
│       │   ├── mod.rs             # NIC状態管理・送受信・デモ実行
│       │   ├── arp.rs              # ARP (要求/応答/キャッシュ)
│       │   ├── ipv4.rs             # IPv4ヘッダ・チェックサム・送信ルーティング
│       │   ├── icmp.rs             # ICMP echo (ping送受信)
│       │   ├── udp.rs              # UDP送受信 + ポート別受信箱
│       │   ├── dhcp.rs             # DHCPクライアント
│       │   └── dns.rs              # 簡易DNSリゾルバ (Aレコードのみ)
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
gui: initialized, 1280x800
ramfs: /hello.txt = "Hello from moon OS RAMFS!\n"
ramfs: files = ["/hello.txt"]
ahci: found controller 8086:2922 at 00:1f.2 (ABAR=0xfebd5000)
ahci: port 2 live, device = Atapi
ahci: port 2 ATAPI read of LBA16 succeeded, ISO9660 PVD signature: CD001 (verified!)
rtl8139: found controller 10ec:8139 at 00:02.0 (io_base=0xc000)
rtl8139: mac=52:54:00:12:34:56
net: mac=52:54:00:12:34:56
dhcp: DISCOVER sent
dhcp: OFFER 10.0.2.15
dhcp: REQUEST sent
dhcp: ACK, lease = 10.0.2.15
net: configured ip=10.0.2.15 mask=255.255.255.0 gateway=10.0.2.2 dns=10.0.2.3
icmp: echo reply from 10.0.2.2 seq=1
net: ping to gateway 10.0.2.2 succeeded
net: DNS example.com -> 104.20.23.154
scheduler: 2 task(s) spawned
keyboard: IRQ1 unmasked
mouse: enabled, IRQ12 unmasked
interrupts enabled, 100 Hz timer running, entering idle loop
[task A] iteration 100000000
[task B] iteration 100000000
[task B] iteration 200000000
[task A] iteration 200000000
```

`tools/run.sh` は RTL8139 NIC (`-netdev user -device rtl8139`) を自動的に接続します。
デバッグ用に、QEMUをGUIなしでシリアルログだけ確認したい場合 (NICも付ける場合):

```bash
./tools/build.sh
qemu-system-x86_64 -M q35 -m 256M -cdrom build/moon-os.iso \
    -netdev user,id=net0 -device rtl8139,netdev=net0 \
    -serial stdio -display none -no-reboot -no-shutdown
```

## 次の開発ステップ (M7: パッケージ管理 / アプリ基盤)

- moon OS ネイティブアプリの実行形式定義
- ユーザーモード + ELFローダー
- パッケージマネージャー

詳細は [ROADMAP.md](ROADMAP.md) を参照してください。
