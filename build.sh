#!/usr/bin/env bash

podman build --quiet -t rust-android:latest .

podman run --rm -it \
	-v ~/.cargo/registry:/root/.cargo/registry \
	-v ~/.cargo/git:/root/.cargo/git \
	-v "$PWD:/mnt" rust-android:latest \
	--package scribble-reader \
	--lib

