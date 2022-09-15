.PHONY: backend-debug backend-release end-to-end clean setup

backend-debug:
	cargo run --debug

backend-release:
	cargo run --release

end-to-end:
	mkdir -p serve/frontend
	mkdir -p serve/node
	mkdir -p serve/recorded_node
	mkdir -p serve/css
	cd ../hackrf-node-webworker && \
		make wasm &&\
		cp -r ./dist/* ../backend/serve/node
	cd ../recorded-node-webworker && \
		make wasm &&\
		cp -r ./dist/* ../backend/serve/recorded_node
	cd ../frontend && \
		make wasm &&\
		cp -r ./dist/* ../backend/serve/frontend
	mv serve/frontend/favicon.png serve/favicon.png
	mv serve/frontend/css/* serve/css
	./scripts/init_db.sh
	cargo run --release --features incoming_data_counter

clean:
	rm -rf serve
	cd ../hackrf-node-webworker && make clean
	cd ../recorded-node-webworker && make clean
	cd ../frontend && make clean
	cd ../shared-utils && cargo clean
	cargo clean

setup:
	cd ../hackrf-node-webworker && make setup
	cd ../recorded-node-webworker && make setup
	cd ../frontend && make setup
