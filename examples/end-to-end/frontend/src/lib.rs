use chrono::{TimeZone, Utc};
use gloo_net::http::Request;

use shared_utils::{
    DataTypeConfig, DataTypeMarker, NodeConfig, NodeConfigRequest, NodeMetaDataResponse, SdrConfig,
};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};

fn get_config_from_form() -> NodeConfigRequest {
    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();

    let node_id: Uuid = Uuid::from_str(
        document
            .get_element_by_id("node_input")
            .unwrap()
            .dyn_into::<HtmlInputElement>()
            .unwrap()
            .value()
            .trim(),
    )
    .unwrap();
    let fft: bool = document
        .get_element_by_id("fft_checkbox")
        .unwrap()
        .dyn_into::<HtmlInputElement>()
        .unwrap()
        .checked();
    let zigbee: bool = document
        .get_element_by_id("zigbee_checkbox")
        .unwrap()
        .dyn_into::<HtmlInputElement>()
        .unwrap()
        .checked();
    let freq: u64 = document
        .get_element_by_id("frequency_input")
        .unwrap()
        .dyn_into::<HtmlInputElement>()
        .unwrap()
        .value()
        .trim()
        .parse()
        .unwrap();
    let amp: bool = document
        .get_element_by_id("amp_checkbox")
        .unwrap()
        .dyn_into::<HtmlInputElement>()
        .unwrap()
        .checked();
    let amp = if amp { 1_u8 } else { 0_u8 };
    let lna: u8 = document
        .get_element_by_id("lna_input")
        .unwrap()
        .dyn_into::<HtmlSelectElement>()
        .unwrap()
        .value()
        .trim()
        .parse()
        .unwrap();
    let vga: u8 = document
        .get_element_by_id("vga_input")
        .unwrap()
        .dyn_into::<HtmlSelectElement>()
        .unwrap()
        .value()
        .trim()
        .parse()
        .unwrap();
    let sample_rate: u64 = document
        .get_element_by_id("sample_rate_input")
        .unwrap()
        .dyn_into::<HtmlInputElement>()
        .unwrap()
        .value()
        .trim()
        .parse()
        .unwrap();
    let fft_chunks_per_ws_transfer: usize = document
        .get_element_by_id("fft_chunks_per_ws_transfer_input")
        .unwrap()
        .dyn_into::<HtmlInputElement>()
        .unwrap()
        .value()
        .trim()
        .parse()
        .unwrap();
    let mut data_types = HashMap::new();
    if fft {
        data_types.insert(
            DataTypeMarker::Fft,
            DataTypeConfig::Fft {
                fft_chunks_per_ws_transfer,
            },
        );
    }
    if zigbee {
        data_types.insert(DataTypeMarker::ZigBee, DataTypeConfig::ZigBee);
    }

    NodeConfigRequest {
        node_id,
        config: NodeConfig {
            sdr_config: SdrConfig {
                freq,
                amp,
                lna,
                vga,
                sample_rate,
            },
            data_types,
        },
    }
}

#[wasm_bindgen]
pub async fn send_config_rust() {
    let config = get_config_from_form();
    let resp = Request::post("/frontend_api/config")
        .json(&config)
        .unwrap()
        .send()
        .await
        .unwrap();
    if resp.ok() {
        let window = web_sys::window().unwrap();
        let document = window.document().unwrap();
        let send_response = document.get_element_by_id("send_response").unwrap();
        send_response.set_inner_html("success");

        spawn_local(async move {
            gloo_timers::future::sleep(Duration::from_secs(3)).await;
            let window = web_sys::window().unwrap();
            let document = window.document().unwrap();
            let send_response = document.get_element_by_id("send_response").unwrap();
            send_response.set_inner_html("");
        })
    }
}

#[wasm_bindgen]
pub async fn update_from_backend_rust() {
    let resp = Request::get("/frontend_api/nodes").send().await.unwrap();
    let nodes = resp
        .json::<Vec<shared_utils::NodeMetaDataResponse>>()
        .await
        .unwrap();

    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();

    let table = document.get_element_by_id("nodes_table").unwrap();
    let table_header = r#"<tr>
                <th>Node ID</th>
                <th>FFT Visualizer</th>
                <th>ZigBee Visualizer</th>
                <th>Current config</th>
                <th>Last seen</th>
            </tr>"#
        .to_owned();
    let table_rows = nodes.iter().fold(table_header, |acc, node| {
        format!("{acc} {}", create_table_row(node.clone()))
    });

    table.set_inner_html(&table_rows);
}

fn create_table_row(node_meta_data_response: NodeMetaDataResponse) -> String {
    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();

    let time_selector = document
        .get_element_by_id("historic_time")
        .unwrap()
        .dyn_into::<HtmlInputElement>()
        .unwrap()
        .value();
    let time = chrono::NaiveDateTime::parse_from_str(&time_selector, "%Y-%m-%dT%H:%M").unwrap();
    let current_js_date = js_sys::Date::new_0();
    let timezone_offset = current_js_date.get_timezone_offset();
    let timezone_fixed_offset =
        chrono::offset::FixedOffset::west((timezone_offset * 60_f64) as i32);
    let datetime_fixed_offset = timezone_fixed_offset.from_local_datetime(&time).unwrap();
    let naive_utc_time = datetime_fixed_offset.naive_utc();
    let utc_with_tz_time = Utc.from_utc_datetime(&naive_utc_time);
    let query_vec = vec![("timestamp".to_owned(), utc_with_tz_time.to_string())];
    let timestamp = serde_urlencoded::to_string(query_vec).unwrap();

    let last_seen = node_meta_data_response.last_seen.to_string();

    let SdrConfig {
        freq,
        amp,
        lna,
        vga,
        sample_rate,
    } = node_meta_data_response.config.sdr_config;
    let mut data_types = node_meta_data_response
        .config
        .data_types
        .values()
        .fold("".to_owned(), |acc, x| format!("{acc}{x}; "));
    // remove last space and semicolon
    data_types.pop();
    data_types.pop();
    let amp = if amp == 0 {
        "off".to_owned()
    } else {
        "on".to_owned()
    };
    let node_id = node_meta_data_response.node_id.to_string();
    format!(
        "\
    <tr>\
        <td>{node_id}</td>\
        <td>\
            <a href=\"fft/{node_id}\">stream</a>\
            <br>\
            <a href=\"fft/{node_id}/?{timestamp}\">historic</a>\
        </td>\
        <td>\
            <a href=\"zigbee/{node_id}\">stream</a>\
            <br>\
            <a href=\"zigbee/{node_id}/?{timestamp}\">historic</a>\
        </td>\
        <td>\
            Mode: {data_types}\
            <br> \
            Freq: {freq} Hz\
            <br> \
            Amp: {amp}\
            <br> \
            LNA: {lna}\
            <br> \
            VGA: {vga}\
            <br> \
            Sample rate: {sample_rate}\
        </td>\
        <td>\
            {last_seen}\
        </td> \
    </tr>\
    "
    )
}

#[wasm_bindgen]
pub fn set_console_error_panic_hook() {
    console_error_panic_hook::set_once();
}
