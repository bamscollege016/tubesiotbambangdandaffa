tubesiotbambangdandaffa

# ESP32-S3 Rust DHT22 → ThingsBoard (Telemetry + OTA via RPC)

Proyek ini menjalankan **ESP32-S3** dengan **Rust + ESP-IDF**, membaca sensor **DHT22**, mengirim telemetry ke **ThingsBoard Cloud** via **MQTT**, sinkronisasi waktu via **NTP**, dan mendukung **OTA update** melalui **Client-Side RPC**.

## Fitur

* 📡 MQTT ke ThingsBoard (`v1/devices/me/telemetry`)
* 🌡️ Baca **DHT22** (temperature & humidity) tiap 60 s
* ⏱️ NTP time sync → timestamp & waktu WIB
* 🔁 **OTA firmware update** lewat RPC (param `ota_url`)
* 🧪 Logging terstruktur (melaporkan `fw_version`, `fw_state`)

---

## 1) Prasyarat

* **Hardware**

  * ESP32-S3 (contoh: ESP32-S3-DevKitC)
  * Sensor **DHT22**
  * Kabel jumper

* **Software & Toolchain**

  * Rust toolchain (stable) + `cargo`
  * **espup** (untuk setup ESP-IDF + toolchain Xtensa)
  * **ESP-IDF** (otomatis via `espup`)
  * **espflash** (flash & serial monitor)
  * (Opsional) VS Code + rust-analyzer

> **Install cepat (Linux/macOS/WSL):**
>
> ```bash
> cargo install espup espflash
> espup install
> # setelah selesai, load env (perintah ini biasanya tertulis di akhir output espup):
> source "$HOME/export-esp.sh"
> ```
>
> **Windows (PowerShell):** gunakan terminal “ESP-IDF PowerShell”/“Developer PowerShell” dan jalankan perintah setara. Pastikan driver USB-UART sudah terpasang.

---

## 2) Wiring DHT22 → ESP32-S3

| DHT22 |  ESP32-S3 | Keterangan                          |
| ----: | :-------: | ----------------------------------- |
|   VCC |    3V3    | 3.3V                                |
|   GND |    GND    | Ground                              |
|  DATA | **GPIO4** | **Wajib**: kode menggunakan `gpio4` |

> **Catatan:** beberapa modul DHT22 butuh resistor pull-up 10kΩ dari **DATA** ke **3V3** (kebanyakan modul board DHT22 sudah ada).

---

## 3) Konfigurasi di Kode

Buka file sumber (mis. `src/main.rs`) dan sesuaikan:

* **WiFi SSID & Password**

  ```rust
  // ganti sesuai WiFi kamu
  ssid.push_str("▶️ SSID_WIFI_KAMU ◀️").unwrap();
  pass.push_str("▶️ PASSWORD_WIFI_KAMU ◀️").unwrap();
  ```

* **ThingsBoard**

  ```rust
  const TB_MQTT_URL: &str = "mqtt://mqtt.thingsboard.cloud:1883";
  const THINGSBOARD_TOKEN: &str = "▶️ TOKEN_DEVICE_TB ◀️"; // Device → Copy access token
  ```

* **Versi Firmware (untuk OTA)**

  ```rust
  const CURRENT_FIRMWARE_VERSION: &str = "PaceP-s3-v2.0";
  ```

> **Keamanan:** jangan commit token WiFi/ThingsBoard ke repo publik. Gunakan `.env`/`secrets` jika perlu.

---

## 4) Persiapan Proyek Rust

**Struktur minimal:**

```
your-project/
├─ Cargo.toml
└─ src/
   └─ main.rs
```

**Contoh `Cargo.toml` (sesuaikan versi crate jika perlu):**

```toml
[package]
name = "esp32s3-dht22-tb-ota"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
chrono = { version = "0.4", default-features = false, features = ["clock"] }
dht-sensor = "0.2"
esp-idf-svc = { version = "0.48", features = ["alloc"] }
embedded-svc = "0.26"
heapless = "0.8"
serde_json = "1"
log = "0.4"

[build-dependencies]
embuild = "0.31"

[package.metadata.esp-idf-sys]
esp_idf_version = "latest"
```

**Target & env (Xtensa ESP-IDF std):**
`espup` sudah men-setup target **`xtensa-esp32s3-espidf`**. Pastikan sesi terminal kamu sudah `source export-esp.sh` (Linux/macOS) atau memakai shell yang sudah memuat environment (Windows).

---

## 5) Build, Flash, dan Monitor

1. **Build:**

   ```bash
   cargo build --release
   ```

