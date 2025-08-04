use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;
use futuresdr::runtime::mocker;
use std::iter::repeat_with;

use futuresdr::blocks::Apply;
use futuresdr::runtime::mocker::Mocker;
use futuresdr::runtime::mocker::Reader;
use futuresdr::runtime::mocker::Writer;
use futuresdr::prelude::*;

#[derive(Block)]
struct Add {
    #[input]
    input: mocker::Reader<u8>,
    #[output]
    output: mocker::Writer<u8>,
}

impl Kernel for Add {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _m: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let i = self.input.slice();
        let o = self.output.slice();
        let i_len = i.len();

        let n = std::cmp::min(i_len, o.len());

        for (i, o) in i.iter().zip(o.iter_mut()) {
            *o = i.wrapping_add(1);
        }

        self.input.consume(n);
        self.output.produce(n);

        if self.input.finished() && i_len == n {
            io.finished = true;
        }

        Ok(())
    }
}

#[derive(Block)]
struct AddChunk {
    #[input]
    input: mocker::Reader<u8>,
    #[output]
    output: mocker::Writer<u8>,
}

impl Kernel for AddChunk {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _m: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let i = self.input.slice();
        let o = self.output.slice();
        let i_len = i.len();

        let n = std::cmp::min(i_len, o.len());

        for (i, o) in i.chunks_exact(16).zip(o.chunks_exact_mut(16)) {
            for x in 0..16 {
                o[x] = i[x].wrapping_add(1);
            }
            // o[0] = i[0].wrapping_add(1);
            // o[1] = i[1].wrapping_add(1);
            // o[2] = i[2].wrapping_add(1);
            // o[3] = i[3].wrapping_add(1);
            // o[4] = i[4].wrapping_add(1);
            // o[5] = i[5].wrapping_add(1);
            // o[6] = i[6].wrapping_add(1);
            // o[7] = i[7].wrapping_add(1);
            // o[8] = i[8].wrapping_add(1);
            // o[9] = i[9].wrapping_add(1);
            // o[10] = i[10].wrapping_add(1);
            // o[11] = i[11].wrapping_add(1);
            // o[12] = i[12].wrapping_add(1);
            // o[13] = i[13].wrapping_add(1);
            // o[14] = i[14].wrapping_add(1);
            // o[15] = i[15].wrapping_add(1);
        }

        self.input.consume(n);
        self.output.produce(n);

        if self.input.finished() && i_len == n {
            io.finished = true;
        }

        Ok(())
    }
}

pub fn apply(c: &mut Criterion) {
    let n_samp = 1024 * 1024;
    let input: Vec<u8> = repeat_with(rand::random::<u8>).take(n_samp).collect();

    let mut group = c.benchmark_group("apply");

    group.throughput(criterion::Throughput::Elements(n_samp as u64));

    group.bench_function(format!("apply-u8-plus-1-{n_samp}"), |b| {
        let block: Apply<_, _, _, Reader<u8>, Writer<u8>> = Apply::new(|x: &u8| x.wrapping_add(1));
        let mut mocker = Mocker::new(block);
        mocker.input().set(input.clone());
        mocker.output().reserve(n_samp);

        b.iter(move || {
            mocker.run();
        });
    });

    group.bench_function(format!("block-u8-plus-1-{n_samp}"), |b| {
        let block = Add { input: Default::default(), output: Default::default() };
        let mut mocker = Mocker::new(block);
        mocker.input().set(input.clone());
        mocker.output().reserve(n_samp);

        b.iter(move || {
            mocker.run();
        });
    });

    group.bench_function(format!("chunks-u8-plus-1-{n_samp}"), |b| {
        let block = AddChunk { input: Default::default(), output: Default::default() };
        let mut mocker = Mocker::new(block);
        mocker.input().set(input.clone());
        mocker.output().reserve(n_samp);

        b.iter(move || {
            mocker.run();
        });
    });

    group.finish();
}

criterion_group!(benches, apply);
criterion_main!(benches);
