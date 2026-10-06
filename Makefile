.PHONY: up down build test coverage coverage-gate lint fmt css bash prod-build

up:
	docker compose up --build

down:
	docker compose down

build:
	docker compose build

test:
	docker compose run --rm web sh -c 'tailwindcss -i assets/input.css -o static/css/app.css --minify && cargo test'

IGNORE_FOR_COVERAGE ?= src/main.rs|src/dokku/russh_client.rs

coverage:
	docker compose run --rm web sh -c 'tailwindcss -i assets/input.css -o static/css/app.css --minify && cargo llvm-cov --html --jobs 1 --ignore-filename-regex "$(IGNORE_FOR_COVERAGE)"'

coverage-gate:
	docker compose run --rm web sh -c 'tailwindcss -i assets/input.css -o static/css/app.css --minify && cargo llvm-cov --fail-under-lines 90 --jobs 1 --ignore-filename-regex "$(IGNORE_FOR_COVERAGE)"'

lint:
	docker compose run --rm web sh -c 'cargo fmt --check && cargo clippy --all-targets -- -D warnings'

fmt:
	docker compose run --rm web cargo fmt

css:
	docker compose run --rm web tailwindcss -i assets/input.css -o static/css/app.css --minify

bash:
	docker compose run --rm web bash

prod-build:
	docker build -t dokku-ui .