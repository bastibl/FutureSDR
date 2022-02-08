use async_native_tls::Identity;
use async_native_tls::TlsConnector;
use async_native_tls::TlsStream;
use avro_rs::types::Value;
use avro_rs::Schema;
use futuresdr::futures_lite::AsyncWriteExt;
use std::mem::size_of;
use std::time::{SystemTime, UNIX_EPOCH};

use futuresdr::anyhow::Context;
use futuresdr::anyhow::Result;
use futuresdr::async_net::TcpStream;
use futuresdr::async_trait::async_trait;
use futuresdr::runtime::AsyncKernel;
use futuresdr::runtime::Block;
use futuresdr::runtime::BlockMeta;
use futuresdr::runtime::BlockMetaBuilder;
use futuresdr::runtime::MessageIo;
use futuresdr::runtime::MessageIoBuilder;
use futuresdr::runtime::StreamIo;
use futuresdr::runtime::StreamIoBuilder;
use futuresdr::runtime::WorkIo;

pub struct Sink {
    schema: Option<Schema>,
    sensor_id: i64,
    stream: Option<TlsStream<TcpStream>>,
}

impl Sink {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(sensor_id: i64) -> Block {
        Block::new_async(
            BlockMetaBuilder::new("ElectrosenseSink").build(),
            StreamIoBuilder::new()
                .add_input("in", size_of::<f32>())
                .build(),
            MessageIoBuilder::new().build(),
            Self {
                schema: None,
                sensor_id,
                stream: None,
            },
        )
    }
}

#[async_trait]
impl AsyncKernel for Sink {
    async fn init(
        &mut self,
        _sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        println!("###### In init()");
        let schema = std::fs::read_to_string("assets/rtl-spec.avsc")
            .context("could not read schema file")?;
        let schema = Schema::parse_str(&schema).context("failed to parse schema file")?;
        self.schema = Some(schema);

        println!("###### open TCP stream");
        let stream = TcpStream::connect("collector.electrosense.org:5001").await?;

        println!("###### creating idendity");
        let id = Identity::from_pkcs12(&std::fs::read("assets/identity.pfx")?, "")?;
        println!("###### open tls");
        let stream = TlsConnector::new()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .identity(id)
            .connect("collector.electrosense.org", stream)
            .await?;
        self.stream = Some(stream);

        println!("###### Done init()");
        Ok(())
    }

    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let input = sio.input(0).slice::<f32>();

        let n = input.len() / 2048;
        let schema = self.schema.as_ref().unwrap();
        let stream = self.stream.as_mut().unwrap();

        println!("###### In work() n floats {}", input.len());

        for i in 0..n {
            let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            let data: Vec<Value> = input[i * 2048..(i + 1) * 2048]
                .iter()
                .map(|v| Value::Float(*v))
                .collect();

            let record = Value::Record(vec![
                ("SenId".to_owned(), Value::Long(self.sensor_id)),
                (
                    "SenConf".to_owned(),
                    Value::Record(vec![
                        ("HoppingStrategy".to_owned(), Value::Int(0)),
                        ("WindowingFunction".to_owned(), Value::Int(2)),
                        ("FFTSize".to_owned(), Value::Int(2048)),
                        ("AveragingFactor".to_owned(), Value::Int(4)),
                        ("FrequencyOverlap".to_owned(), Value::Float(0.1)),
                        ("FrequencyResolution".to_owned(), Value::Float(1562.5)),
                        ("Gain".to_owned(), Value::Float(32.0)),
                    ]),
                ),
                ("SenPos".to_owned(), Value::Union(Box::new(Value::Null))),
                ("SenTemp".to_owned(), Value::Union(Box::new(Value::Null))),
                (
                    "SenTime".to_owned(),
                    Value::Record(vec![
                        ("TimeSecs".to_owned(), Value::Long(time.as_secs() as i64)),
                        (
                            "TimeMicrosecs".to_owned(),
                            Value::Int(time.subsec_millis() as i32),
                        ),
                    ]),
                ),
                (
                    "SenData".to_owned(),
                    Value::Record(vec![
                        ("CenterFreq".to_owned(), Value::Long(100_000_000)),
                        ("SquaredMag".to_owned(), Value::Array(data)),
                    ]),
                ),
            ]);

            let mut data = avro_rs::to_avro_datum(schema, record).unwrap();
            let before = data.len();
            if data.len() % 4 != 0 {
                for _ in 0..(4 - data.len() % 4) {
                    data.push(0);
                }
            }

            let mut send = Vec::new();
            send.extend_from_slice(&(data.len() as u32).to_be_bytes());
            send.extend_from_slice(&(1024 as u32).to_be_bytes());
            send.extend_from_slice(&data);

            println!("###### Sending PSD data (len {} - {})", before, data.len());

            stream.write_all(&send).await?;
        }

        if sio.input(0).finished() {
            io.finished = true;
        }

        println!("###### Done work() n {}", n);
        sio.input(0).consume(n * 2048);

        Ok(())
    }
}
