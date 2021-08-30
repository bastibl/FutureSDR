use anyhow::Result;
use async_trait::async_trait;

use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;

fn main() -> Result<()> {
    let mut fg = Flowgraph::new();
    let rt = Runtime::new();

    let src = SigSource::new();
    let src = fg.add_block(src);
    let snk = AudioSink::new();
    let snk = fg.add_block(snk);

    fg.connect_stream(src, "out", snk, "in")?;

    rt.run(fg)?;

    Ok(())
}

use async_io::block_on;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::BufferSize;
use cpal::SampleRate;
use cpal::Stream;
use cpal::StreamConfig;
use futures::channel::mpsc;
use futures::channel::oneshot;
use futures::SinkExt;
use futures::StreamExt;
use rodio::source::{SineWave, Source};

use futuresdr::runtime::AsyncKernel;
use futuresdr::runtime::Block;
use futuresdr::runtime::BlockMeta;
use futuresdr::runtime::BlockMetaBuilder;
use futuresdr::runtime::MessageIo;
use futuresdr::runtime::MessageIoBuilder;
use futuresdr::runtime::StreamIo;
use futuresdr::runtime::StreamIoBuilder;
use futuresdr::runtime::WorkIo;

#[allow(clippy::type_complexity)]
pub struct AudioSink {
    stream: Option<Stream>,
    rx: Option<mpsc::UnboundedReceiver<(usize, oneshot::Sender<Box<[f32]>>)>>,
    buff: Option<(Box<[f32]>, usize, oneshot::Sender<Box<[f32]>>)>,
}

unsafe impl Send for AudioSink {}

impl AudioSink {
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> Block {
        Block::new_async(
            BlockMetaBuilder::new("AudioSink").build(),
            StreamIoBuilder::new().add_stream_input("in", 4).build(),
            MessageIoBuilder::new().build(),
            AudioSink {
                stream: None,
                rx: None,
                buff: None,
            },
        )
    }
}

#[async_trait]
impl AsyncKernel for AudioSink {
    async fn init(
        &mut self,
        _s: &mut StreamIo,
        _m: &mut MessageIo<Self>,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .expect("no output device available");

        let config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(48000),
            buffer_size: BufferSize::Default,
        };

        let (mut tx, rx) = mpsc::unbounded();

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let (my_tx, my_rx) = oneshot::channel::<Box<[f32]>>();
                let samples = block_on(async {
                    tx.send((data.len(), my_tx)).await.unwrap();
                    my_rx.await.unwrap()
                });
                assert_eq!(data.len(), samples.len());
                for (i, s) in samples.iter().enumerate() {
                    data[i] = *s;
                }
            },
            move |err| {
                panic!("cpal stream error {:?}", err);
            },
        )?;

        stream.play()?;

        self.rx = Some(rx);
        self.stream = Some(stream);

        Ok(())
    }

    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        if let Some((mut buff, mut full, tx)) = self.buff.take() {
            let i = sio.input(0).slice::<f32>();
            let n = std::cmp::min(i.len(), buff.len() - full);

            for (i, s) in i.iter().take(n).enumerate() {
                buff[full + i] = *s;
            }

            full += n;

            if buff.len() == full {
                tx.send(buff).unwrap();
                self.buff = None;
            } else {
                self.buff = Some((buff, full, tx));
            }

            sio.input(0).consume(n);
        } else if let Some((n, tx)) = self.rx.as_mut().unwrap().next().await {
            io.call_again = true;
            self.buff = Some((vec![0f32; n].into_boxed_slice(), 0, tx));
        } else {
            io.finished = true;
        }

        Ok(())
    }
}

pub struct SigSource {
    src: Box<dyn Iterator<Item = f32> + Send>,
}

impl SigSource {
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> Block {
        Block::new_async(
            BlockMetaBuilder::new("SigSource").build(),
            StreamIoBuilder::new().add_stream_output("out", 4).build(),
            MessageIoBuilder::new().build(),
            SigSource {
                src: Box::new(SineWave::new(440).amplify(0.20)),
            },
        )
    }
}

#[async_trait]
impl AsyncKernel for SigSource {
    async fn work(
        &mut self,
        _io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let out = sio.output(0).slice::<f32>();

        for (i, v) in self.src.by_ref().take(out.len()).enumerate() {
            out[i] = v;
        }
        sio.output(0).produce(out.len());

        Ok(())
    }
}
