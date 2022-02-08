mod fft_shift;
use fft_shift::FftShift;
mod keep_1_in_n;
use keep_1_in_n::Keep1InN;
mod sink;
use sink::Sink;

use avro_rs::types::Value;
use avro_rs::Schema;
use futuresdr::anyhow::Context;
use futuresdr::anyhow::Result;
use futuresdr::async_io::Timer;
use futuresdr::blocks::Apply;
use futuresdr::blocks::Fft;
use futuresdr::blocks::SoapySourceBuilder;
use futuresdr::futures_lite::StreamExt;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::config;
use futuresdr::runtime::Block;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;
use mqtt::AsyncClient;
use paho_mqtt as mqtt;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn lin2db_block() -> Block {
    Apply::new(|x: &f32| 10.0 * x.log10())
}

pub fn power_block() -> Block {
    Apply::new(|x: &Complex32| x.norm())
}

#[derive(Deserialize, Debug)]
struct ElectrosenseConfig {
    sensor_id: i64,
}

async fn send_status(conn: &mut AsyncClient, schema: &Schema, sensor_id: i64) {
    println!("sending status");

    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let uptime = 123;

    let mut h = HashMap::new();
    h.insert(
        "Temperature".to_string(),
        Value::String("13.37".to_string()),
    );
    let status = Value::Map(h);

    let record = Value::Record(vec![
        (
            "Type".to_owned(),
            Value::Enum(4, "SensorStatus".to_string()),
        ),
        (
            "Message".to_owned(),
            Value::Union(Box::new(Value::Record(vec![
                ("SerialNumber".to_owned(), Value::Long(sensor_id)),
                ("Timestamp".to_owned(), Value::Long(time as i64)),
                ("Uptime".to_owned(), Value::Int(uptime as i32)),
                ("Status".to_owned(), Value::Enum(1, "SENSING".to_owned())),
                ("StatusInfo".to_owned(), Value::Union(Box::new(status))),
            ]))),
        ),
    ]);
    conn.publish(mqtt::Message::new(
        format!("sensor/status/{}", sensor_id),
        avro_rs::to_avro_datum(schema, record).unwrap(),
        2,
    ))
    .await
    .unwrap();
}

async fn send_info(conn: &mut AsyncClient, schema: &Schema, sensor_id: i64) {
    println!("sending info");

    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let record = Value::Record(vec![
        ("Type".to_owned(), Value::Enum(3, "SensorInfo".to_string())),
        (
            "Message".to_owned(),
            Value::Union(Box::new(Value::Record(vec![
                ("SerialNumber".to_owned(), Value::Long(sensor_id)),
                ("Timestamp".to_owned(), Value::Long(time as i64)),
                (
                    "SensorType".to_owned(),
                    Value::String("gnuradio".to_owned()),
                ),
                (
                    "SoftwareInfo".to_owned(),
                    Value::Union(Box::new(Value::Null)),
                ),
            ]))),
        ),
    ]);
    conn.publish(mqtt::Message::new(
        format!("sensor/status/{}", sensor_id),
        avro_rs::to_avro_datum(schema, record).unwrap(),
        2,
    ))
    .await
    .unwrap();
}

enum FrameType {
    GroupConfiguration(Value),
    Log(Value),
    MeasurementCommand(Value),
    SensorInfo(Value),
    SensorStatus(Value),
    StatusInfo(Value),
    StatusRequest(Value),
}

async fn handle_message(
    conn: &mut AsyncClient,
    schema: &Schema,
    msg: mqtt::Message,
    sensor_id: i64,
) {
    let data = avro_rs::from_avro_datum(&schema, &mut msg.payload(), None).unwrap();

    println!("{:?}", data);

    let frame_type = match data {
        Value::Record(v) => match v[0] {
            (_, Value::Enum(0, _)) => FrameType::GroupConfiguration(v[1].1.clone()),
            (_, Value::Enum(1, _)) => FrameType::Log(v[1].1.clone()),
            (_, Value::Enum(2, _)) => FrameType::MeasurementCommand(v[1].1.clone()),
            (_, Value::Enum(3, _)) => FrameType::SensorInfo(v[1].1.clone()),
            (_, Value::Enum(4, _)) => FrameType::SensorStatus(v[1].1.clone()),
            (_, Value::Enum(5, _)) => FrameType::StatusInfo(v[1].1.clone()),
            (_, Value::Enum(6, _)) => FrameType::StatusRequest(v[1].1.clone()),
            _ => panic!("unknown frame type"),
        },
        _ => panic!("top level is not record"),
    };

    let frame_type = match frame_type {
        FrameType::GroupConfiguration(Value::Union(v)) => FrameType::GroupConfiguration(*v),
        FrameType::Log(Value::Union(v)) => FrameType::Log(*v),
        FrameType::MeasurementCommand(Value::Union(v)) => FrameType::MeasurementCommand(*v),
        FrameType::SensorInfo(Value::Union(v)) => FrameType::SensorInfo(*v),
        FrameType::SensorStatus(Value::Union(v)) => FrameType::SensorStatus(*v),
        FrameType::StatusInfo(Value::Union(v)) => FrameType::StatusInfo(*v),
        FrameType::StatusRequest(Value::Union(v)) => FrameType::StatusRequest(*v),
        _ => panic!("wrong frame type union destructure"),
    };

    match frame_type {
        FrameType::GroupConfiguration(_) => println!("GroupConfig"),
        FrameType::Log(_) => panic!("log message should not be received"),
        FrameType::MeasurementCommand(_) => println!("measurement"),
        FrameType::SensorInfo(_) => panic!("sensor info should not be receivd"),
        FrameType::SensorStatus(_) => panic!("sensor status should not be received"),
        FrameType::StatusInfo(_) => {
            panic!("status info is a bogus field imho and should not be here")
        }
        FrameType::StatusRequest(v) => match v {
            Value::Record(v) => match v[0] {
                (_, Value::Enum(0, _)) => {
                    send_info(conn, schema, sensor_id).await;
                }
                (_, Value::Enum(1, _)) => {
                    send_status(conn, schema, sensor_id).await;
                }
                (_, Value::Enum(2, _)) => {
                    println!("req measurement");
                }
                _ => panic!("bad status request"),
            },
            _ => panic!("bad status request"),
        },
    }
}

