# KUMAUSHI CLIMATE

屈斜路湖畔 KUMAUSHI BASE（12コンテナ）のための独自換気・空調制御システム。  
センサーノードからクラウドAPIまで、完全Rust実装。

```
[ESP32-S3 センサーノード] ─ MQTT ─> [Raspberry Pi コントローラー] ─ REST ─> [KAGI App]
   CO2 / 温湿度 / 水温                  PID制御 / GPIO / SQLite             スマートホーム連携
```

## アーキテクチャ

| クレート | ターゲット | 役割 |
|---------|----------|------|
| `kumaushi-common` | host | 共通型定義 (SensorReading, Zone, PidState) |
| `kumaushi-controller` | aarch64 Linux (Raspberry Pi 5) | MQTT subscriber + PID制御 + axum REST API |
| `kumaushi-sensor-node` | xtensa-esp32s3-espidf | CO2/SHT31読み取り + MQTT publish |

## センサー構成

- **MH-Z19B** — CO2濃度 (ppm)、UART接続
- **SHT31** — 温度 (°C) + 湿度 (%RH)、I2C接続
- **DS18B20** — 水温 (°C)、1-Wire（Z4バス・サウナゾーン）

## ゾーン構成 (12コンテナ)

| Zone | コンテナ | 用途 | 目標CO2 | 目標温度 |
|------|---------|------|---------|---------|
| Z1 | C1-C2 | メインリビング | 800 ppm | 22°C |
| Z2 | C3-C4 | 寝室A | 700 ppm | 20°C |
| Z3 | C5-C6 | 寝室B | 700 ppm | 20°C |
| Z4 | C7-C8 | バス・サウナ | 1200 ppm | 38°C (水温) |
| Z5 | C9-C10 | 多目的・ワーク | 800 ppm | 21°C |
| Z6 | C11-C12 | 機械室・廊下 | — | — |

## REST API

```
GET  /api/v1/sensors                   # 全センサー最新値
GET  /api/v1/sensors/:node_id/history  # 時系列 (hours=24)
GET  /api/v1/zones                     # 全ゾーン状態
GET  /api/v1/zones/:id                 # ゾーン詳細
POST /api/v1/zones/:id/mode            # {"mode": "auto"|"manual"|"off"}
POST /api/v1/zones/:id/setpoint        # {"temperature":22,"co2_max":800,"humidity":50}
GET  /api/v1/controls                  # 全デバイス現在値
POST /api/v1/controls/:device_id       # {"value": 0.8}
GET  /api/v1/dashboard                 # ダッシュボード用スナップショット
GET  /ws                               # WebSocket ライブフィード
GET  /dashboard                        # リアルタイムWeb UI
```

## セットアップ

### コントローラー (Raspberry Pi 5)

```bash
# 依存インストール
sudo apt install mosquitto mosquitto-clients sqlite3

# ビルド
cargo build --release -p kumaushi-controller

# 環境変数
export MQTT_HOST=localhost
export MQTT_PORT=1883
export BIND_ADDR=0.0.0.0:3000

# 実行
./target/release/kumaushi-controller
```

### センサーノード (ESP32-S3)

```bash
# ESP-IDF + Rust xtensa toolchain
cargo install espup
espup install

# ビルドと書き込み (NODE_IDとWiFi設定を環境変数で渡す)
cd crates/sensor-node
WIFI_SSID="KumaushiBase" \
WIFI_PASS="your-password" \
MQTT_URI="mqtt://192.168.1.10:1883" \
NODE_ID="node-z1-a" \
cargo build --release
espflash flash --monitor target/xtensa-esp32s3-espidf/release/sensor-node
```

## PID制御パラメータ

### 換気ファン (CO2制御)
- Kp=0.05, Ki=0.001, Kd=0.01
- 出力: ファンPWM 0-100%

### 床暖房 (温度制御)  
- Kp=2.0, Ki=0.1, Kd=0.5
- デッドバンド: ±0.5°C でヒステリシスOn/Off

## アラート条件

| 状態 | 閾値 | アクション |
|------|------|----------|
| CO2高 | >1500 ppm | 最大換気 + アラート |
| 水温超過 | >42°C | ボイラー強制OFF |
| 湿度高 | >設定+5% | 除湿機ON |

## KAGI連携

KAGI スマートホームアプリから `/api/v1/zones/:id/setpoint` および `/api/v1/zones/:id/mode` を呼び出して遠隔操作可能。認証: `Authorization: Bearer $AUTH_TOKEN`

## ライセンス

MIT © SOLUNA / Yuki Hamada
