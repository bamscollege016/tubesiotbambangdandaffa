use std::{thread, time::Duration, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use anyhow::{Result, Error};
use chrono::{Duration as ChronoDuration, NaiveDateTime, Utc, TimeZone}; // WARNING FIX: Removed unused DateTime import
use dht_sensor::dht22::Reading;
use dht_sensor::DhtReading;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::{
        delay::Ets,
        // WARNING FIX: Removed unused Input, Output, Pin
        gpio::{PinDriver}, 
        prelude::*,
    },
    log::EspLogger,
    mqtt::client::*,
    nvs::EspDefaultNvsPartition,
    sntp,
    systime::EspSystemTime,
    wifi::*,
    ota::EspOta,
    http::client::EspHttpConnection,
};
use embedded_svc::{
    mqtt::client::QoS,
    // WARNING FIX: Removed unused Request
    http::client::Client, 
    io::Read, // This is embedded_svc::io::Read, required for ota_process
};
use heapless::String;
use serde_json::json;
// use rand::Rng; // ⚠️ CLEANUP: Dihapus karena tidak digunakan

// --- Konfigurasi Firmware & Device ---
const CURRENT_FIRMWARE_VERSION: &str = "PaceP-s3-v2.0";
// 📌 Menggunakan ThingsBoard Cloud
const TB_MQTT_URL: &str = "mqtt://mqtt.thingsboard.cloud:1883";
const THINGSBOARD_TOKEN: &str = "WravrVt8rB79D8hQeqr2"; // Token Device ThingsBoard

// 3. EXTERN C FUNCTION
// ✅ FIX E0425: Menambahkan kembali deklarasi fungsi C
extern "C" {
    fn esp_restart();
}

// =========================================================================================
// --- MQTT Client State (Global Access) ---
static mut MQTT_CLIENT: Option<EspMqttClient<'static>> = None;

fn get_mqtt_client() -> Option<&'static mut EspMqttClient<'static>> {
    unsafe {
        MQTT_CLIENT.as_mut().map(|c| {
            // SAFETY: Transmute untuk mengatasi batasan lifetime 'static dari EspMqttClient
            std::mem::transmute::<&mut EspMqttClient<'_>, &mut EspMqttClient<'static>>(c)
        })
    }
}

// Fungsi untuk mengirim telemetry fw_state
fn publish_fw_state(state: &str) {
    let payload = format!("{{\"fw_state\":\"{}\"}}", state);
    log::info!("➡️ Mengirim telemetry fw_state: {}", payload);

    if let Some(client) = get_mqtt_client() {
        // ✅ FIX 1: Mengubah QoS dari ExactlyOnce (QoS 2) menjadi AtLeastOnce (QoS 1)
        if let Err(e) = client.publish(
            "v1/devices/me/telemetry",
            QoS::AtLeastOnce, 
            false,
            payload.as_bytes(),
        ) {
            log::error!("⚠️ Gagal kirim fw_state {}: {:?}", state, e);
        }
    } else {
        log::error!("⚠️ MQTT client belum siap untuk kirim fw_state {}", state);
    }
}

// Mengirim versi firmware saat ini (CRUCIAL UNTUK OTA)
fn publish_fw_version() {
    let payload = format!("{{\"fw_version\":\"{}\"}}", CURRENT_FIRMWARE_VERSION);
    log::info!("➡️ Mengirim Current FW Version: {}", payload);

    if let Some(client) = get_mqtt_client() {
        if let Err(e) = client.publish(
            "v1/devices/me/telemetry",
            QoS::AtLeastOnce,
            false,
            payload.as_bytes(),
        ) {
            log::error!("⚠️ Gagal kirim fw_version: {:?}", e);
        }
    } else {
        log::error!("⚠️ MQTT client belum siap untuk kirim fw_version");
    }
}

