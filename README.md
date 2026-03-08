# Scribble Reader

## Containerized build

Build the image in podman

```sh
$ podman build -t rust-android:latest .
```

Build in image, from repo root:

```sh
$ podman run --rm -it -v "$PWD:/mnt" rust-android:latest --package scribble-reader --lib
```