async fn sensor_status(sensor_id: i64) -> Result<()> {
    let ca_crt = Path::new("assets/ca.crt");
    let client_crt = Path::new("assets/sensor.crt");
    let client_key = Path::new("assets/sensor.key");

    let schema =
        fs::read_to_string("assets/FullSchema.avsc").context("could not read schema file")?;
    let schema = Schema::parse_str(&schema).context("failed to parse schema file")?;

    let topics: &[String] = &[
        "control/sensor/all".into(),
        format!("control/sensor/id/{}", sensor_id),
    ];
    let qos: &[i32] = &[2, 2];
    let host = "ssl://controller.electrosense.org:8883";

    let create_opts = mqtt::CreateOptionsBuilder::new()
        .server_uri(host)
        .client_id(format!("{}", sensor_id))
        .finalize();

    let mut conn = mqtt::AsyncClient::new(create_opts).unwrap();
    let mut strm = conn.get_stream(25);

    let record = Value::Record(vec![
        (
            "Type".to_owned(),
            Value::Enum(4, "SensorStatus".to_string()),
        ),
        (
            "Message".to_owned(),
            Value::Union(Box::new(Value::Record(vec![
                ("SerialNumber".to_owned(), Value::Long(123)),
                ("Timestamp".to_owned(), Value::Long(0)),
                ("Uptime".to_owned(), Value::Int(0)),
                ("Status".to_owned(), Value::Enum(4, "OFF".to_owned())),
                ("StatusInfo".to_owned(), Value::Union(Box::new(Value::Null))),
            ]))),
        ),
    ]);
    let lwt = mqtt::Message::new(
        format!("sensor/status/{}", sensor_id),
        avro_rs::to_avro_datum(&schema, record).unwrap(),
        mqtt::QOS_1,
    );

    let ssl_opts = mqtt::SslOptionsBuilder::new()
        .trust_store(ca_crt)?
        .key_store(client_crt)?
        .private_key(client_key)?
        .finalize();

    let conn_opts = mqtt::ConnectOptionsBuilder::new()
        .ssl_options(ssl_opts)
        .keep_alive_interval(Duration::from_secs(20))
        .mqtt_version(mqtt::MQTT_VERSION_3_1_1)
        .user_name("sensor")
        .password("sensor")
        .will_message(lwt)
        .finalize();

    conn.connect(conn_opts).await?;
    conn.subscribe_many(topics, qos).await?;

    send_status(&mut conn, &schema, sensor_id).await;
    send_info(&mut conn, &schema, sensor_id).await;

    while let Some(msg_opt) = strm.next().await {
        if let Some(msg) = msg_opt {
            handle_message(&mut conn, &schema, msg, sensor_id).await;
        } else {
            // A "None" means we were disconnected. Try to reconnect...
            println!("Lost connection. Attempting reconnect.");
            while let Err(err) = conn.reconnect().await {
                println!("Error reconnecting: {}", err);
                Timer::after(Duration::from_millis(1000)).await;
            }
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let sensor_id = config::get_value("electrosense")
        .and_then(|v| v.try_into::<ElectrosenseConfig>().ok())
        .map(|v| v.sensor_id)
        .context("Electrosense-specific config not found")?;

    let mut fg = Flowgraph::new();

    let src = SoapySourceBuilder::new()
        .freq(100e6)
        .sample_rate(3.2e6)
        .gain(34.0)
        .build();
    let src = fg.add_block(src);
    let fft = fg.add_block(Fft::new());
    let power = fg.add_block(power_block());
    let log = fg.add_block(lin2db_block());
    let shift = fg.add_block(FftShift::<f32>::new());
    let keep = fg.add_block(Keep1InN::new(0.01, 600));
    let esnk = fg.add_block(Sink::new(sensor_id));

    fg.connect_stream(src, "out", fft, "in")?;
    fg.connect_stream(fft, "out", power, "in")?;
    fg.connect_stream(power, "out", log, "in")?;
    fg.connect_stream(log, "out", shift, "in")?;
    fg.connect_stream(shift, "out", keep, "in")?;
    fg.connect_stream(keep, "out", esnk, "in")?;

    let rt = Runtime::new();
    rt.spawn_background(sensor_status(sensor_id));
    rt.run(fg)?;
    Ok(())
}
