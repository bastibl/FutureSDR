mod call_sign;
pub use call_sign::CallSign;

mod crc;
pub use crc::Crc;

mod encoder;
pub use encoder::Encoder;

mod encoder_block;
pub use encoder_block::EncoderBlock;

mod decoder;
pub use decoder::Decoder;

mod decoder_block;
pub use decoder_block::DecoderBlock;

mod golay;
pub use golay::Golay;

mod lsf;
pub use lsf::LinkSetupFrame;

mod moving_average;
pub use moving_average::MovingAverage;

mod symbol_sync;
pub use symbol_sync::SymbolSync;

const PUNCTERING_1: [u8; 61] = [
    1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1,
    1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1,
];
const PUNCTERING_2: [u8; 12] = [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0];

const SYNC_LSF: u16 = 0x55F7;
const SYNC_STR: u16 = 0xFF5D;
// const SYNC_PKT: u16 = 0x75FF;
// const SYNC_BER: u16 = 0xDF55;
// const EOT_MRKR: u16 = 0x555D;

const INTERLEAVER: [usize; 368] = [
    0, 137, 90, 227, 180, 317, 270, 39, 360, 129, 82, 219, 172, 309, 262, 31, 352, 121, 74, 211,
    164, 301, 254, 23, 344, 113, 66, 203, 156, 293, 246, 15, 336, 105, 58, 195, 148, 285, 238, 7,
    328, 97, 50, 187, 140, 277, 230, 367, 320, 89, 42, 179, 132, 269, 222, 359, 312, 81, 34, 171,
    124, 261, 214, 351, 304, 73, 26, 163, 116, 253, 206, 343, 296, 65, 18, 155, 108, 245, 198, 335,
    288, 57, 10, 147, 100, 237, 190, 327, 280, 49, 2, 139, 92, 229, 182, 319, 272, 41, 362, 131,
    84, 221, 174, 311, 264, 33, 354, 123, 76, 213, 166, 303, 256, 25, 346, 115, 68, 205, 158, 295,
    248, 17, 338, 107, 60, 197, 150, 287, 240, 9, 330, 99, 52, 189, 142, 279, 232, 1, 322, 91, 44,
    181, 134, 271, 224, 361, 314, 83, 36, 173, 126, 263, 216, 353, 306, 75, 28, 165, 118, 255, 208,
    345, 298, 67, 20, 157, 110, 247, 200, 337, 290, 59, 12, 149, 102, 239, 192, 329, 282, 51, 4,
    141, 94, 231, 184, 321, 274, 43, 364, 133, 86, 223, 176, 313, 266, 35, 356, 125, 78, 215, 168,
    305, 258, 27, 348, 117, 70, 207, 160, 297, 250, 19, 340, 109, 62, 199, 152, 289, 242, 11, 332,
    101, 54, 191, 144, 281, 234, 3, 324, 93, 46, 183, 136, 273, 226, 363, 316, 85, 38, 175, 128,
    265, 218, 355, 308, 77, 30, 167, 120, 257, 210, 347, 300, 69, 22, 159, 112, 249, 202, 339, 292,
    61, 14, 151, 104, 241, 194, 331, 284, 53, 6, 143, 96, 233, 186, 323, 276, 45, 366, 135, 88,
    225, 178, 315, 268, 37, 358, 127, 80, 217, 170, 307, 260, 29, 350, 119, 72, 209, 162, 299, 252,
    21, 342, 111, 64, 201, 154, 291, 244, 13, 334, 103, 56, 193, 146, 283, 236, 5, 326, 95, 48,
    185, 138, 275, 228, 365, 318, 87, 40, 177, 130, 267, 220, 357, 310, 79, 32, 169, 122, 259, 212,
    349, 302, 71, 24, 161, 114, 251, 204, 341, 294, 63, 16, 153, 106, 243, 196, 333, 286, 55, 8,
    145, 98, 235, 188, 325, 278, 47,
];

const RAND_SEQ: [usize; 46] = [
    0xD6, 0xB5, 0xE2, 0x30, 0x82, 0xFF, 0x84, 0x62, 0xBA, 0x4E, 0x96, 0x90, 0xD8, 0x98, 0xDD, 0x5D,
    0x0C, 0xC8, 0x52, 0x43, 0x91, 0x1D, 0xF8, 0x6E, 0x68, 0x2F, 0x35, 0xDA, 0x14, 0xEA, 0xCD, 0x76,
    0x19, 0x8D, 0xD5, 0x80, 0xD1, 0x33, 0x87, 0x13, 0x57, 0x18, 0x2D, 0x29, 0x78, 0xC3,
];

const FLT_LEN: usize = 81;
pub const RRC_TAPS: [f32; FLT_LEN] = [
    -0.003195702904062073,
    -0.002930279157647190,
    -0.001940667871554463,
    -0.000356087678023658,
    0.001547011339077758,
    0.003389554791179751,
    0.004761898604225673,
    0.005310860846138910,
    0.004824746306020221,
    0.003297923526848786,
    0.000958710871218619,
    -0.001749908029791816,
    -0.004238694106631223,
    -0.005881783042101693,
    -0.006150256456781309,
    -0.004745376707651645,
    -0.001704189656473565,
    0.002547854551539951,
    0.007215575568844704,
    0.011231038205363532,
    0.013421952197060707,
    0.012730475385624438,
    0.008449554307303753,
    0.000436744366018287,
    -0.010735380379191660,
    -0.023726883538258272,
    -0.036498030780605324,
    -0.046500883189991064,
    -0.050979050575999614,
    -0.047340680079891187,
    -0.033554880492651755,
    -0.008513823955725943,
    0.027696543159614194,
    0.073664520037517042,
    0.126689053778116234,
    0.182990955139333916,
    0.238080025892859704,
    0.287235637987091563,
    0.326040247765297220,
    0.350895727088112619,
    0.359452932027607974,
    0.350895727088112619,
    0.326040247765297220,
    0.287235637987091563,
    0.238080025892859704,
    0.182990955139333916,
    0.126689053778116234,
    0.073664520037517042,
    0.027696543159614194,
    -0.008513823955725943,
    -0.033554880492651755,
    -0.047340680079891187,
    -0.050979050575999614,
    -0.046500883189991064,
    -0.036498030780605324,
    -0.023726883538258272,
    -0.010735380379191660,
    0.000436744366018287,
    0.008449554307303753,
    0.012730475385624438,
    0.013421952197060707,
    0.011231038205363532,
    0.007215575568844704,
    0.002547854551539951,
    -0.001704189656473565,
    -0.004745376707651645,
    -0.006150256456781309,
    -0.005881783042101693,
    -0.004238694106631223,
    -0.001749908029791816,
    0.000958710871218619,
    0.003297923526848786,
    0.004824746306020221,
    0.005310860846138910,
    0.004761898604225673,
    0.003389554791179751,
    0.001547011339077758,
    -0.000356087678023658,
    -0.001940667871554463,
    -0.002930279157647190,
    -0.003195702904062073,
];
