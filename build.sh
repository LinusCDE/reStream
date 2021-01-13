#!/bin/sh

cargo build --target=armv7-unknown-linux-gnueabihf --release --bin restream
cp target/armv7-unknown-linux-gnueabihf/release/restream restream.arm.static
cargo build --release --bin unxor
cp target/release/unxor .
cross build --target=x86_64-pc-windows-gnu --release --bin unxor
cp target/x86_64-pc-windows-gnu/release/unxor.exe .
