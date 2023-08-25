#!/bin/bash

set -xe

echo "list of all running flowgraphs"
curl -sL http://127.0.0.1:1337/api/fg/ | jq

echo "description of flowgraph 0"
curl -sL http://127.0.0.1:1337/api/fg/0/ | jq

echo "description of block 0 (of flowgraph 0)"
curl -sL http://127.0.0.1:1337/api/fg/0/block/0/ | jq

echo "call message handler 0 of block 0 w/ PMT::Null"
curl -sL http://127.0.0.1:1337/api/fg/0/block/0/call/0/ | jq

echo "call message handler 'gain' of block 0 w/ PMT::Null"
curl -sL http://127.0.0.1:1337/api/fg/0/block/0/call/gain/ | jq

echo "call message handler 'gain' of block 0 w/ PMT::F32(30.0)"
curl -X POST -H "Content-Type: application/json" -d '{"F32": 30.0}' -sL http://127.0.0.1:1337/api/fg/0/block/0/call/gain/ | jq

