// AudioWorklet that batches mono input into ~100 ms Float32Array chunks.
// Plain JS on purpose: it is loaded by URL into the audio thread, unbundled.
class CaptureProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.size = 1600;
    this.buffer = new Float32Array(this.size);
    this.fill = 0;
    this.active = true;
    this.port.onmessage = (event) => {
      if (event.data === 'flush') {
        this.active = false;
        this.emit();
        this.port.postMessage({ type: 'flushed' });
      } else if (event.data === 'resume') {
        this.active = true;
        this.fill = 0;
      }
    };
  }

  emit() {
    if (this.fill === 0) return;
    const chunk = this.buffer.slice(0, this.fill);
    this.fill = 0;
    this.port.postMessage({ type: 'chunk', pcm: chunk }, [chunk.buffer]);
  }

  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (!this.active || !channel) return true;
    let offset = 0;
    while (offset < channel.length) {
      const n = Math.min(channel.length - offset, this.size - this.fill);
      this.buffer.set(channel.subarray(offset, offset + n), this.fill);
      this.fill += n;
      offset += n;
      if (this.fill === this.size) this.emit();
    }
    return true;
  }
}

registerProcessor('flow-capture', CaptureProcessor);