// Fungsi untuk mengirim RPC response ke ThingsBoard
fn send_rpc_response(request_id: &str, status: &str) {
    let topic = format!("v1/devices/me/rpc/response/{}", request_id);
    log::info!("➡️ Mengirim RPC response ke: {}", topic);

    let payload = format!("{{\"status\":\"{}\"}}", status);

    if let Some(client) = get_mqtt_client() {
        if let Err(e) = client.publish(
            topic.as_str(),
            QoS::AtLeastOnce,
            false,
            payload.as_bytes(),
        ) {
            log::error!("⚠️ Gagal kirim RPC response: {:?}", e);
        }
    } else {
        log::error!("⚠️ MQTT client belum siap untuk kirim RPC response");
    }
}

// 5. OTA PROCESS FUNCTION
fn ota_process(url: &str) {
    log::info!("📥 Mulai OTA dari URL: {}", url);
    publish_fw_state("DOWNLOADING");

    // Jeda 500ms untuk memastikan pesan "DOWNLOADING" terkirim oleh event loop MQTT
    // sebelum proses HTTP download yang berat dimulai dan memblokir network.
    thread::sleep(Duration::from_millis(500));

    match EspOta::new() {
        Ok(mut ota) => {
            let http_config = esp_idf_svc::http::client::Configuration {
                ..Default::default()
            };

            let conn = match EspHttpConnection::new(&http_config) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("⚠️ Gagal buat koneksi HTTP: {:?}", e);
                    publish_fw_state("FAILED");
                    return;
                }
            };

            let mut client = Client::wrap(conn);
            let request = match client.get(url) {
                Ok(r) => r,
                Err(e) => {
                    log::error!("⚠️ Gagal buat HTTP GET: {:?}", e);
                    publish_fw_state("FAILED");
                    return;
                }
            };

            let mut response = match request.submit() {
                Ok(r) => r,
                Err(e) => {
                    log::error!("⚠️ Gagal submit request: {:?}", e);
                    publish_fw_state("FAILED");
                    return;
                }
            };

            if response.status() < 200 || response.status() >= 300 {
                log::error!("⚠️ HTTP request gagal. Status code: {}", response.status());
                publish_fw_state("FAILED");
                return;
            }

            let mut buf = [0u8; 1024];
            let mut update = match ota.initiate_update() {
                Ok(u) => u,
                Err(e) => {
                    log::error!("⚠️ Gagal init OTA: {:?}", e);
                    publish_fw_state("FAILED");
                    return;
                }
            };

            loop {
                match response.read(&mut buf) {
                    Ok(0) => break,
                    Ok(size) => {
                        if let Err(e) = update.write(&buf[..size]) {
                            log::error!("⚠️ Gagal tulis OTA: {:?}", e);
                            publish_fw_state("FAILED");
                            return;
                        }
                    }
                    Err(e) => {
                        log::error!("⚠️ HTTP read error: {:?}", e);
                        publish_fw_state("FAILED");
                        return;
                    }
                }
            }

            publish_fw_state("VERIFYING");

            if let Err(e) = update.complete() {
                log::error!("⚠️ OTA complete error: {:?}", e);
                publish_fw_state("FAILED");
                return;
            }

            log::info!("✅ OTA selesai, restart...");
            publish_fw_state("SUCCESS");

            // Jeda 1 detik agar pesan SUCCESS terkirim
            thread::sleep(Duration::from_secs(1));

            unsafe { esp_restart(); }
        }
        Err(e) => {
            log::error!("⚠️ Gagal init OTA: {:?}", e);
            publish_fw_state("FAILED");
        }
    }
}

