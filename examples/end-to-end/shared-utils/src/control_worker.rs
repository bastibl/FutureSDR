use anyhow::bail;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};

// SQLx does not compile for wasm32 (the socket2 crate in particular).
/// Used to mark what kind of data a node does or should produce. Also used in path parameter for node and frontend.
#[derive(Debug, Deserialize, Serialize, Hash, Eq, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(not(target_arch = "wasm32"), derive(sqlx::Type))]
#[cfg_attr(
    not(target_arch = "wasm32"),
    sqlx(type_name = "data_type_marker", rename_all = "lowercase")
)]
pub enum DataTypeMarker {
    /// Fast Fourier Transformations
    Fft,
    /// ZigBee
    ZigBee,
}

impl Display for DataTypeMarker {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DataTypeMarker::Fft => {
                write!(f, "FFT")
            }
            DataTypeMarker::ZigBee => {
                write!(f, "ZigBee")
            }
        }
    }
}

/// Configuration for a specific data type.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum DataTypeConfig {
    /// Fast Fourier Transformations
    Fft {
        /// The amount of 2048 element chunks the fft worker should send to the backend at once.
        fft_chunks_per_ws_transfer: usize,
    },
    /// ZigBee
    ZigBee,
}
impl Display for DataTypeConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DataTypeConfig::Fft {
                fft_chunks_per_ws_transfer,
            } => {
                write!(f, "FFT({fft_chunks_per_ws_transfer})")
            }
            DataTypeConfig::ZigBee => {
                write!(f, "ZigBee")
            }
        }
    }
}

/// Configuration of a node.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NodeConfig {
    /// How the SDR should be configured.
    pub sdr_config: SdrConfig,
    /// The data types that should be processed with corresponding config.
    pub data_types: HashMap<DataTypeMarker, DataTypeConfig>,
}

/// SDR parameters used to set the configuration of a node.
/// default node config taken from https://github.com/bastibl/webusb-libusb/blob/works/example/hackrf_open.cc#L161
/// steps and ranges taken from https://hackrf.readthedocs.io/en/latest/faq.html#what-gain-controls-are-provided-by-hackrf
/// Sample rate and frequency taken from https://hackrf.readthedocs.io/en/latest/hackrf_one.html
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SdrConfig {
    /// Frequency
    /// default: 2480000000 (2,48GHz)
    /// 1MHz to 6 GHz (1000000 - 6000000000)
    pub freq: u64,
    /// Amplification
    /// default 1
    /// on or off (0 or 1)
    pub amp: u8,
    /// LNA
    /// default: 32
    /// 0-40 in steps of 8
    pub lna: u8,
    /// VGA
    /// default: 14
    /// 0-62 in steps of 2
    pub vga: u8,
    /// Sample rate
    /// default: 4000000 (4 Msps)
    /// 1 Msps to 20 Msps (million samples per second) (1000000 - 20000000)
    pub sample_rate: u64,
}

impl SdrConfig {
    /// Check whether the config values are within the specified limits.
    /// # Errors
    /// If one of the parameters is not within the defined range.
    pub fn check_plausibility(&self) -> anyhow::Result<()> {
        if self.freq < 1000000 {
            bail!("Frequency too low");
        }
        if self.freq > 6000000000 {
            bail!("Frequency too high")
        }
        if self.amp != 0 && self.amp != 1 {
            bail!("Amp invalid")
        }
        if self.lna % 8 != 0 {
            bail!("LNA invalid")
        }
        if self.lna > 40 {
            bail!("LNA too high")
        }
        if self.vga % 2 != 0 {
            bail!("VGA invalid")
        }
        if self.vga > 62 {
            bail!("VGA too high")
        }
        if self.sample_rate < 1000000 {
            bail!("Sample rate too low")
        }
        if self.sample_rate > 20000000 {
            bail!("Sample rate too high")
        }

        Ok(())
    }
}

/// Used to communicate from the UI/main thread to the control worker.
#[derive(Debug, Deserialize, Serialize)]
pub enum ToControlWorkerMsg {
    /// Initialize the control worker.
    Initialize,
    /// Acknowledge the config to the backend.
    AckConfig {
        /// The config to be acknowledged.
        config: NodeConfig,
    },
    /// Request the initial config from the backend.
    GetInitialConfig,
}

/// Used to communicate from the control worker to the UI/main thread.
#[derive(Debug, Deserialize, Serialize)]
pub enum FromControlWorkerMsg {
    /// A new config was received.
    ReceivedConfig {
        /// The newly received config
        config: NodeConfig,
    },
    /// Print to the screen of the UI.
    PrintToScreen {
        /// The message to print
        msg: String,
    },
    /// The worker got disconnected from the backend.
    Disconnected,
    /// Terminate all workers.
    Terminate {
        /// Message to print to the UI.
        msg: String,
    },
}

/// Used to communicate from the node to the backend.
#[derive(Debug, Deserialize, Serialize)]
pub enum NodeToBackend {
    /// Request configuration from backend.
    RequestConfig,
    /// Acknowledge configuration to backend.
    AckConfig {
        /// Config to acknowledge.
        config: NodeConfig,
    },
}

/// Used to communicate from the node to the backend.
#[derive(Debug, Deserialize, Serialize)]
pub enum BackendToNode {
    /// Send config to node.
    SendConfig {
        /// The config to send.
        config: NodeConfig,
    },
    /// Send error and whether to terminate the node or not.
    Error {
        /// Message to be printed to the UI.
        msg: String,
        /// Whether the node should be terminated.
        terminate: bool,
    },
}
