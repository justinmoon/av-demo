# Marmot Chat workspace commands

MOQ_ROOT := env_var_or_default("MOQ_ROOT", env_var("HOME") + "/code/moq/moq")
RELAY_BIN := MOQ_ROOT + "/rs/target/debug/moq-relay"

default:
	@just --list

install:
	npm install

build:
	npm run build

build-wasm:
	npm run build:wasm

build-ui:
	npm run build:ui

wasm-test:
	wasm-pack test --node crates/marmot-chat

playwright:
	npm run test

web-test *ARGS:
	./scripts/test-web.sh {{ARGS}}

dev relay_port='54943' server_port='8890' nostr_port='7447' hosts='localhost,127.0.0.1':
	./scripts/run-web.sh --relay-port {{relay_port}} --server-port {{server_port}} --nostr-port {{nostr_port}} --hosts {{hosts}}

chat-dev port='8890':
	npm run build
	node apps/chat-ui/server.js --port {{port}}

relay-dev port='54943' hosts='localhost,127.0.0.1':
	"{{RELAY_BIN}}" --listen 127.0.0.1:{{port}} --tls-generate {{hosts}} --auth-public marmot --web-http-listen 127.0.0.1:{{port}}
