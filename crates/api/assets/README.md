`holding-v1.wav` is a six-second, locally generated instrumental chime (8 kHz,
16-bit mono PCM). It contains no speech or third-party recording. The API embeds
it at compile time and serves it at `/audio/holding-v1.wav` with `audio/wav` and
immutable cache headers. Twilio speaks the explanation separately using `<Say>`,
then plays this file using `<Play>` before gathering more speech.

When replacing the recording, change the filename and URL version as well so
Twilio does not reuse the old cached audio.
