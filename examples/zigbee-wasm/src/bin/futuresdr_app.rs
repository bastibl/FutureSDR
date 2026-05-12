#[cfg(not(target_arch = "wasm32"))]
pub fn main() {
    println!("This is a WASM-only application. Please use trunk to run it.");
}

#[cfg(target_arch = "wasm32")]
pub fn main() {
    zigbee_wasm::frontend::wasm_main();
}
