// Recorder AudioWorklet.
//
// Runs on the audio thread. It forwards raw Float32 samples to the main thread
// in small batches; the main thread is responsible for downsampling to 16 kHz
// and converting to PCM16, because resampling is not real-time critical.
//
// The capture rate is the AudioContext's rate (typically 48000), NOT 16000, so
// the consumer must resample. This mirrors how desktop dictation tools run the
// worklet at the device rate and encode downstream.

class RecorderProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.recording = false;
    this.chunkSize = 128; // one render quantum per message keeps latency low
    this.port.onmessage = (e) => {
      if (e.data === "start") this.recording = true;
      else if (e.data === "stop") this.recording = false;
    };
  }

  process(inputs) {
    if (!this.recording) return true;
    const input = inputs[0];
    if (input && input[0]) {
      // Copy: the underlying buffer is reused by the audio engine.
      this.port.postMessage({ samples: new Float32Array(input[0]), sampleRate });
    }
    return true;
  }
}

registerProcessor("recorder-processor", RecorderProcessor);
