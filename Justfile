set dotenv-load
set positional-arguments
set windows-powershell
set export

run *ARGS:
    cargo run -- $ARGS

test:
    cargo test

build:
    cargo build

install:
    cargo install --path .
