.PHONY: wasm wasm-debug wasm-profiling serve clean setup

wasm:
	RUSTFLAGS='-C target-cpu=generic --cfg=web_sys_unstable_apis' wasm-pack build --target web --out-name recorded_node_webworker --out-dir ./dist --release
	cd ../fft_worker && \
		make wasm && \
		cp -r ./dist/* ../recorded-node-webworker/dist
	cd ../control_worker && \
    		make wasm && \
    		cp -r ./dist/* ../recorded-node-webworker/dist
	./gulp

wasm-debug:
	RUSTFLAGS='-C target-cpu=generic --cfg=web_sys_unstable_apis' wasm-pack build --target web --out-name recorded_node_webworker --out-dir ./dist --debug
	cd ../fft_worker && \
		make wasm-debug && \
		cp -r ./dist/* ../recorded-node-webworker/dist
	cd ../control_worker && \
		make wasm-debug && \
		cp -r ./dist/* ../recorded-node-webworker/dist
	./gulp

wasm-profiling:
	RUSTFLAGS='-C target-cpu=generic --cfg=web_sys_unstable_apis' wasm-pack build --target web --out-name recorded_node_webworker --out-dir ./dist --profiling
	cd ../fft_worker && \
		make wasm-profiling && \
		cp -r ./dist/* ../recorded-node-webworker/dist
	cd ../control_worker && \
		make wasm-profiling && \
		cp -r ./dist/* ../recorded-node-webworker/dist
	./gulp

serve:
	cd dist && ../serve.py
	
clean:
	cargo clean
	rm -rf ./dist
	cd ../fft_worker && \
		make clean
	cd ../control_worker && \
		make clean

setup:
	npm install
