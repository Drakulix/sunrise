
# Sunrise

This repository contains a bunch of experiments surrounding the nvidia gamestream protocol
and reimplementations in the form of sunshine and moonlight.

## capture

Capture is a test client for the wayland export-dmabuf protocol

## comp

Is an absolute minimal wayland compositor, designed to be driven by an export-dmabuf client (e.g. Sunshine).
It is designed to run one fullscreen application for maximum performance (though it can handle multiple applications)
and to directly pass on client buffers to the capturing client, if possible.

(Doesn't work, the buffer format seems to mismatch what is expected by sunshine for the nvidia-driver at least.
Sunshines EGL code is convoluted at best and likely buggy...)

## host

Early attempts at re-implementing sunshine and combining that with `comp` at some point.

## rtsp-types

Fork of the [`rtsp-types`](crates.io/crates/rtsp-types) crate for `host`


