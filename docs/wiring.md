# KUMAUSHI CLIMATE — 配線ガイド

## センサーノード（ESP32-S3）ピン配置

```
ESP32-S3-MINI-1
    ┌─────────────────────────────┐
    │  GPIO8  ──── SHT31 SDA      │
    │  GPIO9  ──── SHT31 SCL      │
    │  3.3V   ──── SHT31 VDD      │
    │  GND    ──── SHT31 GND      │
    │                              │
    │  GPIO17 ──── MH-Z19B TX (RXD)│
    │  GPIO18 ──── MH-Z19B RX (TXD)│
    │  5V     ──── MH-Z19B Vin    │
    │  GND    ──── MH-Z19B GND    │
    └─────────────────────────────┘

注: MH-Z19BはUARTの向きに注意。センサーのTXDをESP32のRXへ。
```

## 中央コントローラー（Raspberry Pi 5）

### MQTT ブローカー（Mosquitto）
```bash
sudo apt install mosquitto mosquitto-clients
sudo systemctl enable mosquitto
```

### GPIO配線

| デバイスID | GPIO# | 用途 |
|-----------|-------|------|
| fan-z1 | 12 | PWM換気ファンZ1 (PCA9685 ch0) |
| fan-z2 | 13 | PWM換気ファンZ2 (PCA9685 ch1) |
| fan-z3 | 14 | PWM換気ファンZ3 (PCA9685 ch2) |
| fan-z5 | 15 | PWM換気ファンZ5 (PCA9685 ch3) |
| heat-z1 | 16 | 床暖房リレーZ1 |
| heat-z2 | 17 | 床暖房リレーZ2 |
| heat-z3 | 18 | 床暖房リレーZ3 |
| heat-z5 | 19 | 床暖房リレーZ5 |
| boiler-z4 | 20 | サウナボイラーリレー |
| pump-z4 | 21 | 循環ポンプリレー |
| dehu-z1 | 22 | 除湿機リレー |

### PCA9685 (I2C PWM expander)
```
Pi GPIO2 (SDA) ──── PCA9685 SDA
Pi GPIO3 (SCL) ──── PCA9685 SCL
Pi 3.3V        ──── PCA9685 VCC
Pi GND         ──── PCA9685 GND
12V external   ──── PCA9685 V+  (ファン電源)
```

## ネットワーク

```
屈斜路湖 山岳無線 ──── WiFiルーター(192.168.1.1)
                           │
               ┌───────────┴───────────┐
     Raspberry Pi (192.168.1.10)    ESP32-S3 nodes
       - Mosquitto :1883              - MQTT clients
       - kumaushi-controller :3000    - 30秒周期publish
       - SQLite /var/lib/kumaushi/
```

## 安全設計

- **水温ハードウェア過熱保護**: DS18B20がKtype42°Cを超えるとソフトでボイラーOFF（ソフトがハングした場合のバックアップとしてサーモスタット機械式安全装置も設置）
- **ファン故障検知**: RPMフィードバック（オプション）でPID出力に反してRPMが上がらない場合はアラート
- **CO2 1500ppm超**: 最大換気＋Telegram/KAGI通知
- **電源冗長**: UPS（APC Back-UPS 600VA）→ Raspberry Pi + センサーノード用PoEスイッチ
