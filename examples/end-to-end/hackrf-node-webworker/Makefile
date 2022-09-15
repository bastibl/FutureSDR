.PHONY: wasm wasm-debug wasm-profiling serve clean setup

wasm:
	# building in a temporary directory and moving to the dist directory is required since wasm-opt fails when trying to optimize "hackrf_open.wasm"
	RUSTFLAGS='-C target-cpu=generic --cfg=web_sys_unstable_apis' wasm-pack build --target web --out-name hackrf_node_webworker --out-dir ./dist-tmp --release
	mkdir -p ./dist
	mv ./dist-tmp/* ./dist
	cd ../fft_worker && \
		make wasm && \
		cp -r ./dist/* ../hackrf-node-webworker/dist
	cd ../control_worker && \
    		make wasm && \
    		cp -r ./dist/* ../hackrf-node-webworker/dist
	./gulp

wasm-debug:
	# building in a temporary directory and moving to the dist directory is required since wasm-opt fails when trying to optimize "hackrf_open.wasm"
	RUSTFLAGS='-C target-cpu=generic --cfg=web_sys_unstable_apis' wasm-pack build --target web --out-name hackrf_node_webworker --out-dir ./dist-tmp --debug
	mkdir -p ./dist
	mv ./dist-tmp/* ./dist
	cd ../fft_worker && \
		make wasm-debug && \
		cp -r ./dist/* ../hackrf-node-webworker/dist
	cd ../control_worker && \
		make wasm-debug && \
		cp -r ./dist/* ../hackrf-node-webworker/dist
	./gulp

wasm-profiling:
	# building in a temporary directory and moving to the dist directory is required since wasm-opt fails when trying to optimize "hackrf_open.wasm"
	RUSTFLAGS='-C target-cpu=generic --cfg=web_sys_unstable_apis' wasm-pack build --target web --out-name hackrf_node_webworker --out-dir ./dist-tmp --profiling
	mkdir -p ./dist
	mv ./dist-tmp/* ./dist
	cd ../fft_worker && \
		make wasm-profiling && \
		cp -r ./dist/* ../hackrf-node-webworker/dist
	cd ../control_worker && \
		make wasm-profiling && \
		cp -r ./dist/* ../hackrf-node-webworker/dist
	./gulp

serve:
	cd dist && ../serve.py
clean:
	cargo clean
	rm -rf ./dist
	rm -rf ./dist-tmp
	cd ../fft_worker && \
		make clean
	cd ../control_worker && \
		make clean

setup:
	npm install