2. **Cek board (opsional):**

   ```bash
   espflash board-info --port ▶️/dev/ttyACM0◀️
   # Windows contoh: --port COM4
   ```

3. **Flash + monitor (auto reset):**

   ```bash
   espflash flash target/xtensa-esp32s3-espidf/release/esp32s3-dht22-tb-ota --monitor --port ▶️/dev/ttyACM0◀️
   ```

> Jika port berbeda, ganti sesuai hasil `Device Manager` (Windows) / `ls /dev/tty*` (Linux).

---

## 6) Setup ThingsBoard Cloud

1. Buat akun di **ThingsBoard Cloud**.
2. **Devices → Add new device →** beri nama (mis. `esp32-rust-ota`).
3. Buka device → **Copy access token** → tempel ke `THINGSBOARD_TOKEN`.
4. (Opsional) Buat **Dashboard** dan **Timeseries** untuk `temperature`, `humidity`, dll.

**Format telemetry yang dikirim:**

```json
{
  "send_time": "YYYY-MM-DD HH:MM:SS",   // WIB
  "ts": 173... ,                        // epoch millis (untuk ThingsBoard ts)
  "temperature": 26.5,
  "humidity": 60.2,
  "fw_version": "PaceP-s3-v2.0",        // dikirim terpisah via telemetry awal
  "fw_state": "IDLE|DOWNLOADING|VERIFYING|SUCCESS|FAILED" // status OTA
}
```

> Kode mengirim `fw_version` & `fw_state` saat startup/OTA.

---

## 7) Menjalankan OTA via Client-Side RPC

Firmware baru harus berupa **binary** yang kompatibel (hasil `cargo build --release`, di-pack oleh ESP-IDF). Host file `.bin` tersebut di HTTP server yang dapat diakses (contoh: file server lokal, GitHub Release asset, atau object storage).

**Di ThingsBoard (Device → RPC):**

* Pilih **Client-side RPC**
* **Method** bebas (kode tidak mengecek nama metoda)
* **Params** (harus ada `ota_url`):

  ```json
  {
    "ota_url": "http://▶️server-kamu◀️/firmware.bin"
  }
  ```

**Alur di log:**

* `fw_state: "DOWNLOADING"` → download bin via HTTP
* `fw_state: "VERIFYING"` → selesai tulis, verifikasi
* `fw_state: "SUCCESS"` → restart otomatis
* Jika error, `fw_state: "FAILED"`

> **Tip:** sebelum trigger OTA, pastikan device **sudah subscribe** ke `v1/devices/me/rpc/request/+` (kode melakukannya setelah MQTT connected).

---

## 8) Troubleshooting

* **WiFi tidak connect**

  * Cek SSID/PW → pastikan 2.4 GHz aktif
  * Jarak ke AP & daya sinyal
* **MQTT gagal**

  * Token salah → cek di Device → Credentials (ThingsBoard)
  * Port & URL benar (`mqtt://mqtt.thingsboard.cloud:1883`)
* **NTP stuck “Menunggu sinkronisasi NTP…”**

  * Koneksi internet wajib aktif
  * Coba tunggu ±10–20 detik atau restart board
* **DHT22 error baca**

  * Pastikan **DATA → GPIO4**
  * Pull-up 10kΩ ke 3V3 jika modul tidak memiliki resistor internal
  * Hindari kabel terlalu panjang
* **OTA gagal**

  * URL tidak bisa diakses dari jaringan device
  * MIME/ukuran file/korup → host ulang file
  * Pastikan binary hasil rilis terbaru & cocok untuk ESP32-S3 (ESP-IDF std)

---

## 9) Keamanan & Praktik Baik

* Jangan commit **SSID/PW WiFi** & **Device Token** ke repo publik
* Gunakan **.gitignore**:

  ```
  target/
  .vscode/
  **/secrets.*
  ```
* Versikan firmware di `CURRENT_FIRMWARE_VERSION` saat rilis

---

## 10) Lisensi

▶️ Tambahkan lisensi yang kamu pilih (MIT/Apache-2.0/BSD-3-Clause).
Contoh cepat (MIT):

```
MIT License
Copyright (c) 2025 ...
...
```

---

## 11) Ringkasan Perintah Cepat

```bash
# sekali saja (tooling)
cargo install espup espflash
espup install
source "$HOME/export-esp.sh"   # Windows: gunakan shell yang sudah ter-setup

# build
cargo build --release

# flash & monitor (ubah port)
espflash flash target/xtensa-esp32s3-espidf/release/esp32s3-dht22-tb-ota --monitor --port /dev/ttyACM0
```

📜 Lisensi

Lisensi bebas digunakan dengan mencantumkan kredit:
© 2025 Bambang Pamarta — MIT License

