// « Jouer ici » : lecteur WebAudio qui consomme le flux PCM binaire de /ws/audio.
// Trames : texte JSON {"type":"audio_spec"|"audio_mode"|"audio_lag"}, binaire = f32 LE entrelacés.

export class BrowserAudio {
  private ws: WebSocket | null = null;
  private ctx: AudioContext | null = null;
  private nextTime = 0;
  private rate = 44100;
  private channels = 2;
  private running = false;

  get active(): boolean {
    return this.running;
  }

  start(): void {
    if (this.ws) return;
    this.running = true;
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/ws/audio`);
    ws.binaryType = "arraybuffer";
    ws.onmessage = (e) => this.onMessage(e);
    ws.onclose = () => {
      this.ws = null;
    };
    ws.onerror = () => {
      this.ws = null;
    };
    this.ws = ws;
  }

  stop(): void {
    this.running = false;
    try {
      this.ws?.close();
    } catch { /* déjà fermé */ }
    this.ws = null;
    if (this.ctx) {
      void this.ctx.close().catch(() => {});
      this.ctx = null;
    }
    this.nextTime = 0;
  }

  private ensureCtx(): AudioContext {
    if (!this.ctx) {
      const AC = window.AudioContext ?? (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
      this.ctx = new AC();
    }
    if (this.ctx.state === "suspended") void this.ctx.resume();
    return this.ctx;
  }

  private onMessage(e: MessageEvent): void {
    if (typeof e.data === "string") {
      try {
        const m = JSON.parse(e.data) as { type?: string; sample_rate?: number; channels?: number };
        if (m.type === "audio_spec") {
          this.rate = m.sample_rate ?? 44100;
          this.channels = m.channels ?? 2;
          this.nextTime = 0; // resync à chaque changement de format
        }
        // audio_mode / audio_lag : informatifs, rien à faire
      } catch { /* trame ignorée */ }
      return;
    }
    const samples = new Float32Array(e.data as ArrayBuffer);
    if (samples.length > 0) this.play(samples);
  }

  private play(samples: Float32Array): void {
    if (!this.running) return;
    const ctx = this.ensureCtx();
    const frames = Math.floor(samples.length / this.channels);
    if (frames === 0) return;
    const buf = ctx.createBuffer(this.channels, frames, this.rate);
    for (let ch = 0; ch < this.channels; ch++) {
      const d = buf.getChannelData(ch);
      for (let i = 0; i < frames; i++) d[i] = samples[i * this.channels + ch];
    }
    const src = ctx.createBufferSource();
    src.buffer = buf;
    src.connect(ctx.destination);
    // lookahead de 120 ms ; si on a pris trop de retard (pause, lag), on resync
    if (this.nextTime < ctx.currentTime + 0.05) this.nextTime = ctx.currentTime + 0.12;
    src.start(this.nextTime);
    this.nextTime += buf.duration;
  }
}
