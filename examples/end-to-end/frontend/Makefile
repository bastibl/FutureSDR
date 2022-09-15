.PHONY: wasm wasm-debug wasm-profiling serve clean setup

wasm:
	RUSTFLAGS='-C target-cpu=generic --cfg=web_sys_unstable_apis' wasm-pack build --target web --out-name frontend --out-dir ./dist --release
	./gulp

wasm-debug:
	RUSTFLAGS='-C target-cpu=generic --cfg=web_sys_unstable_apis' wasm-pack build --target web --out-name frontend --out-dir ./dist --debug
	./gulp

wasm-profiling:
	RUSTFLAGS='-C target-cpu=generic --cfg=web_sys_unstable_apis' wasm-pack build --target web --out-name frontend --out-dir ./dist --profiling
	./gulp

serve:
	cd dist && ../serve.py

clean:
	cargo clean
	rm -rf ./dist

setup:
	npm install
