# Test data

`sweep.eld` and `sweep.pcm` were made with libfdk-aac (Fedora's
fdk-aac-free) from a synthetic signal: a 200 Hz to 8 kHz sweep under a
440 Hz tone, two seconds at 48 kHz with 0.2 s of silence, encoded as
AAC-ELD (object type 39, 480 samples a frame, mono, 64 kbit/s, no SBR:
AudioSpecificConfig `f8 e6 30 00`, as AirPods' microphone stream), then
decoded by libfdk to 16-bit PCM. Nothing recorded from anyone.

`sweep.eld`: the AudioSpecificConfig, then each frame, each preceded by
its length (u16, little-endian). `sweep.pcm`: 16-bit little-endian
samples, 480 a frame.

The script (Python, ctypes on libfdk-aac.so.2) was kept outside the repo
when they were made; any libfdk build with ELD makes equivalent files.
