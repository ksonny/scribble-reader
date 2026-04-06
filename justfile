setup:
	cargo +stable install cargo-ndk
	cargo +stable install samply
	rustup target add aarch64-linux-android x86_64-linux-android

_build profile:
	#!/usr/bin/env bash
	set -e
	cd crates/app-android/
	VERSION=$(cargo pkgid | cut -d '@' -f2)
	echo "Build v$VERSION"
	cargo ndk \
		--target aarch64-linux-android \
		--target x86_64-linux-android \
		--output-dir app/src/main/jniLibs/ \
		build \
		--profile {{profile}}
	./gradlew -PversionName=$VERSION build
	cd -

build target="dev-opt": (_build target)
	@echo "Build {{target}}"

install target="dev-opt": (_build target)
	adb install ./crates//app-android/app/build/outputs/apk/release/app-release.apk

profile:
	cargo build --profile dev-opt
	samply record ./target/dev-opt/scribble-reader

launch:
	adb shell am start -n org.lotrax.scribblereader/.MainActivity

check:
	cargo ndk \
		--target x86_64-linux-android \
		--target aarch64-linux-android \
		--package scribble-reader-android \
		--output-dir ./crates/app-android/app/src/main/jniLibs/ \
		check

logcat:
	adb logcat \
		-v color \
		-s "scribble-reader:D","main-activity:D","RustStdoutStderr"
