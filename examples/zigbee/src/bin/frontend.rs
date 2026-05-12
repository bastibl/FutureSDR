#[cfg(not(all(target_arch = "wasm32", feature = "frontend")))]
pub fn main() {
    println!(
        "This is a WASM-only frontend. Please build it with trunk and the `frontend` feature."
    );
}

#[cfg(all(target_arch = "wasm32", feature = "frontend"))]
pub fn main() {
    zigbee::frontend::frontend();
}
