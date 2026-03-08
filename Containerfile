FROM docker.io/library/ubuntu:latest

# Variable arguments
ARG JAVA_VERSION=25
ARG NDK_VERSION=29.0.14033849
ARG BUILDTOOLS_VERSION=35.0.1
ARG PLATFORM_VERSION=android-35
ARG CLITOOLS_VERSION=13114758_latest

# Install Android requirements
RUN apt-get update -yqq \
	&& apt-get install -y --no-install-recommends \
		build-essential \
		libssl-dev \
		pkg-config \
		python3 \
		curl \
		ca-certificates \
		unzip \
		openjdk-${JAVA_VERSION}-jdk \
	&& apt-get clean \
	&& rm -rf /var/lib/apt/lists/*


# Generate Environment Variables
ENV JAVA_VERSION=${JAVA_VERSION}
ENV ANDROID_HOME=/opt/Android
ENV NDK_HOME=/opt/Android/ndk/${NDK_VERSION}
ENV ANDROID_NDK_ROOT=${NDK_HOME}
ENV PATH=$PATH:${ANDROID_HOME}:${ANDROID_NDK_ROOT}:${ANDROID_HOME}/build-tools/${BUILDTOOLS_VERSION}:${ANDROID_HOME}/cmdline-tools/bin

# Install command line tools && sdk
RUN mkdir -p ${ANDROID_HOME}/cmdline-tools \
	&& curl -O --proto '=https' --tlsv1.2 -sSf "https://dl.google.com/android/repository/commandlinetools-linux-${CLITOOLS_VERSION}.zip" \
	&& unzip -d ${ANDROID_HOME} ./commandlinetools-linux-${CLITOOLS_VERSION}.zip \
	&& rm ./commandlinetools-linux-${CLITOOLS_VERSION}.zip \
	&& yes | sdkmanager --sdk_root=${ANDROID_HOME} --install "build-tools;${BUILDTOOLS_VERSION}" "ndk;${NDK_VERSION}" "platforms;${PLATFORM_VERSION}" \
	&& rm -rf /tmp/*

# Create APK keystore for debug profile
# Adapted from https://github.com/rust-mobile/cargo-apk/blob/caa806283dc26733ad8232dce1fa4896c566f7b8/ndk-build/src/ndk.rs#L393-L423
RUN keytool -genkey -v \
	-keystore ${HOME}/.android/debug.keystore \
	-storepass android \
	-alias androiddebugkey \
	-keypass android \
	-dname 'CN=Android Debug,O=Android,C=US' \
	-keyalg RSA \
	-keysize 2048 \
	-validity 10000

# Install rustup
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Install android targets
RUN rustup target add aarch64-linux-android \
	&& rustup target add x86_64-linux-android \
	&& rustup component add rust-std --target aarch64-linux-android \
	&& rustup component add rust-std --target x86_64-linux-android \
	&& rustup update

# Install cargo-apk
RUN cargo install --locked cargo-apk

WORKDIR /mnt

ENTRYPOINT ["cargo", "apk", "build"]