// 6. MAIN FUNCTION
fn main() -> Result<(), Error> {
    // --- Inisialisasi dasar ---
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();
    log::info!("🚀 Program dimulai, Versi FW: {} - 🔥 FIRMWARE AKTIF!", CURRENT_FIRMWARE_VERSION);

    // --- Inisialisasi perangkat ---
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take().unwrap();

    // --- Konfigurasi WiFi ---
    let mut wifi = EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs.clone()))?;

    let mut ssid: String<32> = String::new();
    ssid.push_str("aasstagaa").unwrap();

    let mut pass: String<64> = String::new();
    pass.push_str("hahahaha").unwrap();

    let wifi_config = Configuration::Client(ClientConfiguration {
        ssid,
        password: pass,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    });

    wifi.set_configuration(&wifi_config)?;
    wifi.start()?;
    wifi.connect()?;

    // --- Tunggu sampai WiFi benar-benar aktif ---
    while !wifi.is_connected().unwrap() {
        log::info!("⏳ Menunggu koneksi WiFi...");
        thread::sleep(Duration::from_secs(1));
    }
    log::info!("✅ WiFi terhubung!");

    // Leak services to prevent drop/deinitialization
    let _wifi = Box::leak(Box::new(wifi));
    let _sysloop = Box::leak(Box::new(sysloop));
    let _nvs = Box::leak(Box::new(nvs));

    // --- Sinkronisasi waktu via NTP ---
    log::info!("🌐 Sinkronisasi waktu NTP...");
    let sntp = sntp::EspSntp::new_default()?;

    loop {
        let status = sntp.get_sync_status();
        if status == sntp::SyncStatus::Completed {
            log::info!("✅ Waktu berhasil disinkronkan dari NTP");
            break;
        } else {
            log::info!("⏳ Menunggu sinkronisasi NTP...");
            thread::sleep(Duration::from_secs(1));
        }
    }

    // Delay tambahan agar waktu stabil
    thread::sleep(Duration::from_secs(5));

    // --- Konfigurasi MQTT (ThingsBoard Cloud) ---
    let mqtt_config = MqttClientConfiguration {
        client_id: Some("esp32-rust-ota"),
        username: Some(THINGSBOARD_TOKEN), // Menggunakan const THINGSBOARD_TOKEN
        password: None,
        keep_alive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    };

    let mqtt_connected = Arc::new(AtomicBool::new(false));

    // --- MQTT Callback Handler (untuk menangani RPC OTA) ---
    let mqtt_callback = {
        let mqtt_connected = mqtt_connected.clone();

        move |event: EspMqttEvent<'_>| {
            use esp_idf_svc::mqtt::client::EventPayload;

            match event.payload() {
                EventPayload::Connected(_) => {
                    log::info!("📡 MQTT connected");
                    mqtt_connected.store(true, Ordering::SeqCst);
                }
                EventPayload::Received { topic, data, .. } => {
                    let payload_str = std::str::from_utf8(data).unwrap_or("");
                    log::info!("📩 Payload diterima. Topic: {:?}, Data: {}", topic, payload_str);

                    if let Some(topic_str) = topic {
                        // ⚡ Logic RPC Request untuk OTA
                        if topic_str.starts_with("v1/devices/me/rpc/request/") {
                            let parts: Vec<&str> = topic_str.split('/').collect();
                            if let Some(request_id) = parts.last() {
                                log::info!("✅ Menerima RPC request_id: {}", request_id);

                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload_str) {
                                    let mut ota_url_owned = None;

                                    // Cek apakah ada parameter ota_url di dalam RPC
                                    if let Some(url_str) = json.get("params").and_then(|p| p.get("ota_url")).and_then(|u| u.as_str()) {
                                        // ✅ FIX E0597: Kloning string agar thread baru memiliki data (owned)
                                        ota_url_owned = Some(url_str.to_owned());
                                    }

                                    if let Some(url) = ota_url_owned {
                                        log::info!("⚡ Dapat OTA URL dari RPC: {}", url);
                                        send_rpc_response(request_id, "success");
                                        
                                        // ✅ FIX 2: Mencegah Stack Overflow dengan mengalokasikan stack 10KB
                                        std::thread::Builder::new()
                                            .stack_size(10 * 1024) 
                                            .spawn(move || {
                                                // Pass reference (&str) to the owned String
                                                ota_process(&url);
                                            })
                                            .expect("Gagal membuat thread OTA");
                                            
                                        return;
                                    } else {
                                        log::warn!("⚠️ Payload RPC diterima, tetapi \"ota_url\" tidak ditemukan.");
                                    }
                                } else {
                                    log::error!("⚠️ Gagal mem-parse JSON payload RPC.");
                                }

                                send_rpc_response(request_id, "failure");
                            }
                        }
                    }
                }
                EventPayload::Disconnected => {
                    log::warn!("⚠️ MQTT Disconnected!");
                    mqtt_connected.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    };

    // --- Inisialisasi MQTT Client (Non-static Callback) ---
    let client = loop {
        let res = unsafe {
            EspMqttClient::new_nonstatic_cb(
                TB_MQTT_URL,
                &mqtt_config,
                mqtt_callback.clone(),
            )
        };

        match res {
            Ok(c) => {
                unsafe { MQTT_CLIENT = Some(c) };

                if let Some(c_ref) = get_mqtt_client() {
                    while !mqtt_connected.load(Ordering::SeqCst) {
                        log::info!("⏳ Menunggu MQTT connect...");
                        thread::sleep(Duration::from_millis(500));
                    }
                    log::info!("📡 MQTT Connected!");

                    // Subscribe ke topik RPC untuk menerima perintah OTA
                    c_ref.subscribe("v1/devices/me/rpc/request/+", QoS::AtLeastOnce).unwrap();

                    // LAPORAN INI KRUSIAL UNTUK OTA (melaporkan versi dan status awal)
                    publish_fw_version();
                    publish_fw_state("IDLE");

                    break c_ref;
                } else {
                    log::error!("⚠️ Gagal mendapatkan referensi client setelah koneksi.");
                    thread::sleep(Duration::from_secs(5));
                    continue;
                }
            }
            Err(e) => {
                log::error!("⚠️ MQTT connect gagal: {:?}", e);
                thread::sleep(Duration::from_secs(5));
            }
        }
    };

    // --- Inisialisasi sensor DHT22 ---
    let mut pin = PinDriver::input_output_od(peripherals.pins.gpio4)?;
    let mut delay = Ets;

    // --- Loop utama kirim data (Interval 60 detik) ---
    loop {
        // Ambil waktu sekarang
        let systime = EspSystemTime {}.now();
        let secs = systime.as_secs() as i64;
        let nanos = systime.subsec_nanos();
        
        let naive = NaiveDateTime::from_timestamp_opt(secs, nanos as u32).unwrap_or(NaiveDateTime::from_timestamp_opt(0, 0).unwrap());
        // ✅ FIX E0308: Meminjam (&) nilai naive karena from_utc_datetime membutuhkan referensi.
        let utc_time = Utc.from_utc_datetime(&naive);
        let wib_time = utc_time + ChronoDuration::hours(7);
        // ✅ Fix Chrono API warning
        let ts_millis = naive.and_utc().timestamp_millis();
        let send_time_str = wib_time.format("%Y-%m-%d %H:%M:%S").to_string();

        // Baca sensor DHT22
        match Reading::read(&mut delay, &mut pin) {
            Ok(Reading {
                temperature,
                relative_humidity,
            }) => {
                // Siapkan payload JSON
                let payload = json!({
                    "send_time": send_time_str, // waktu kirim dari ESP (WIB)
                    "ts": ts_millis,            // waktu epoch (untuk analisis ThingsBoard)
                    "temperature": temperature,
                    "humidity": relative_humidity
                });

                let payload_str = payload.to_string();

                // Kirim data Telemetry
                match client.publish(
                    "v1/devices/me/telemetry",
                    QoS::AtLeastOnce, // Menggunakan QoS 1 untuk telemetry
                    false,
                    payload_str.as_bytes(),
                ) {
                    Ok(_) => log::info!("📤 Data terkirim (T: {}°C, H: {}%): {}", temperature, relative_humidity, payload_str),
                    Err(e) => log::error!("❌ Gagal publish ke MQTT: {:?}", e),
                }
            }
            Err(e) => log::error!("⚠️ Gagal baca DHT22: {:?}", e),
        }

        // Delay diubah menjadi 60 detik
        thread::sleep(Duration::from_secs(60));
    }
}