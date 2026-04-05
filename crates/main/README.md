# Running on Android

```bash
export ANDROID_HOME="path/to/sdk"
export ANDROID_NDK_HOME="path/to/ndk"

rustup target add aarch64-linux-android
cargo install cargo-apk
```

Connect your Android device via USB cable to your computer in debug mode and run the following command

```bash
cargo apk run --package scribble-reader --lib
```

## Running on Desktop

Sometimes it is helpful to run your Android apps on a Desktop environment (e.g., Windows, macOS, or
Linux). It works the same way as all other `pixels` examples:

```bash
cargo run
```
