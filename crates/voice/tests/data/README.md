# Test speech

`slt.raw` and `rms.raw`: 16-bit little-endian mono PCM at 16 kHz, made
with CMU Flite (`flite -voice slt|rms -t "…" -o …`), reading made-up
sentences. Synthetic speech, no one's recording; Flite's voices are free to
use, and their output belongs to no one. `tests/speech.rs` brings them up
to 48 kHz.

- slt (a woman's voice): "We will meet at the station at half past nine,
  and then walk down to the harbour together. Please call me back when you
  get this."
- rms (a man's voice): "The quick brown fox jumps over the lazy dog. Seven
  slim swans sailed slowly south, while the old clock struck twelve."

DeepFilterNet treats them as speech (it keeps them clean, to 25-34 dB
SDR), which hand-made test tones are not: it takes a synthetic buzz for
noise.
