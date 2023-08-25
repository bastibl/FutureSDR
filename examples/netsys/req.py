#!/usr/bin/env python3

import requests
import json

print("all flowgraphs")
res = requests.get("http://127.0.0.1:1337/api/fg/")
j = res.json()
print(json.dumps(j, indent=2))

print("description of flowgraph 0")
res = requests.get("http://127.0.0.1:1337/api/fg/0/")
j = res.json()
print(json.dumps(j, indent=2))

print("description of block 0 (of flowgraph 0)")
res = requests.get("http://127.0.0.1:1337/api/fg/0/block/0/")
j = res.json()
print(json.dumps(j, indent=2))

print("call message handler 0 of block 0 w/ PMT::Null")
res = requests.get("http://127.0.0.1:1337/api/fg/0/block/0/call/0/")
j = res.json()
print(json.dumps(j, indent=2))

print("call message handler 'gain' of block 0 w/ PMT::Null")
res = requests.get("http://127.0.0.1:1337/api/fg/0/block/0/call/gain/")
j = res.json()
print(json.dumps(j, indent=2))

print("call message handler 'gain' of block 0 w/ PMT::F32(30.0)")
res = requests.post("http://127.0.0.1:1337/api/fg/0/block/0/call/gain/", json={"F32": 30.0})
j = res.json()
print(json.dumps(j, indent=2))

