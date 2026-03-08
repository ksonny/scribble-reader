#!/usr/bin/env bash

PID=$(adb shell pidof -s rust.lotrax.scribblereader)
if [[ -z "$PID" ]]
then
	echo "Failed to get pid" 2>&1
	exit 1
fi

adb logcat -v color --pid $PID
