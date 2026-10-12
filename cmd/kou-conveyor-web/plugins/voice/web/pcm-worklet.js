// The voice plugin's audio worklet: what the microphone hears, as 16-bit
// PCM, mono, at the rate asked (16 kHz), in chunks of a tenth of a second
// posted to the page with the loudest sample of each. Downsampling averages
// the samples each output sample stands for: a box filter, enough for
// speech. Asked to flush, it posts what it holds at once, then says so.
class VoicePCM extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const rate = Math.min(options?.processorOptions?.rate || 16000, sampleRate);
    this.step = sampleRate / rate;
    this.position = 0;
    this.sum = 0;
    this.count = 0;
    this.chunk = new Int16Array(Math.round(rate / 10));
    this.filled = 0;
    this.peak = 0;
    this.port.onmessage = ({ data }) => {
      if (data !== 'flush') return;
      this.post();
      this.port.postMessage({ flushed: true });
    };
  }

  process(inputs) {
    const channels = inputs[0];
    if (!channels?.length) return true;
    const frames = channels[0].length;
    for (let i = 0; i < frames; i++) {
      let sample = 0;
      for (let c = 0; c < channels.length; c++) sample += channels[c][i];
      this.sum += sample / channels.length;
      this.count++;
      this.position++;
      if (this.position >= this.step) {
        this.position -= this.step;
        this.push(this.sum / this.count);
        this.sum = 0;
        this.count = 0;
      }
    }
    return true;
  }

  push(value) {
    const sample = Math.max(-1, Math.min(1, value));
    this.peak = Math.max(this.peak, Math.abs(sample));
    this.chunk[this.filled++] = sample < 0 ? sample * 0x8000 : sample * 0x7fff;
    if (this.filled === this.chunk.length) this.post();
  }

  post() {
    if (!this.filled) return;
    const pcm = this.chunk.slice(0, this.filled).buffer;
    this.port.postMessage({ pcm, level: this.peak }, [pcm]);
    this.filled = 0;
    this.peak = 0;
  }
}

registerProcessor('kou-voice-pcm', VoicePCM);
