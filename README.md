# Scribble Reader

E-ink optimized ebook reader for Android with a focus on efficiency and minimal distractions.

## Why?

Most of my books are large ones that gets appended to weekly, primarily generated with [epub-builder](https://github.com/crowdagger/epub-builder);
This makes location tracking and efficient read patterns important, and also disqualifies most readers available for free.
It is also a sufficiently complicated problem for a hobby project. :D

## Download

Grab the apk from [latest release](https://github.com/ksonny/scribble-reader/releases/latest) and sideload to your device.
Alternatively, use [Obtanium](https://obtainium.imranr.dev/).

## Crates

* `app-android` - Android activity & glue
* `main` - Main event loop, wgpu rendering and views
* `scribe` - Models, database, Epub parsing and settings
* `illustrator` - Epub layouting
* `sculpter` - Font shaping and printing
* `wrangler` - File system abstraction for Android storage madness

## Develop Android

Requires Android Studio & Java installed to build the Android app!

Create `crates/app-android/keystore.properties` with these properties set according to your local setup:

```properties
storeFile=
storePassword=
keyAlias=
keyPassword=

```

* `cargo install just`
* `just setup`
* `just build`

See also `just install` to upload the resulting apk to your device.
