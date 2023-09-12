use futuresdr::num_complex::Complex32;
use rustfft::Fft;
use rustfft::FftPlanner;
use std::sync::Arc;

const BASE37_BITMAP: [u8; 407] = [
    0, 60, 8, 60, 60, 2, 126, 28, 126, 60, 60, 60, 124, 60, 120, 126, 126, 60, 66, 56, 14, 66, 64,
    130, 66, 60, 124, 60, 124, 60, 254, 66, 66, 130, 66, 130, 126, 0, 66, 24, 66, 66, 6, 64, 32, 2,
    66, 66, 66, 66, 66, 68, 64, 64, 66, 66, 16, 4, 68, 64, 198, 66, 66, 66, 66, 66, 66, 16, 66, 66,
    130, 66, 130, 2, 0, 66, 40, 66, 66, 10, 64, 64, 2, 66, 66, 66, 66, 66, 66, 64, 64, 66, 66, 16,
    4, 72, 64, 170, 66, 66, 66, 66, 66, 64, 16, 66, 66, 130, 36, 68, 2, 0, 70, 8, 2, 2, 18, 64, 64,
    4, 66, 66, 66, 66, 64, 66, 64, 64, 64, 66, 16, 4, 80, 64, 146, 98, 66, 66, 66, 66, 64, 16, 66,
    66, 130, 36, 68, 4, 0, 74, 8, 4, 28, 34, 124, 124, 4, 60, 66, 66, 124, 64, 66, 120, 120, 64,
    126, 16, 4, 96, 64, 146, 82, 66, 66, 66, 66, 60, 16, 66, 66, 130, 24, 40, 8, 0, 82, 8, 8, 2,
    66, 2, 66, 8, 66, 62, 126, 66, 64, 66, 64, 64, 78, 66, 16, 4, 96, 64, 130, 74, 66, 124, 66,
    124, 2, 16, 66, 36, 146, 24, 16, 16, 0, 98, 8, 16, 2, 126, 2, 66, 8, 66, 2, 66, 66, 64, 66, 64,
    64, 66, 66, 16, 4, 80, 64, 130, 70, 66, 64, 66, 80, 2, 16, 66, 36, 146, 36, 16, 32, 0, 66, 8,
    32, 66, 2, 2, 66, 16, 66, 2, 66, 66, 66, 66, 64, 64, 66, 66, 16, 68, 72, 64, 130, 66, 66, 64,
    66, 72, 66, 16, 66, 36, 170, 36, 16, 64, 0, 66, 8, 64, 66, 2, 66, 66, 16, 66, 4, 66, 66, 66,
    68, 64, 64, 66, 66, 16, 68, 68, 64, 130, 66, 66, 64, 74, 68, 66, 16, 66, 24, 198, 66, 16, 64,
    0, 60, 62, 126, 60, 2, 60, 60, 16, 60, 56, 66, 124, 60, 120, 126, 64, 60, 66, 56, 56, 66, 126,
    130, 66, 60, 64, 60, 66, 60, 16, 60, 24, 130, 66, 16, 126, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

fn xor_be_bit(buf: &mut [u8], pos: usize, val: bool) {
    buf[pos / 8] ^= (val as u8) << (7 - pos % 8);
}

fn set_be_bit(buf: &mut [u8], pos: usize, val: bool) {
    let val = val as u8;
    buf[pos / 8] = (!(1 << (7 - pos % 8)) & buf[pos / 8]) | (val << (7 - pos % 8));
}

fn get_be_bit(buf: &[u8], pos: usize) -> bool {
    (buf[pos / 8] >> (7 - pos % 8)) & 1 == 1
}

struct Bch {
    generator: [u8; Self::G],
}

impl Bch {
    const LEN: usize = 255;
    const MSG: usize = 71;
    const N: usize = Self::LEN;
    const K: usize = Self::MSG;
    const NP: usize = Self::N - Self::K;
    const G: usize = ((Self::NP + 1) + 7) / 8;

    fn slb1(buf: &[u8], pos: usize) -> u8 {
        (buf[pos] << 1) | (buf[pos + 1] >> 7)
    }

    fn new(minimal_polynomials: &[i64]) -> Self {
        let mut generator_degree = 1;
        let mut generator = [0; Self::G];

        set_be_bit(generator.as_mut_slice(), Self::NP, true);

        for m in minimal_polynomials.iter().copied() {
            assert!(0 < m);
            let mut degree = 0;
            while (m >> degree) > 0 {
                degree += 1;
            }
            degree -= 1;
            assert!(generator_degree + degree <= Self::NP + 1);
            for i in generator_degree..=0 {
                if !get_be_bit(generator.as_slice(), Self::NP - i) {
                    continue;
                }
                set_be_bit(generator.as_mut_slice(), Self::NP - i, m & 1 == 1);
                for j in 1..=degree {
                    xor_be_bit(
                        generator.as_mut_slice(),
                        Self::NP - (i + j),
                        ((m >> j) & 1) == 1,
                    );
                }
            }
            generator_degree += degree;
        }

        assert_eq!(generator_degree, Self::NP + 1);

        for i in 0..Self::NP {
            let v = get_be_bit(generator.as_slice(), i + 1);
            set_be_bit(generator.as_mut_slice(), i, v);
        }
        set_be_bit(generator.as_mut_slice(), Self::NP, false);

        Self { generator }
    }

    fn process(&mut self, data: &[u8], parity: &mut [u8]) {
        let data_len = Self::K;
        assert!(0 < data_len);
        assert!(data_len <= Self::K);

        for l in 0..=(Self::NP - 1) / 8 {
            parity[l] = 0;
        }

        for i in 0..data_len {
            if get_be_bit(data, i) != get_be_bit(parity, 0) {
                for l in 0..(Self::NP - 1) / 8 {
                    parity[l] = self.generator[l] ^ Self::slb1(parity, l);
                }
                parity[(Self::NP - 1) / 8] =
                    self.generator[(Self::NP - 1) / 8] ^ (parity[(Self::NP - 1) / 8] << 1);
            } else {
                for l in 0..(Self::NP - 1) / 8 {
                    parity[l] = Self::slb1(parity, l);
                }
                parity[(Self::NP - 1) / 8] <<= 1;
            }
        }
    }
}

struct Crc {
    crc: u16,
    poly: u16,
    lut: [u16; 256],
}

impl Crc {
    fn new(poly: u16) -> Self {
        let mut lut = [0; 256];
        for j in 0..256u16 {
            let mut tmp = j;
            for _ in 0..8 {
                tmp = Self::update(tmp, false, poly);
            }
            lut[j as usize] = tmp;
        }

        Self { crc: 0, poly, lut }
    }

    fn reset(&mut self) {
        self.crc = 0;
    }

    fn update(prev: u16, data: bool, poly: u16) -> u16 {
        let tmp = prev ^ data as u16;
        (prev >> 1) ^ ((tmp & 1) * poly)
    }

    fn add_u8(&mut self, data: u8) -> u16 {
        let tmp = self.crc ^ data as u16;
        self.crc = (self.crc >> 8) ^ self.lut[(tmp & 255) as usize];
        self.crc
    }

    fn add_u64(&mut self, data: u64) -> u16 {
        self.add_u8((data & 0xff) as u8);
        self.add_u8(((data >> 8) & 0xff) as u8);
        self.add_u8(((data >> 16) & 0xff) as u8);
        self.add_u8(((data >> 24) & 0xff) as u8);
        self.add_u8(((data >> 32) & 0xff) as u8);
        self.add_u8(((data >> 40) & 0xff) as u8);
        self.add_u8(((data >> 48) & 0xff) as u8);
        self.add_u8(((data >> 56) & 0xff) as u8);
        self.crc
    }
}

struct Mls {
    poly: u64,
    test: u64,
    reg: u64,
}

impl Mls {
    fn new(poly: u64) -> Self {
        Self {
            poly,
            test: Self::hibit(poly) >> 1,
            reg: 1,
        }
    }

    fn hibit(mut n: u64) -> u64 {
        n |= n >> 1;
        n |= n >> 2;
        n |= n >> 4;
        n |= n >> 8;
        n |= n >> 16;
        n ^ (n >> 1)
    }

    fn reset(&mut self, r: Option<u64>) {
        self.reg = r.unwrap_or(1);
    }

    fn next(&mut self) -> bool {
        let fb = self.reg & self.test;
        self.reg <<= 1;
        self.reg ^= fb * self.poly;
        fb != 0
    }

    fn bad(&mut self, r: Option<u64>) -> bool {
        let r = r.unwrap_or(1);
        self.reg = r;
        let len = Self::hibit(self.poly) - 1;

        for i in 1..len {
            self.next();
            if self.reg == r {
                return true;
            }
        }

        self.next();
        self.reg != r
    }
}

pub struct Psk<const N: usize> {}

impl Psk<4> {
    fn map(b: &[bool; 2]) -> Complex32 {
        const A: f32 = std::f32::consts::FRAC_1_SQRT_2;

        match b {
            [true, true] => Complex32::new(A, A),
            [true, false] => Complex32::new(A, -A),
            [false, true] => Complex32::new(-A, A),
            [false, false] => Complex32::new(-A, -A),
        }
    }
}

enum OperationMode {
    Null,
    Mode14,
    Mode15,
    Mode16,
}

pub struct Encoder {
    temp: [Complex32; Self::EXTENDED_LENGTH],
    freq: [Complex32; Self::SYMBOL_LENGTH],
    prev: [Complex32; Self::PAY_CAR_CNT],
    noise_seq: Mls,
    symbol_number: usize,
    code: [bool; Self::CODE_LEN],
    carrier_offset: usize,
    fft_scratch: [Complex32; Self::SYMBOL_LENGTH],
    fft: Arc<dyn Fft<f32>>,
    fancy_line: usize,
    meta_data: u64,
    crc: Crc,
    bch: Bch,
}

impl Encoder {
    const RATE: usize = 48000;
    const CODE_ORDER: usize = 11;
    const MOD_BITS: usize = 2;
    const CODE_LEN: usize = 1 << Self::CODE_ORDER;
    const SYMBOL_COUNT: usize = 4;
    const SYMBOL_LENGTH: usize = (1280 * Self::RATE) / 8000;
    const GUARD_LENGTH: usize = Self::SYMBOL_LENGTH / 8;
    const EXTENDED_LENGTH: usize = Self::SYMBOL_LENGTH + Self::GUARD_LENGTH;
    const MAX_BITS: usize = 1360;
    const COR_SEQ_LEN: isize = 127;
    const COR_SEQ_OFF: isize = 1 - Self::COR_SEQ_LEN;
    const COR_SEQ_POLY: u64 = 0b10001001;
    const PRE_SEQ_LEN: isize = 255;
    const PRE_SEQ_OFF: isize = -Self::PRE_SEQ_LEN / 2;
    const PRE_SEQ_POLY: u64 = 0b100101011;
    const PAY_CAR_CNT: usize = 256;
    const PAY_CAR_OFF: isize = -(Self::PAY_CAR_CNT as isize) / 2;
    const FANCY_OFF: isize = -(8 * 9 * 3) / 2;
    const NOISE_POLY: u64 = 0b100101010001;

    fn new() -> Self {
        let mut fft_planner = FftPlanner::new();
        let fft = fft_planner.plan_fft_forward(Self::SYMBOL_LENGTH);

        let bch = Bch::new(&[
            0b100011101,
            0b101110111,
            0b111110011,
            0b101101001,
            0b110111101,
            0b111100111,
            0b100101011,
            0b111010111,
            0b000010011,
            0b101100101,
            0b110001011,
            0b101100011,
            0b100011011,
            0b100111111,
            0b110001101,
            0b100101101,
            0b101011111,
            0b111111001,
            0b111000011,
            0b100111001,
            0b110101001,
            0b000011111,
            0b110000111,
            0b110110001,
        ]);

        Self {
            temp: [Complex32::new(0.0, 0.0); Self::EXTENDED_LENGTH],
            freq: [Complex32::new(0.0, 0.0); Self::SYMBOL_LENGTH],
            prev: [Complex32::new(0.0, 0.0); Self::PAY_CAR_CNT],
            noise_seq: Mls::new(Self::NOISE_POLY),
            symbol_number: 0,
            code: [false; Self::CODE_LEN],
            carrier_offset: 0,
            fft_scratch: [Complex32::new(0.0, 0.0); Self::SYMBOL_LENGTH],
            fft,
            fancy_line: 0,
            meta_data: 0,
            crc: Crc::new(0xA8F4),
            bch,
        }
    }

    pub fn encode(
        &self,
        payload: &[u8],
        call_sign: &[u8],
        carrier_frequency: u64,
        noise_symbols: u64,
        fancy_header: bool,
    ) -> Vec<f32> {
        let len = payload.len();

        // 	void configure(const uint8_t *payload, const int8_t *call_sign, int carrier_frequency, int noise_symbols, bool fancy_header) final {
        // 		int len = 0;
        // 		while (len <= 128 && payload[len])
        // 			++len;
        // 		if (!len)
        // 			operation_mode = 0;
        // 		else if (len <= 85)
        // 			operation_mode = 16;
        // 		else if (len <= 128)
        // 			operation_mode = 15;
        // 		else
        // 			operation_mode = 14;
        // 		carrier_offset = (carrier_frequency * symbol_length) / RATE;
        // 		meta_data = (base37(call_sign) << 8) | operation_mode;
        // 		for (int i = 0; i < 9; ++i)
        // 			call[i] = 0;
        // 		for (int i = 0; i < 9 && call_sign[i]; ++i)
        // 			call[i] = base37_map(call_sign[i]);
        // 		symbol_number = 0;
        // 		count_down = 5;
        // 		fancy_line = 11 * fancy_header;
        // 		noise_count = noise_symbols;
        // 		for (int i = 0; i < guard_length; ++i)
        // 			guard[i] = 0;
        // 		const uint32_t *frozen_bits;
        // 		int data_bits;
        // 		switch (operation_mode) {
        // 			case 14:
        // 				data_bits = 1360;
        // 				frozen_bits = frozen_2048_1392;
        // 				break;
        // 			case 15:
        // 				data_bits = 1024;
        // 				frozen_bits = frozen_2048_1056;
        // 				break;
        // 			case 16:
        // 				data_bits = 680;
        // 				frozen_bits = frozen_2048_712;
        // 				break;
        // 			default:
        // 				return;
        // 		}
        // 		CODE::Xorshift32 scrambler;
        // 		for (int i = 0; i < data_bits / 8; ++i)
        // 			mesg[i] = payload[i] ^ scrambler();
        // 		polar(code, mesg, frozen_bits, data_bits);
        // 	}

        Vec::new()
    }

    pub fn rate() -> usize {
        Self::RATE
    }

    fn nrz(bit: bool) -> f32 {
        if bit {
            -1.0
        } else {
            1.0
        }
    }

    fn bin(&self, carrier: isize) -> usize {
        (carrier + self.carrier_offset as isize + Self::SYMBOL_LENGTH as isize) as usize
            % Self::SYMBOL_LENGTH
    }

    fn mod_map(b: &[bool; Self::MOD_BITS]) -> Complex32 {
        Psk::<4>::map(b)
    }

    fn base37(str: &[u8]) -> u64 {
        fn base37_map(c: u8) -> u8 {
            if c >= b'0' && c <= b'9' {
                return c - b'0' + 1;
            }
            if c >= b'a' && c <= b'z' {
                return c - b'a' + 11;
            }
            if c >= b'A' && c <= b'Z' {
                return c - b'A' + 11;
            }
            0
        }

        let mut acc = 0u64;
        for c in str {
            acc = 37 * acc + base37_map(*c) as u64;
        }
        acc
    }

    fn noise_symbol(&mut self) {
        let factor = Self::SYMBOL_LENGTH as f32 / Self::PAY_CAR_CNT as f32;
        self.freq.fill(Complex32::new(0.0, 0.0));
        for i in 0..Self::PAY_CAR_CNT {
            self.freq[self.bin(i as isize + Self::PAY_CAR_OFF)] = factor
                * Complex32::new(
                    Self::nrz(self.noise_seq.next()),
                    Self::nrz(self.noise_seq.next()),
                );
        }
        self.transform(false);
    }

    fn payload_symbol(&mut self) {
        self.freq.fill(Complex32::new(0.0, 0.0));

        for i in 0..Self::PAY_CAR_CNT {
            let index = Self::MOD_BITS * (Self::PAY_CAR_CNT * self.symbol_number + i);
            self.prev[i] *= Self::mod_map(&self.code[index..index + 2].try_into().unwrap());
            self.freq[self.bin(i as isize + Self::PAY_CAR_OFF)] = self.prev[i];
        }
        self.transform(true);
    }

    fn silence(&mut self) {
        self.temp.fill(Complex32::new(0.0, 0.0));
    }

    fn transform(&mut self, _papr_reduction: bool) {
        // TODO
        // if papr_reduction && RATE <= 16000 {
        // 	improve_papr(freq);
        //         }
        self.fft.process_outofplace_with_scratch(
            self.freq.as_mut_slice(),
            self.temp.as_mut_slice(),
            self.fft_scratch.as_mut_slice(),
        );
        for i in 0..Self::SYMBOL_LENGTH {
            self.temp[i] /= ((8 * Self::SYMBOL_LENGTH) as f32).sqrt();
        }
    }

    fn schmidl_cox(&mut self) {
        let mut seq = Mls::new(Self::COR_SEQ_POLY);
        let factor = (2 * Self::SYMBOL_LENGTH) as f32 / Self::COR_SEQ_LEN as f32;
        let factor = factor.sqrt();

        self.freq.fill(Complex32::new(0.0, 0.0));
        self.freq[self.bin(Self::COR_SEQ_OFF - 2)] = Complex32::new(factor, 0.0);

        for i in 0..Self::COR_SEQ_LEN {
            self.freq[self.bin(2 * i + Self::COR_SEQ_OFF)] =
                Complex32::new(Self::nrz(seq.next()), 0.0);
        }

        for i in 0..Self::COR_SEQ_LEN {
            self.freq[self.bin(2 * i + Self::COR_SEQ_OFF)] *=
                self.freq[self.bin(2 * (i - 1) + Self::COR_SEQ_OFF)];
        }
        self.transform(false);
    }

    fn fancy_symbol(&mut self, call: &[u8]) {
        let mut active_carriers = 1;

        for j in 0..9 {
            for i in 0..8 {
                active_carriers +=
                    (BASE37_BITMAP[call[j] as usize + 37 * self.fancy_line] >> i) & 1;
            }
        }

        let factor = Self::SYMBOL_LENGTH as f32 / active_carriers as f32;
        let factor = factor.sqrt();

        self.freq.fill(Complex32::new(0.0, 0.0));

        for j in 0..9isize {
            for i in 0..8isize {
                if (BASE37_BITMAP[call[j as usize] as usize + 37 * self.fancy_line]
                    & (1 << (7 - i)))
                    != 0
                {
                    self.freq[self.bin((8 * j + i) * 3 + Self::FANCY_OFF)] =
                        Complex32::new(factor * Self::nrz(self.noise_seq.next()), 0.0);
                }
            }
        }
        self.transform(false);
    }
    fn preamble(&mut self) {
        let mut data = [0u8; 9];
        let mut parity = [0u8; 23];

        for i in 0..55 {
            set_be_bit(data.as_mut_slice(), i, ((self.meta_data >> i) & 1) == 1);
        }

        self.crc.reset();
        let cs = self.crc.add_u64(self.meta_data << 9);

        for i in 0..16 {
            set_be_bit(data.as_mut_slice(), i + 55, ((cs >> i) & 1) == 1);
        }

        // bch(data, parity); TODO

        let mut seq = Mls::new(Self::PRE_SEQ_POLY);
        let factor = Self::SYMBOL_LENGTH as f32 / Self::PRE_SEQ_LEN as f32;
        let factor = factor.sqrt();
        self.freq.fill(Complex32::new(0.0, 0.0));

        self.freq[self.bin(Self::PRE_SEQ_OFF - 1)] = Complex32::new(factor, 0.0);

        for i in 0..71 {
            self.freq[self.bin(i + Self::PRE_SEQ_OFF)] =
                Self::nrz(get_be_bit(data.as_slice(), i as usize)).into();
        }

        for i in 71..Self::PRE_SEQ_LEN {
            self.freq[self.bin(i + Self::PRE_SEQ_OFF)] =
                Self::nrz(get_be_bit(parity.as_slice(), (i - 71) as usize)).into();
        }

        for i in 0..Self::PRE_SEQ_LEN {
            self.freq[self.bin(i + Self::PRE_SEQ_OFF)] *=
                self.freq[self.bin(i - 1 + Self::PRE_SEQ_OFF)];
        }

        for i in 0..Self::PRE_SEQ_LEN {
            self.freq[self.bin(i + Self::PRE_SEQ_OFF)] *= Self::nrz(seq.next());
        }

        for i in 0..Self::PAY_CAR_CNT {
            self.prev[i] = self.freq[self.bin(i as isize + Self::PAY_CAR_OFF)];
        }

        self.transform(true);
    }
}

// template<int RATE>
// class Encoder : public EncoderInterface {
// 	DSP::FastFourierTransform<symbol_length, cmplx, 1> bwd;
// 	CODE::CRC<uint16_t> crc;
// 	CODE::BoseChaudhuriHocquenghemEncoder<255, 71> bch;
// 	CODE::MLS noise_seq;
// 	ImprovePAPR<cmplx, symbol_length, RATE <= 16000 ? 4 : 1> improve_papr;
// 	PolarEncoder<code_type> polar;
// 	cmplx temp[extended_length], freq[symbol_length], prev[pay_car_cnt], guard[guard_length];
// 	uint8_t mesg[max_bits / 8], call[9];
// 	code_type code[code_len];
// 	uint64_t meta_data;
// 	int operation_mode = 0;
// 	int carrier_offset = 0;
// 	int symbol_number = symbol_count;
// 	int count_down = 0;
// 	int fancy_line = 0;
// 	int noise_count = 0;
//
//
//
//
//
// public:
// 	Encoder() : noise_seq(noise_poly), crc(0xA8F4), bch({
// 		0b100011101, 0b101110111, 0b111110011, 0b101101001,
// 		0b110111101, 0b111100111, 0b100101011, 0b111010111,
// 		0b000010011, 0b101100101, 0b110001011, 0b101100011,
// 		0b100011011, 0b100111111, 0b110001101, 0b100101101,
// 		0b101011111, 0b111111001, 0b111000011, 0b100111001,
// 		0b110101001, 0b000011111, 0b110000111, 0b110110001}) {}
//
// 	int rate() final {
// 		return RATE;
// 	}
//
// 	bool produce(int16_t *audio_buffer, int channel_select) final {
// 		bool data_symbol = false;
// 		switch (count_down) {
// 			case 5:
// 				if (noise_count) {
// 					--noise_count;
// 					noise_symbol();
// 					break;
// 				}
// 				--count_down;
// 			case 4:
// 				schmidl_cox();
// 				data_symbol = true;
// 				--count_down;
// 				break;
// 			case 3:
// 				preamble();
// 				data_symbol = true;
// 				--count_down;
// 				if (!operation_mode)
// 					--count_down;
// 				break;
// 			case 2:
// 				payload_symbol();
// 				data_symbol = true;
// 				if (++symbol_number == symbol_count)
// 					--count_down;
// 				break;
// 			case 1:
// 				if (fancy_line) {
// 					--fancy_line;
// 					fancy_symbol();
// 					break;
// 				}
// 				silence();
// 				--count_down;
// 				break;
// 			default:
// 				for (int i = 0; i < extended_length; ++i)
// 					next_sample(audio_buffer, 0, channel_select, i);
// 				return false;
// 		}
// 		for (int i = 0; i < guard_length; ++i) {
// 			float x = i / float(guard_length - 1);
// 			float ratio(0.5);
// 			if (data_symbol)
// 				x = std::min(x, ratio) / ratio;
// 			float y = 0.5f * (1 - std::cos(DSP::Const<float>::Pi() * x));
// 			cmplx sum = DSP::lerp(guard[i], temp[i + symbol_length - guard_length], y);
// 			next_sample(audio_buffer, sum, channel_select, i);
// 		}
// 		for (int i = 0; i < guard_length; ++i)
// 			guard[i] = temp[i];
// 		for (int i = 0; i < symbol_length; ++i)
// 			next_sample(audio_buffer, temp[i], channel_select, i + guard_length);
// 		return true;
// 	}
//
// 	void configure(const uint8_t *payload, const int8_t *call_sign, int carrier_frequency, int noise_symbols, bool fancy_header) final {
// 		int len = 0;
// 		while (len <= 128 && payload[len])
// 			++len;
// 		if (!len)
// 			operation_mode = 0;
// 		else if (len <= 85)
// 			operation_mode = 16;
// 		else if (len <= 128)
// 			operation_mode = 15;
// 		else
// 			operation_mode = 14;
// 		carrier_offset = (carrier_frequency * symbol_length) / RATE;
// 		meta_data = (base37(call_sign) << 8) | operation_mode;
// 		for (int i = 0; i < 9; ++i)
// 			call[i] = 0;
// 		for (int i = 0; i < 9 && call_sign[i]; ++i)
// 			call[i] = base37_map(call_sign[i]);
// 		symbol_number = 0;
// 		count_down = 5;
// 		fancy_line = 11 * fancy_header;
// 		noise_count = noise_symbols;
// 		for (int i = 0; i < guard_length; ++i)
// 			guard[i] = 0;
// 		const uint32_t *frozen_bits;
// 		int data_bits;
// 		switch (operation_mode) {
// 			case 14:
// 				data_bits = 1360;
// 				frozen_bits = frozen_2048_1392;
// 				break;
// 			case 15:
// 				data_bits = 1024;
// 				frozen_bits = frozen_2048_1056;
// 				break;
// 			case 16:
// 				data_bits = 680;
// 				frozen_bits = frozen_2048_712;
// 				break;
// 			default:
// 				return;
// 		}
// 		CODE::Xorshift32 scrambler;
// 		for (int i = 0; i < data_bits / 8; ++i)
// 			mesg[i] = payload[i] ^ scrambler();
// 		polar(code, mesg, frozen_bits, data_bits);
// 	}
// };
