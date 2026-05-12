# Zigbee Example

## Introduction

Zigbee is a wireless standard designed for low-power, low-data-rate applications, operating under the IEEE 802.15.4 specification. It is commonly used in Internet of Things (IoT) networks for device-to-device communication in smart homes and industrial environments. This implementation utilizes O-QPSK modulation and DSSS (Direct Sequence Spread Spectrum) within the 2.4 GHz ISM band.

This project implements Zigbee signal processing using the FutureSDR framework. It provides native tools to transmit, receive, and analyze Zigbee frames using Software Defined Radio (SDR) hardware. The signal-processing blocks in this crate are also reused by the separate `../zigbee-wasm` browser receiver.

## 1. Transmitter (tx)
The transmitter mode generates Zigbee-compliant frames and broadcasts them via the connected SDR hardware. It handles the MAC-layer framing and physical layer modulation.

To transmit via SDR:

```sh
cargo run --release --bin tx -- --gain {value} --channel {11..26}
```

The SDR will transmit periodic messages. The terminal will display the current configuration and transmission status (in debug mode).

## 2. Receiver (rx)
The receiver mode captures and demodulates Zigbee signals. It can process live signals from SDR hardware or read from pre-recorded IQ files.

* To receive from SDR:

```sh
cargo run --release --bin rx -- --gain {value} --channel {11..26}
```

* To decode from a file:

```sh
cargo run --release --bin rx -- --file {file_name}.{file_type}
```

Decoded frame information, including addresses and payloads, is printed to the terminal. Decoded data is also sent to a UDP sink (default port 55555) for analysis in Wireshark.

Note that the receiver can be tested by using a COTS Zigbee device or by running the transmitter on a separate SDR setup. 

## 3. Transceiver (trx)
The transceiver mode runs the transmitter and receiver chains simultaneously within a single flowgraph. This is used for loopback testing on devices with multiple ports.

```sh
cargo run --release --bin trx -- --rx-gain {value} --tx-gain {value} --tx-channel {11..26} --rx-channel {11..26}
```

## 4. Native Web GUI

The native receiver can serve a browser GUI through the FutureSDR control port. Build the frontend once:

```sh
trunk build --release
```

Then run `rx` from this directory. `config.toml` points the control port at `dist/`, so open <http://127.0.0.1:1337/> to control the native flowgraph and monitor decoded frames.

## 5. WebAssembly Receiver

The browser/WebUSB receiver lives in the separate `../zigbee-wasm` workspace. It depends on this crate with default features disabled and reuses the blocks here.
